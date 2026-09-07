use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

/// 创建数据库连接池
pub async fn create_pool(database_url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect(database_url)
        .await?;

    tracing::info!(max_connections, "database pool connected");

    Ok(pool)
}
