use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;
use validator::ValidationErrors;

use crate::common::response::{ApiCode, ApiResponse};

/// 参数校验失败时返回给客户端的单个字段错误。
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorDetail {
    pub field: String,   // 出错字段名；请求体整体校验失败时使用 `_request`。
    pub message: String, // 面向调用方的中文错误原因，取自校验规则中声明的 message。
}

/// 校验失败时的附加数据，作为 `ApiResponse::data` 返回。
#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorData {
    /// 按字段名排序后的字段错误列表，保证响应稳定便于客户端处理。
    pub details: Vec<ErrorDetail>,
}

/// 应用统一错误边界。
///
/// Handler、Service、提取器等各层统一返回该类型，由 [`IntoResponse`] 实现集中
/// 转换为 HTTP 状态码、业务码与 `ApiResponse` 响应信封，避免各层重复拼装错误响应。
/// 变体携带的 `&'static str` 为可直接展示给调用方的中文提示，不应包含数据库结构、
/// SQL 或连接信息等敏感内容。
#[derive(Debug)]
pub enum AppError {
    // 字段级参数校验失败，对应 `422` 与业务码 `40001`，响应中会附带 `ErrorData` 明细。
    Validation(ValidationErrors),
    /// 请求格式或参数不合法，例如路径、查询参数解析失败，对应 `400` 与业务码 `40000`。
    BadRequest(&'static str),
    /// 请求语义不可处理，例如 JSON 结构正确但字段取值非法，对应 `422` 与业务码 `40001`。
    UnprocessableEntity(&'static str),
    /// 请求媒体类型不受支持，例如 `Content-Type` 非 `application/json`，对应 `415` 与业务码 `40002`。
    UnsupportedMediaType(&'static str),
    /// 目标资源不存在，例如未知接口或查询不到记录，对应 `404` 与业务码 `40400`。
    NotFound(&'static str),
    /// 请求方法不被路由允许，对应 `405` 与业务码 `40500`，提示语固定无需携带内容。
    MethodNotAllowed,
    /// 依赖服务不可用，例如数据库探测失败，对应 `503` 与业务码 `50300`。
    ServiceUnavailable(&'static str),
    /// 业务状态冲突，例如唯一键重复，对应 `409` 与业务码 `40900`。
    Conflict(&'static str),
    /// 数据库操作失败，细节仅写入服务端日志，对外统一返回 `500` 与业务码 `50000`。
    Database(sqlx::Error),
    /// 内部未预期错误，例如任务失败，对外统一返回 `500` 与业务码 `50000`。
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
