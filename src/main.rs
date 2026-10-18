mod app;
mod common;
mod config;
mod infrastructure;
mod modules;

use app::bootstrap;
use app::create_app;
use config::AppConfig;
use infrastructure::database;
use infrastructure::redis::RedisStore;
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
    // 先校验配置再初始化其他组件：弱 JWT 密钥等问题必须在监听端口之前暴露，
    // 否则服务会带着可被伪造的令牌签发能力对外提供服务。
    config.validate()?;
    init_tracing(&config.log.level); // 初始化日志系统

    let db_pool: PgPool = database::create_pool(&config.database.url, config.database.max_connections).await?; // 创建数据库连接池
    database::run_migrations(&db_pool).await?; // 在接收请求前完成数据库结构迁移
    // 迁移之后、监听端口之前创建引导账号：数据库为空时没有任何账号能通过登录，
    // 而创建用户的接口自身也要求登录，这一步是唯一能让空库变得可用的出口。
    bootstrap::ensure_bootstrap_user(&db_pool, &config.bootstrap)
        .await
        .map_err(|error| format!("引导账号初始化失败: {error:?}"))?;
    // Redis 是可选加速组件：创建失败或首次探测失败都只告警，不阻断启动。
    // 理由是本项目的每一项 Redis 功能都有降级路径（详见 docs/REDIS_GUIDE.md），
    // 为了让一个缓存组件拖垮整个服务并不划算；同时 /ready 会把实例标记为未就绪，
    // 生产环境的编排系统据此不会把流量路由到没有 Redis 的实例上。
    let redis_store = RedisStore::from_config(&config.redis)
        .map_err(|error| format!("Redis 初始化失败: {error}"))?;
    if let Err(error) = redis_store.ping().await {
        tracing::warn!(%error, "Redis 不可用，登录限流、令牌吊销与用户缓存将降级运行");
    }
    let app = create_app(db_pool, redis_store, &config); // 创建应用程序实例
    let address: String = config.server_address(); // 获取服务器地址
    let listener: tokio::net::TcpListener = tokio::net::TcpListener::bind(&address).await?; // 绑定服务器地址

    tracing::info!(%address, "server started"); // 记录服务器启动信息
    // 使用带连接信息的服务：登录与刷新接口依赖客户端 IP 做审计记录。
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await?; // 启动服务器并监听请求
    Ok(())
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info")); // 设置日志过滤器，默认级别为 info
    tracing_subscriber::fmt().with_env_filter(filter).init(); // 初始化日志订阅器，使用环境变量过滤器
}
