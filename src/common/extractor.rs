use axum::{
    async_trait,
    extract::{
        FromRequest, FromRequestParts, Json, Path, Query, Request, rejection::JsonRejection,
    },
    http::request::Parts,
};
use serde::de::DeserializeOwned;
use std::{fmt::Debug, str::FromStr};
use validator::Validate;

use crate::common::error::AppError;

/// 完成 JSON 反序列化后执行模型校验，并将两类错误都转换为统一响应。
pub struct ValidatedJson<T>(pub T);

#[async_trait]
impl<S, T> FromRequest<S> for ValidatedJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Validate,
{
    type Rejection = AppError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(request, state)
            .await
            .map_err(json_rejection_to_error)?;
        value.validate()?;
        Ok(Self(value))
    }
}

/// 完成查询参数反序列化后执行模型校验。
pub struct ValidatedQuery<T>(pub T);

#[async_trait]
impl<S, T> FromRequestParts<S> for ValidatedQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Validate,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(value) = Query::<T>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::BadRequest("查询参数格式错误"))?;
        value.validate()?;
        Ok(Self(value))
    }
}

/// 包装路径参数提取，避免 Axum 的纯文本拒绝响应绕过统一错误格式。
pub struct ApiPath<T>(pub T);

#[async_trait]
impl<S, T> FromRequestParts<S> for ApiPath<T>
where
    S: Send + Sync,
    T: FromStr + Send,
    T::Err: Debug,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(raw_value) = Path::<String>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::BadRequest("路径参数缺失"))?;
        let value = raw_value
            .parse::<T>()
            .map_err(|_| AppError::BadRequest("路径参数格式错误"))?;
        Ok(Self(value))
    }
}

fn json_rejection_to_error(rejection: JsonRejection) -> AppError {
    match rejection {
        JsonRejection::MissingJsonContentType(_) => {
            AppError::UnsupportedMediaType("Content-Type 必须为 application/json")
        }
        JsonRejection::JsonDataError(_) => AppError::UnprocessableEntity("JSON 请求字段不合法"),
        JsonRejection::JsonSyntaxError(_) => AppError::BadRequest("JSON 请求体语法错误"),
        JsonRejection::BytesRejection(_) => AppError::BadRequest("读取请求体失败"),
        _ => AppError::BadRequest("JSON 请求体格式错误"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use uuid::Uuid;

    #[tokio::test]
    async fn parses_valid_uuid_path_value() {
        async fn handler(ApiPath(value): ApiPath<Uuid>) -> String {
            value.to_string()
        }

        let app = Router::new().route("/users/:public_id", get(handler));
        let id = Uuid::now_v7();
        let response = tower::ServiceExt::oneshot(
            app,
            axum::http::Request::builder()
                .uri(format!("/users/{id}"))
                .body(axum::body::Body::empty())
                .expect("测试请求应构造成功"),
        )
        .await
        .expect("测试请求应成功进入 Router");

        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }
}
