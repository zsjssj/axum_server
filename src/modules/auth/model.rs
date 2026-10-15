use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

/// 登录请求。
#[derive(Debug, Deserialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    #[validate(length(min = 3, max = 50, message = "用户名长度必须在 3 到 50 个字符之间"))]
    #[schema(min_length = 3, max_length = 50)]
    pub username: String,
    /// 登录密码。
    ///
    /// 刻意不声明长度校验：登录接口的职责是核验凭证而不是约束密码格式。
    /// 若在此处校验长度，密码策略一旦调整就会把存量用户挡在门外，
    /// 且不同的长度提示会对外泄露服务端的密码策略。
    #[schema(min_length = 1, max_length = 128, write_only)]
    pub password: String,
}

/// 令牌对，仅用于服务层内部传递。
///
/// 刻意不派生 `Serialize` 与 `ToSchema`：令牌通过 `Set-Cookie` 响应头下发，
/// 绝不能进入响应体。类型上不可序列化可以从根本上避免后续被误放进 JSON。
#[derive(Debug)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    /// access token 剩余有效秒数。
    pub expires_in: i64,
}

impl TokenPair {
    pub fn new(access_token: String, refresh_token: String, expires_in: i64) -> Self {
        Self {
            access_token,
            refresh_token,
            expires_in,
        }
    }
}

/// 登录与刷新接口的响应体。
///
/// 只回传与客户端调度有关的信息，令牌本身只存在于 `Set-Cookie` 响应头中，
/// 页面脚本无法通过响应体读取令牌。
#[derive(Debug, Serialize, ToSchema)]
pub struct AuthSessionResponse {
    /// 认证方式固定为 `cookie`，表示凭据已通过 Cookie 下发。
    pub token_type: String,
    /// access token 剩余有效秒数，供客户端提前刷新。
    pub expires_in: i64,
    /// 已下发的 Cookie 名称，便于客户端确认与排查。
    pub cookies: CookieNames,
}

/// 已下发的令牌 Cookie 名称。
#[derive(Debug, Serialize, ToSchema)]
pub struct CookieNames {
    pub access_token: String,
    pub refresh_token: String,
}

impl AuthSessionResponse {
    pub fn new(expires_in: i64, access_name: &str, refresh_name: &str) -> Self {
        Self {
            token_type: "cookie".to_owned(),
            expires_in,
            cookies: CookieNames {
                access_token: access_name.to_owned(),
                refresh_token: refresh_name.to_owned(),
            },
        }
    }
}

/// 当前登录用户响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct CurrentUserResponse {
    pub public_id: Uuid,
    pub username: String,
    pub email: String,
    pub nickname: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_login_request() {
        let request = LoginRequest {
            username: "testuser".to_owned(),
            password: "any-password".to_owned(),
        };

        request.validate().expect("合法登录参数应通过校验");
    }

    #[test]
    fn rejects_short_username() {
        let request = LoginRequest {
            username: "ab".to_owned(),
            password: "any-password".to_owned(),
        };

        let errors = request.validate().expect_err("过短用户名应校验失败");
        assert!(errors.field_errors().contains_key("username"));
    }

    /// 登录不校验密码长度：过短的密码应由凭证核验失败来拒绝，
    /// 而不是返回 422 泄露服务端密码策略。
    #[test]
    fn does_not_validate_password_length() {
        let request = LoginRequest {
            username: "testuser".to_owned(),
            password: "x".to_owned(),
        };

        request.validate().expect("登录不应校验密码长度");
    }

    #[test]
    fn token_pair_carries_tokens_and_expiry() {
        let pair = TokenPair::new("access".to_owned(), "refresh".to_owned(), 900);

        assert_eq!(pair.access_token, "access");
        assert_eq!(pair.refresh_token, "refresh");
        assert_eq!(pair.expires_in, 900);
    }

    /// 响应体只含调度信息，不包含令牌本身。
    #[test]
    fn auth_session_response_exposes_cookie_names_not_tokens() {
        let response = AuthSessionResponse::new(900, "access_token", "refresh_token");

        assert_eq!(response.token_type, "cookie");
        assert_eq!(response.expires_in, 900);
        assert_eq!(response.cookies.access_token, "access_token");
        assert_eq!(response.cookies.refresh_token, "refresh_token");
    }
}
