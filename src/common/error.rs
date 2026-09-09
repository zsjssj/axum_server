use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;
use validator::ValidationErrors;

use crate::common::response::{ApiCode, ApiResponse};

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorDetail {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorData {
    pub details: Vec<ErrorDetail>,
}

#[derive(Debug)]
pub enum AppError {
    Validation(ValidationErrors),
    BadRequest(&'static str),
    UnprocessableEntity(&'static str),
    UnsupportedMediaType(&'static str),
    NotFound(&'static str),
    MethodNotAllowed,
    ServiceUnavailable(&'static str),
    Conflict(&'static str),
    Database(sqlx::Error),
    Internal(&'static str),
}

impl From<sqlx::Error> for AppError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl From<ValidationErrors> for AppError {
    fn from(errors: ValidationErrors) -> Self {
        Self::Validation(errors)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message, details) = match self {
            Self::Validation(errors) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ApiCode::ValidationError,
                "请求参数校验失败",
                validation_details(&errors),
            ),
            Self::BadRequest(message) => (
                StatusCode::BAD_REQUEST,
                ApiCode::BadRequest,
                message,
                Vec::new(),
            ),
            Self::UnprocessableEntity(message) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ApiCode::ValidationError,
                message,
                Vec::new(),
            ),
            Self::UnsupportedMediaType(message) => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                ApiCode::UnsupportedMediaType,
                message,
                Vec::new(),
            ),
            Self::NotFound(message) => (
                StatusCode::NOT_FOUND,
                ApiCode::NotFound,
                message,
                Vec::new(),
            ),
            Self::MethodNotAllowed => (
                StatusCode::METHOD_NOT_ALLOWED,
                ApiCode::MethodNotAllowed,
                "请求方法不被允许",
                Vec::new(),
            ),
            Self::ServiceUnavailable(message) => (
                StatusCode::SERVICE_UNAVAILABLE,
                ApiCode::ServiceUnavailable,
                message,
                Vec::new(),
            ),
            Self::Conflict(message) => {
                (StatusCode::CONFLICT, ApiCode::Conflict, message, Vec::new())
            }
            Self::Database(error) => {
                tracing::error!(%error, "数据库操作失败");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ApiCode::InternalError,
                    "服务器内部错误",
                    Vec::new(),
                )
            }
            Self::Internal(message) => {
                tracing::error!(%message, "内部操作失败");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ApiCode::InternalError,
                    "服务器内部错误",
                    Vec::new(),
                )
            }
        };

        (
            status,
            Json(ApiResponse::error(
                code,
                message,
                (!details.is_empty()).then_some(ErrorData { details }),
            )),
        )
            .into_response()
    }
}

fn validation_details(errors: &ValidationErrors) -> Vec<ErrorDetail> {
    let mut details = errors
        .field_errors()
        .iter()
        .flat_map(|(field, errors)| {
            errors.iter().map(move |error| ErrorDetail {
                field: if field == "__all__" {
                    "_request".to_owned()
                } else {
                    field.to_string()
                },
                message: error
                    .message
                    .as_deref()
                    .unwrap_or("字段值不合法")
                    .to_owned(),
            })
        })
        .collect::<Vec<_>>();
    details.sort_by(|left, right| left.field.cmp(&right.field));
    details
}

#[cfg(test)]
mod tests {
    use super::*;
    use validator::Validate;

    #[derive(Debug, Validate)]
    struct TestRequest {
        #[validate(length(min = 1, message = "名称不能为空"))]
        name: String,
    }

    #[test]
    fn converts_validation_errors_to_field_details() {
        let errors = TestRequest {
            name: String::new(),
        }
        .validate()
        .expect_err("空名称应校验失败");
        let details = validation_details(&errors);

        assert_eq!(details.len(), 1);
        assert_eq!(details[0].field, "name");
        assert_eq!(details[0].message, "名称不能为空");
    }
}
