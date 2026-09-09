use serde::Serialize;
use utoipa::ToSchema;

/// 所有接口共用的响应信封，业务数据统一放在 `data` 中。
#[derive(Debug, Serialize, ToSchema)]
pub struct ApiResponse<T> {
    /// 业务状态码，成功固定为 0，失败使用稳定的分类编码。
    pub code: i32,
    pub message: String,
    pub data: Option<T>,
}

/// 无业务数据时用于生成 OpenAPI Schema 的占位类型。
#[derive(Debug, Serialize, ToSchema)]
pub struct EmptyData {}

impl<T> ApiResponse<T> {
    pub fn success(message: impl Into<String>, data: T) -> Self {
        Self {
            code: ApiCode::Success.into(),
            message: message.into(),
            data: Some(data),
        }
    }

    pub fn error(code: ApiCode, message: impl Into<String>, data: Option<T>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            data,
        }
    }
}

impl ApiResponse<()> {
    pub fn success_without_data(message: impl Into<String>) -> Self {
        Self {
            code: ApiCode::Success.into(),
            message: message.into(),
            data: None,
        }
    }
}

/// 数字业务码与 HTTP 状态码相互独立，便于客户端稳定处理和后续扩展。
#[derive(Debug, Clone, Copy)]
#[repr(i32)]
pub enum ApiCode {
    Success = 0, //接口成功
    BadRequest = 40000,
    ValidationError = 40001,
    UnsupportedMediaType = 40002,
    NotFound = 40400, //未知接口
    MethodNotAllowed = 40500,
    Conflict = 40900,
    InternalError = 50000, //服务器错误
    ServiceUnavailable = 50300,
}

impl From<ApiCode> for i32 {
    fn from(code: ApiCode) -> Self {
        code as Self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_success_response_with_data() {
        let response = ApiResponse::success("查询成功", 42);

        assert_eq!(response.code, 0);
        assert_eq!(response.message, "查询成功");
        assert_eq!(response.data, Some(42));
    }

    #[test]
    fn creates_success_response_without_data() {
        let response = ApiResponse::success_without_data("删除成功");

        assert_eq!(response.code, 0);
        assert!(response.data.is_none());
    }
}
