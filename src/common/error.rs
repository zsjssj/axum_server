use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug)]
pub enum AppError {
    NotFound(&'static str), // 资源未找到
    Conflict(&'static str), // 请求冲突
    Database(sqlx::Error),  // 数据库错误
    Internal(&'static str), // 内部错误
}

impl From<sqlx::Error> for AppError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound(message) => (StatusCode::NOT_FOUND, message),
            Self::Conflict(message) => (StatusCode::CONFLICT, message),
            Self::Database(error) => {
                tracing::error!(%error, "database operation failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "服务器内部错误")
            }
            Self::Internal(message) => {
                tracing::error!(%message, "internal operation failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "服务器内部错误")
            }
        };

        (
            status,
            Json(ErrorResponse {
                error: message.to_owned(),
            }),
        )
            .into_response()
    }
}
