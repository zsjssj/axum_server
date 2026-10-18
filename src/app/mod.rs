pub mod bootstrap;
pub mod middleware;
pub mod openapi;
pub mod routes;
pub mod state;

use crate::app::middleware::{cors_layer, logging_middleware};
use crate::app::state::AppState;
use crate::config::AppConfig;
use crate::infrastructure::redis::RedisStore;
use axum::Router;
use sqlx::PgPool;

/// 创建 Axum 应用
///
/// 直接接收 [`AppConfig`] 而不是拆散的配置项：后续新增需要共享的配置
/// 时无需再修改该函数签名和所有调用点。
///
/// [`RedisStore`] 由调用方创建后传入而不是在这里根据配置构造：
/// 创建过程可能失败（连接地址非法），把失败点放在启动流程里，
/// 才能给出「哪个组件初始化失败」的明确错误，而不是在 `create_app` 中 panic。
pub fn create_app(db_pool: PgPool, redis: RedisStore, config: &AppConfig) -> Router {
    let state = AppState::new(db_pool, redis, config.jwt.clone(), config.auth.clone());

    Router::new()
        .merge(routes::health_routes())
        .merge(routes::api_routes(state.clone()))
        .merge(openapi::routes())
        .fallback(routes::not_found_handler)
        .method_not_allowed_fallback(routes::method_not_allowed_handler)
        .layer(cors_layer())
        // .layer(axum::middleware::from_fn(logging_middleware)) // 旧版日志中间件，已弃用，使用自定义的 TraceLayer 函数
        .layer(logging_middleware()) // 新版日志中间件，使用 tower-http 提供的 TraceLayer
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

    use crate::config::{AuthConfig, JwtConfig};
    use crate::modules::auth::service::AuthService;

    /// 与测试配置 `auth.cookie.access_name` 保持一致的 Cookie 名称。
    const ACCESS_COOKIE_NAME: &str = "access_token";

    /// 测试令牌的 `exp` 取值。取一个远大于当前时间的绝对值，
    /// 使用例不会随系统时间推移而突然变成「令牌已过期」。
    const LONG_LIVED_TOKEN_EXPIRY: i64 = 4_102_444_800; // 2100-01-01T00:00:00Z

    fn test_app() -> Router {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://user:password@localhost/test")
            .expect("测试数据库连接字符串应合法");
        // Redis 使用不建立连接的实例：用例只覆盖路由、校验与认证中间件，
        // 不依赖真实 Redis；Redis 命令失败时的降级行为由各模块的单元测试覆盖。
        create_app(pool, crate::infrastructure::redis::RedisStore::without_connection(), &test_config())
    }

    /// 构造测试用配置。
    ///
    /// 密钥长度需满足启动校验要求，否则相关断言会在校验阶段就失败。
    fn test_config() -> AppConfig {
        AppConfig {
            server: crate::config::ServerConfig {
                host: "127.0.0.1".to_owned(),
                port: 3000,
            },
            database: crate::config::DatabaseConfig {
                url: "postgres://user:password@localhost/test".to_owned(),
                max_connections: 1,
            },
            log: crate::config::LogConfig { level: "info".to_owned() },
            redis: crate::config::RedisConfig {
                url: "redis://127.0.0.1:6379".to_owned(),
                pool_size: 2,
                key_prefix: "axum-test".to_owned(),
                user_cache_ttl_seconds: 60,
            },
            jwt: JwtConfig {
                secret: "integration-test-secret-32-characters".to_owned(),
                issuer: "rust-axum-server-test".to_owned(),
                access_ttl_minutes: 15,
                refresh_ttl_days: 7,
            },
            auth: AuthConfig {
                max_login_failures: 5,
                login_window_minutes: 15,
                cookie: crate::config::CookieConfig {
                    access_name: "access_token".to_owned(),
                    refresh_name: "refresh_token".to_owned(),
                    secure: false,
                    same_site: "strict".to_owned(),
                    path: "/".to_owned(),
                },
            },
            bootstrap: crate::config::BootstrapConfig {
                // 测试不初始化数据库，引导账号必须保持关闭。
                enabled: false,
                username: "admin".to_owned(),
                email: "admin@example.com".to_owned(),
                password: String::new(),
                nickname: None,
            },
        }
    }

    async fn request(method: Method, uri: &str, body: Body, content_type: bool) -> Response {
        dispatch(method, uri, body, content_type, None).await
    }

    /// 发起一个已登录请求。
    ///
    /// `/api/v1` 下的全部接口都受认证中间件保护，未携带令牌的请求会在进入
    /// Handler 之前就被拦下。因此「请求体格式」「参数校验」这类针对业务逻辑的
    /// 断言必须带上合法令牌，否则用例实际测到的是 `401` 而不是目标行为。
    async fn request_as_logged_in_user(method: Method, uri: &str, body: Body, content_type: bool) -> Response {
        let token = AuthService::issue_test_access_token(&test_config().jwt, 1, LONG_LIVED_TOKEN_EXPIRY);

        dispatch(method, uri, body, content_type, Some(&token)).await
    }

    /// 组装并发送请求；`access_token` 为 `None` 时表示匿名请求。
    async fn dispatch(method: Method, uri: &str, body: Body, content_type: bool, access_token: Option<&str>) -> Response {
        let mut builder = Request::builder().method(method).uri(uri);
        if content_type {
            builder = builder.header("content-type", "application/json");
        }
        if let Some(token) = access_token {
            builder = builder.header("cookie", format!("{}={}", ACCESS_COOKIE_NAME, token));
        }

        test_app()
            .oneshot(builder.body(body).expect("测试请求应构造成功"))
            .await
            .expect("测试请求应成功进入 Router")
    }

    async fn json_body(response: Response) -> Value {
        let bytes = response.into_body().collect().await.expect("响应体应可读取").to_bytes();
        serde_json::from_slice(&bytes).expect("错误响应应为合法 JSON")
    }

    #[tokio::test]
    async fn returns_field_details_for_invalid_json_request() {
        let response = request_as_logged_in_user(
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
        let response = request_as_logged_in_user(Method::POST, "/api/v1/users", Body::from("{"), true).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = json_body(response).await;
        assert_eq!(body["code"], 40000);
        assert!(body["data"].is_null());
    }

    #[tokio::test]
    async fn returns_unified_error_for_invalid_query_and_path() {
        let query_response = request_as_logged_in_user(Method::GET, "/api/v1/users?page=0&page_size=101", Body::empty(), false).await;
        assert_eq!(query_response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json_body(query_response).await["code"], 40001);

        let path_response = request_as_logged_in_user(Method::GET, "/api/v1/users/not-a-uuid", Body::empty(), false).await;
        assert_eq!(path_response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json_body(path_response).await["code"], 40000);
    }

    #[tokio::test]
    async fn returns_unified_error_for_unknown_route_and_method() {
        // 不在 `/api/v1` 下的路径不受认证中间件影响，直接走 404 fallback。
        let not_found = request(Method::GET, "/missing", Body::empty(), false).await;
        assert_eq!(not_found.status(), StatusCode::NOT_FOUND);
        assert_eq!(json_body(not_found).await["code"], 40400);

        // 已登录时方法不匹配应返回 405；未登录时认证中间件会在进入业务逻辑之前
        // 就返回 401，因此这里必须带上令牌才能测到 405 本身。
        let method_not_allowed = request_as_logged_in_user(Method::PATCH, "/api/v1/hello", Body::empty(), false).await;
        assert_eq!(method_not_allowed.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(json_body(method_not_allowed).await["code"], 40500);
    }

    #[tokio::test]
    async fn wraps_success_response_in_api_envelope() {
        let response = request_as_logged_in_user(Method::GET, "/api/v1/hello", Body::empty(), false).await;

        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["message"], "请求成功");
        assert_eq!(body["data"]["message"], "Hello from Rust Axum Server!");
    }

    /// `/api/v1` 下的接口默认需要登录：不带令牌必须被拦下，且不能进入 Handler。
    #[tokio::test]
    async fn rejects_anonymous_requests_under_api_prefix() {
        for (method, uri) in [
            (Method::GET, "/api/v1/hello"),
            (Method::GET, "/api/v1/users"),
            (Method::POST, "/api/v1/users"),
            (Method::GET, "/api/v1/auth/me"),
        ] {
            let response = request(method.clone(), uri, Body::empty(), false).await;

            assert_eq!(
                response.status(),
                StatusCode::UNAUTHORIZED,
                "{method} {uri} 未登录时必须返回 401"
            );
            let body = json_body(response).await;
            assert_eq!(body["code"], 40100, "{method} {uri} 应返回未认证业务码");
            assert_eq!(body["message"], "未登录或登录已过期");
        }
    }

    /// 非法令牌与缺失令牌同样返回 401，不能因为解析失败而放行。
    #[tokio::test]
    async fn rejects_invalid_access_token() {
        let response = dispatch(
            Method::GET,
            "/api/v1/hello",
            Body::empty(),
            false,
            Some("not-a-jwt"),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(json_body(response).await["code"], 40100);
    }

    /// 与上一条用例互为对照：同一路径带上合法令牌后必须穿过中间件。
    ///
    /// 用例环境没有真实数据库，因此这里断言的不是业务成功，而是**响应不再是
    /// 未认证错误**——这足以证明中间件识别出令牌并放行，请求已进入 Service 层。
    #[tokio::test]
    async fn lets_authenticated_request_reach_business_layer() {
        let response = request_as_logged_in_user(Method::GET, "/api/v1/users", Body::empty(), false).await;
        let body = json_body(response).await;

        assert_ne!(body["code"], 40100, "合法令牌不应被认证中间件拒绝");
    }

    /// 未注册的路径不会触发认证，直接落到 404 fallback。
    ///
    /// 这是 `route_layer` 的既有语义（中间件只作用于已匹配到路由的请求），
    /// 也符合 HTTP 直觉：不存在的接口没必要先要求登录。这里把它固定下来，
    /// 避免有人以为「未知路径会先返回 401」而依赖错误的行为。
    #[tokio::test]
    async fn returns_not_found_for_unknown_api_route() {
        let response = request(Method::GET, "/api/v1/not-exists", Body::empty(), false).await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(json_body(response).await["code"], 40400);
    }

    /// 健康检查与根路径属于运维探针，不能纳入登录校验。
    #[tokio::test]
    async fn keeps_health_routes_public() {
        for uri in ["/", "/health"] {
            let response = request(Method::GET, uri, Body::empty(), false).await;

            assert_eq!(response.status(), StatusCode::OK, "{uri} 不应要求登录");
        }
    }

    /// 登出必须匿名可达。判断依据是**响应结果而不是状态码**：认证中间件拦截时
    /// 返回 `401 未登录或登录已过期`，而登出是幂等操作，即使没有刷新令牌也应
    /// 返回成功。只有请求真正进入 Handler 才会得到 `code = 0`。
    #[tokio::test]
    async fn keeps_logout_reachable_without_token() {
        let response = request(Method::POST, "/api/v1/auth/logout", Body::empty(), true).await;
        let body = json_body(response).await;

        assert_eq!(body["code"], 0, "登出不应被认证中间件拦截");
    }

    /// 刷新必须匿名可达，否则 access token 过期后客户端无法续期。
    ///
    /// 携带合法的 access token Cookie、但没有 `refresh_token`：认证中间件
    /// 只看 access token，因此请求会被放行；Handler 随后因缺少刷新令牌返回
    /// 自己的文案。看到「缺少刷新令牌」而非中间件的「未登录或登录已过期」，
    /// 就证明请求确实穿过了中间件。同时该断言不需要访问数据库。
    #[tokio::test]
    async fn keeps_refresh_reachable_without_token() {
        let access_token = AuthService::issue_test_access_token(&test_config().jwt, 1, LONG_LIVED_TOKEN_EXPIRY);
        let response = dispatch(Method::POST, "/api/v1/auth/refresh", Body::empty(), true, Some(&access_token)).await;
        let body = json_body(response).await;

        assert_eq!(body["message"], "缺少刷新令牌", "刷新接口应进入 Handler 而不是被中间件拦截");
    }

    /// 登录必须匿名可达。这里用无法通过校验的请求体：只要响应不是中间件的
    /// 「未登录或登录已过期」，就说明请求已进入 Handler 并走到了参数校验。
    #[tokio::test]
    async fn keeps_login_reachable_without_token() {
        let response = request(
            Method::POST,
            "/api/v1/auth/login",
            Body::from(json!({ "username": "ab" }).to_string()),
            true,
        )
        .await;
        let body = json_body(response).await;

        assert_eq!(body["code"], 40001, "登录接口应进入 Handler 并返回参数校验错误");
    }
}
