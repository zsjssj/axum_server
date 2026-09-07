use sqlx::PgPool;

/// 应用全局状态
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
}

impl AppState {
    pub fn new(db_pool: PgPool) -> Self {
        Self { db: db_pool }
    }
}
