use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug)]
pub enum AppError {
    NotFound(&'static str),
    Conflict(&'static str),
    Database(sqlx::Error),
    Internal(&'static str),
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

        (status, Json(json!({ "error": message }))).into_response()
    }
}
