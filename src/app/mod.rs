pub mod middleware;
pub mod openapi;
pub mod routes;
pub mod state;

use crate::app::middleware::{cors_layer, logging_middleware};
use crate::app::state::AppState;
use axum::Router;
use sqlx::PgPool;

/// 创建 Axum 应用
pub fn create_app(db_pool: PgPool) -> Router {
    let state = AppState::new(db_pool);

    Router::new()
        .merge(routes::health_routes())
        .merge(routes::api_routes())
        .merge(openapi::routes())
        .fallback(routes::not_found_handler)
        .method_not_allowed_fallback(routes::method_not_allowed_handler)
        .layer(cors_layer())
        .layer(axum::middleware::from_fn(logging_middleware))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
        response::Response,
    };
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    fn test_app() -> Router {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://user:password@localhost/test")
            .expect("测试数据库连接字符串应合法");
        create_app(pool)
    }

    async fn request(method: Method, uri: &str, body: Body, content_type: bool) -> Response {
        let mut builder = Request::builder().method(method).uri(uri);
        if content_type {
            builder = builder.header("content-type", "application/json");
        }

        test_app()
            .oneshot(builder.body(body).expect("测试请求应构造成功"))
            .await
            .expect("测试请求应成功进入 Router")
    }

    async fn json_body(response: Response) -> Value {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("响应体应可读取")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("错误响应应为合法 JSON")
    }

    #[tokio::test]
    async fn returns_field_details_for_invalid_json_request() {
        let response = request(
            Method::POST,
            "/api/v1/users",
            Body::from(
                json!({
                    "username": "ab",
                    "email": "invalid-email",
                    "password": "short"
                })
                .to_string(),
            ),
            true,
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = json_body(response).await;
        assert_eq!(body["code"], 40001);
        assert_eq!(body["message"], "请求参数校验失败");
        assert_eq!(body["data"]["details"].as_array().map(Vec::len), Some(3));
    }

    #[tokio::test]
    async fn returns_unified_error_for_malformed_json() {
        let response = request(Method::POST, "/api/v1/users", Body::from("{"), true).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = json_body(response).await;
        assert_eq!(body["code"], 40000);
        assert!(body["data"].is_null());
    }

    #[tokio::test]
    async fn returns_unified_error_for_invalid_query_and_path() {
        let query_response = request(
            Method::GET,
            "/api/v1/users?page=0&page_size=101",
            Body::empty(),
            false,
        )
        .await;
        assert_eq!(query_response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json_body(query_response).await["code"], 40001);

        let path_response = request(
            Method::GET,
            "/api/v1/users/not-a-uuid",
            Body::empty(),
            false,
        )
        .await;
        assert_eq!(path_response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json_body(path_response).await["code"], 40000);
    }

    #[tokio::test]
    async fn returns_unified_error_for_unknown_route_and_method() {
        let not_found = request(Method::GET, "/missing", Body::empty(), false).await;
        assert_eq!(not_found.status(), StatusCode::NOT_FOUND);
        assert_eq!(json_body(not_found).await["code"], 40400);

        let method_not_allowed =
            request(Method::PATCH, "/api/v1/hello", Body::empty(), false).await;
        assert_eq!(method_not_allowed.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(json_body(method_not_allowed).await["code"], 40500);
    }

    #[tokio::test]
    async fn wraps_success_response_in_api_envelope() {
        let response = request(Method::GET, "/api/v1/hello", Body::empty(), false).await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["message"], "请求成功");
        assert_eq!(body["data"]["message"], "Hello from Rust Axum Server!");
    }
}
