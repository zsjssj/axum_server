use sqlx::migrate::MigrateError;
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

/// 执行编译时嵌入的数据库迁移，确保部署产物不依赖运行目录中的迁移文件
pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await?;

    tracing::info!("database migrations completed");

    Ok(())
}
