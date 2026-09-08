mod app;
mod common;
mod config;
mod infrastructure;
mod modules;

use app::create_app;
use config::AppConfig;
use infrastructure::database;
use sqlx::postgres::PgPool;
use tracing_subscriber::EnvFilter;

#[tokio::main(worker_threads = 4)]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("server error: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = AppConfig::new()?; // 创建应用配置
    init_tracing(&config.log.level); // 初始化日志系统

    let db_pool: PgPool =
        database::create_pool(&config.database.url, config.database.max_connections).await?; // 创建数据库连接池
    database::run_migrations(&db_pool).await?; // 在接收请求前完成数据库结构迁移
    let app = create_app(db_pool); // 创建应用程序实例
    let address: String = config.server_address(); // 获取服务器地址
    let listener: tokio::net::TcpListener = tokio::net::TcpListener::bind(&address).await?; // 绑定服务器地址

    tracing::info!(%address, "server started"); // 记录服务器启动信息
    axum::serve(listener, app).await?; // 启动服务器并监听请求
    Ok(())
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info")); // 设置日志过滤器，默认级别为 info
    tracing_subscriber::fmt().with_env_filter(filter).init(); // 初始化日志订阅器，使用环境变量过滤器
}
