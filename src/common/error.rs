use crate::common::response::{ApiCode, ApiResponse};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;
use utoipa::ToSchema;
use validator::ValidationErrors;

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
///
/// `Display` 与 `source` 由 `thiserror` 派生：`Display` 只是日志与启动报错用的摘要，
/// **不作为**响应体内容，对外提示一律由 [`IntoResponse`] 里的常量决定，因此可以安全地
/// 与响应文案分开维护；`source` 链则由 `#[from]` 自动建立，不必再手写 `source()` 分支。
#[derive(Debug, Error)]
pub enum AppError {
    // 字段级参数校验失败，对应 `422` 与业务码 `40001`，响应中会附带 `ErrorData` 明细。
    #[error("请求参数校验失败")]
    Validation(#[from] ValidationErrors),
    /// 请求格式或参数不合法，例如路径、查询参数解析失败，对应 `400` 与业务码 `40000`。
    #[error("{0}")]
    BadRequest(&'static str),
    /// 请求语义不可处理，例如 JSON 结构正确但字段取值非法，对应 `422` 与业务码 `40001`。
    #[error("{0}")]
    UnprocessableEntity(&'static str),
    /// 请求媒体类型不受支持，例如 `Content-Type` 非 `application/json`，对应 `415` 与业务码 `40002`。
    #[error("{0}")]
    UnsupportedMediaType(&'static str),
    /// 目标资源不存在，例如未知接口或查询不到记录，对应 `404` 与业务码 `40400`。
    #[error("{0}")]
    NotFound(&'static str),
    /// 请求方法不被路由允许，对应 `405` 与业务码 `40500`，提示语固定无需携带内容。
    #[error("请求方法不被允许")]
    MethodNotAllowed,
    /// 已认证但无权执行该操作，对应 `403` 与业务码 `40300`。
    ///
    /// 与 `NotFound` 的分工按「会泄露什么」判断：当资源本身的存在与否属于隐私
    /// （例如别人的会话、别人的聊天记录），必须返回 `404`——`403` 等于告诉对方
    /// 「这条记录存在，只是不属于你」，顺手提供了一个探测接口。只有调用方本来
    /// 就能看见的资源上才用 `403`，例如群成员可以看见群，但没有「添加成员」权限。
    #[error("{0}")]
    Forbidden(&'static str),
    /// 未认证或凭证无效，对应 `401` 与业务码 `40100`。
    ///
    /// 登录失败统一使用该变体且提示语保持一致，避免通过响应差异区分
    /// 「用户不存在」与「密码错误」，形成用户名枚举通道。
    #[error("{0}")]
    Unauthorized(&'static str),
    /// 触发登录保护等频率限制，对应 `429` 与业务码 `42900`。
    #[error("{0}")]
    TooManyRequests(&'static str),
    /// 依赖服务不可用，例如数据库探测失败，对应 `503` 与业务码 `50300`。
    #[error("{0}")]
    ServiceUnavailable(&'static str),
    /// 业务状态冲突，例如唯一键重复，对应 `409` 与业务码 `40900`。
    #[error("{0}")]
    Conflict(&'static str),
    /// 数据库操作失败，细节仅写入服务端日志，对外统一返回 `500` 与业务码 `50000`。
    ///
    /// `Display` 只保留「数据库操作失败」这一个摘要，原始原因留在 `source` 链里：
    /// 万一将来有人把 `AppError` 的 `Display` 写进响应，也不会带出 SQL 与连接信息。
    #[error("数据库操作失败")]
    Database(#[from] sqlx::Error),
    /// 内部未预期错误，例如任务失败，对外统一返回 `500` 与业务码 `50000`。
    #[error("{0}")]
    Internal(&'static str),
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
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, ApiCode::BadRequest, message, Vec::new()),
            Self::UnprocessableEntity(message) => (StatusCode::UNPROCESSABLE_ENTITY, ApiCode::ValidationError, message, Vec::new()),
            Self::UnsupportedMediaType(message) => (StatusCode::UNSUPPORTED_MEDIA_TYPE, ApiCode::UnsupportedMediaType, message, Vec::new()),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, ApiCode::NotFound, message, Vec::new()),
            Self::MethodNotAllowed => (StatusCode::METHOD_NOT_ALLOWED, ApiCode::MethodNotAllowed, "请求方法不被允许", Vec::new()),
            Self::Forbidden(message) => (StatusCode::FORBIDDEN, ApiCode::Forbidden, message, Vec::new()),
            Self::Unauthorized(message) => (StatusCode::UNAUTHORIZED, ApiCode::Unauthorized, message, Vec::new()),
            Self::TooManyRequests(message) => (StatusCode::TOO_MANY_REQUESTS, ApiCode::TooManyRequests, message, Vec::new()),
            Self::ServiceUnavailable(message) => (StatusCode::SERVICE_UNAVAILABLE, ApiCode::ServiceUnavailable, message, Vec::new()),
            Self::Conflict(message) => (StatusCode::CONFLICT, ApiCode::Conflict, message, Vec::new()),
            Self::Database(error) => {
                tracing::error!(%error, "数据库操作失败");
                (StatusCode::INTERNAL_SERVER_ERROR, ApiCode::InternalError, "服务器内部错误", Vec::new())
            }
            Self::Internal(message) => {
                tracing::error!(%message, "内部操作失败");
                (StatusCode::INTERNAL_SERVER_ERROR, ApiCode::InternalError, "服务器内部错误", Vec::new())
            }
        };

        (
            status,
            Json(ApiResponse::error(code, message, (!details.is_empty()).then_some(ErrorData { details }))),
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
                field: if field == "__all__" { "_request".to_owned() } else { field.to_string() },
                message: error.message.as_deref().unwrap_or("字段值不合法").to_owned(),
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
        let errors = TestRequest { name: String::new() }.validate().expect_err("空名称应校验失败");
        let details = validation_details(&errors);

        assert_eq!(details.len(), 1);
        assert_eq!(details[0].field, "name");
        assert_eq!(details[0].message, "名称不能为空");
    }

    /// `Display` 与 `source` 改由 thiserror 派生后，必须保证「摘要对外、原因进 source」这条约束不变：
    /// 数据库原因不能混进 `Display`，否则一旦被写进响应就是信息泄露。
    #[test]
    fn keeps_database_cause_out_of_display() {
        let error = AppError::from(sqlx::Error::RowNotFound);

        assert_eq!(error.to_string(), "数据库操作失败");
        assert!(std::error::Error::source(&error).is_some(), "数据库原因必须挂在 source 上");
    }

    /// 携带自定义提示的变体，`Display` 就是该提示本身，便于启动期直接打印给人看。
    #[test]
    fn display_matches_variant_message() {
        assert_eq!(AppError::NotFound("用户不存在").to_string(), "用户不存在");
        assert_eq!(AppError::MethodNotAllowed.to_string(), "请求方法不被允许");
    }
}
