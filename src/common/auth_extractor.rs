use axum::{async_trait, extract::FromRequestParts, http::request::Parts};

use crate::app::state::AppState;
use crate::common::auth::AuthUser;
use crate::common::error::AppError;

/// 已认证用户身份。
///
/// Handler 签名中出现 `CurrentUser` 即表示该接口需要登录，身份来源是
/// [`crate::common::auth::require_login`] 中间件写入请求扩展的 [`AuthUser`]。
///
/// 提取器本身不再解析 Cookie：令牌校验集中在中间件中完成，一处校验、
/// 一处失败响应；若两边各自校验，同一请求会被重复解码，后续新增校验规则时
/// 也极易出现「中间件放了、提取器没放」的不一致。
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
        _state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // 扩展缺失说明该路由没有经过认证中间件。这种情况必须失败关闭而不是
        // 放行为匿名访问，否则一次路由改写就可能静默地把接口暴露出去。
        parts
            .extensions
            .get::<AuthUser>()
            .map(|user| Self {
                user_id: user.user_id,
            })
            .ok_or_else(|| {
                tracing::error!(uri = %parts.uri, "请求缺少认证中间件注入的用户身份");
                AppError::Internal("认证上下文缺失")
            })
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
            redis: crate::infrastructure::redis::RedisStore::without_connection(),
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

    fn parts() -> Parts {
        let request = axum::http::Request::builder()
            .uri("/api/v1/auth/me")
            .body(())
            .expect("测试请求应构造成功");
        let (parts, ()) = request.into_parts();
        parts
    }

    #[tokio::test]
    async fn reads_user_from_request_extensions() {
        let state = test_state();
        let mut parts = parts();
        parts.extensions.insert(AuthUser { user_id: 42 });

        let user = CurrentUser::from_request_parts(&mut parts, &state)
            .await
            .expect("已注入身份的请求应提取成功");

        assert_eq!(user.user_id, 42);
    }

    /// 没有经过认证中间件的请求必须失败关闭，而不是被当成匿名访问放行。
    #[tokio::test]
    async fn fails_closed_without_authenticated_user() {
        let state = test_state();
        let mut parts = parts();

        let result = CurrentUser::from_request_parts(&mut parts, &state).await;

        match result {
            Err(AppError::Internal(message)) => assert_eq!(message, "认证上下文缺失"),
            other => panic!("缺少认证上下文时应返回内部错误，实际为 {other:?}"),
        }
    }
}
