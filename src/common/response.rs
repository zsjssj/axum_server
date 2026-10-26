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
    ///接口成功返回：有数据版
    pub fn success(message: impl Into<String>, data: T) -> Self {
        Self {
            code: ApiCode::Success.into(),
            message: message.into(),
            data: Some(data),
        }
    }
    ///接口失败返回
    pub fn error(code: ApiCode, message: impl Into<String>, data: Option<T>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            data,
        }
    }
}

impl ApiResponse<()> {
    ///接口成功返回：无数据版
    pub fn success_without_data(message: impl Into<String>) -> Self {
        Self {
            code: ApiCode::Success.into(),
            message: message.into(),
            data: None,
        }
    }
}

/// 数字业务码与 HTTP 状态码相互独立，便于客户端稳定处理和后续扩展。
///
/// 显式标注每个变体的数值，避免依赖枚举声明顺序导致业务码意外漂移。
#[derive(Debug, Clone, Copy)]
#[repr(i32)]
pub enum ApiCode {
    /// 成功。按接口契约固定为 `0`，与 HTTP 状态码 `200` 无关，因此不能复用 `200`。
    Success = 0,
    /// 请求本身不合法（格式错误、缺少必要参数等），无法进入业务校验。
    BadRequest = 40000,
    /// 参数已解析但未通过业务校验（如字段格式、取值范围不符合约定）。
    ValidationError = 40001,
    /// 请求的 `Content-Type` 不被接口支持。
    UnsupportedMediaType = 40002,
    /// 未认证或凭证无效。
    Unauthorized = 40100,
    /// 已认证但无权执行该操作（如普通成员尝试管理群成员）。
    Forbidden = 40300,
    /// 请求的资源或接口不存在。
    NotFound = 40400,
    /// 请求方法对该接口不被允许。
    MethodNotAllowed = 40500,
    /// 请求与资源当前状态冲突（如重复创建、版本冲突）。
    Conflict = 40900,
    /// 触发登录保护等频率限制。
    TooManyRequests = 42900,
    /// 服务器内部错误，通常不向客户端暴露细节。
    InternalError = 50000,
    /// 服务暂时不可用（如依赖服务故障、维护中），可稍后重试。
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
