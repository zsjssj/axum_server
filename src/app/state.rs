use std::sync::Arc;

use sqlx::PgPool;

use crate::config::{AuthConfig, JwtConfig};

/// 应用全局状态
///
/// 认证配置随状态共享：提取器需要在校验令牌时访问 JWT 密钥，
/// 若通过参数逐层传递会让每个需要认证的 Handler 都多一个依赖参数。
/// `Arc` 保证克隆状态时不会复制配置内容。
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub jwt: Arc<JwtConfig>,
    pub auth: Arc<AuthConfig>,
}

impl AppState {
    pub fn new(db_pool: PgPool, jwt: JwtConfig, auth: AuthConfig) -> Self {
        Self {
            db: db_pool,
            jwt: Arc::new(jwt),
            auth: Arc::new(auth),
        }
    }
}
