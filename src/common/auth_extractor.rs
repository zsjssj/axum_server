use axum::{async_trait, extract::FromRequestParts, http::request::Parts};

use crate::app::state::AppState;
use crate::common::cookie;
use crate::common::error::AppError;
use crate::modules::auth::service::AuthService;

/// 已认证用户身份。
///
/// 以提取器的形式提供认证能力：Handler 签名中出现 `CurrentUser` 即表示该接口
/// 需要登录，意图直接体现在类型上，无需回看路由配置或中间件注册顺序。
#[derive(Debug, Clone, Copy)]
pub struct CurrentUser {
    /// 用户内部主键，取自 access token 的 `sub`。
    pub user_id: i64,
}

#[async_trait]
impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // 令牌只从 HttpOnly Cookie 读取，不接受 `Authorization` 头。
        // 保留单一来源可以避免出现绕过 Cookie 安全属性的旁路认证方式，
        // 也避免"某条路径漏校验 Cookie"的差异化处理。
        let token = cookie::access_token_from_headers(&parts.headers, &state.auth.cookie)
            .ok_or(AppError::Unauthorized("缺少认证令牌"))?;

        let user_id = AuthService::verify_access_token(&state.jwt, &token)?;

        Ok(Self { user_id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::config::{AuthConfig, CookieConfig, JwtConfig};

    fn test_state() -> AppState {
        AppState {
            db: sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgres://user:password@localhost/test")
                .expect("测试数据库连接字符串应合法"),
            jwt: Arc::new(JwtConfig {
                secret: "unit-test-secret-with-enough-length-32".to_owned(),
                issuer: "rust-axum-server-test".to_owned(),
                access_ttl_minutes: 15,
                refresh_ttl_days: 7,
            }),
            auth: Arc::new(AuthConfig {
                max_login_failures: 5,
                login_window_minutes: 15,
                cookie: CookieConfig {
                    access_name: "access_token".to_owned(),
                    refresh_name: "refresh_token".to_owned(),
                    secure: false,
                    same_site: "strict".to_owned(),
                    path: "/".to_owned(),
                },
            }),
        }
    }

    fn parts_with_cookie(value: &str) -> Parts {
        let request = axum::http::Request::builder()
            .uri("/api/v1/auth/me")
            .header(axum::http::header::COOKIE, value)
            .body(())
            .expect("测试请求应构造成功");
        let (parts, ()) = request.into_parts();
        parts
    }

    #[tokio::test]
    async fn rejects_request_without_token_cookie() {
        let state = test_state();
        let mut parts = parts_with_cookie("other=1");

        let result = CurrentUser::from_request_parts(&mut parts, &state).await;

        match result {
            Err(AppError::Unauthorized(message)) => assert_eq!(message, "缺少认证令牌"),
            other => panic!("缺少令牌时应返回 401，实际为 {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_invalid_token_cookie() {
        let state = test_state();
        let mut parts = parts_with_cookie("access_token=not-a-valid-jwt");

        let result = CurrentUser::from_request_parts(&mut parts, &state).await;

        assert!(
            matches!(result, Err(AppError::Unauthorized(_))),
            "非法令牌必须被拒绝"
        );
    }

    /// 提取器必须只信任 Cookie：`Authorization` 头不再被接受，
    /// 避免出现绕过 Cookie 安全属性的旁路认证方式。
    #[tokio::test]
    async fn ignores_authorization_header() {
        let state = test_state();
        let request = axum::http::Request::builder()
            .uri("/api/v1/auth/me")
            .header(axum::http::header::AUTHORIZATION, "Bearer some.jwt.value")
            .body(())
            .expect("测试请求应构造成功");
        let (mut parts, ()) = request.into_parts();

        let result = CurrentUser::from_request_parts(&mut parts, &state).await;

        assert!(
            matches!(result, Err(AppError::Unauthorized(_))),
            "Authorization 头不应被接受"
        );
    }
}
