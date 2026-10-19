mod app;
mod common;
mod config;
mod infrastructure;
mod modules;

use anyhow::Context;
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
        // 用 Debug 而不是 Display：anyhow 的 Debug 会按 `Caused by` 逐层打印错误链，
        // 并遵守 RUST_BACKTRACE。启动失败往往发生在容器里、看不到交互式输出，
        // 这一步的根因如果只剩一句摘要，排障时只能靠猜是哪一步、哪个依赖出的问题。
        eprintln!("server error: {error:?}");
        std::process::exit(1);
    }
}

/// 启动流程的顶层错误类型。
///
/// 这里刻意用 `anyhow` 而不是 `AppError`：`AppError` 的职责是「运行期请求错误 → HTTP 响应」，
/// 它的 `IntoResponse` 决定了状态码与业务码；而启动失败没有请求、没有响应，只有一连串
/// 需要按顺序处理的初始化步骤，需要的是可读的上下文链而不是状态码。
///
/// 边界（不要放宽）：`anyhow` 只允许出现在本文件。Handler / Service / Repository 必须继续
/// 返回带类型的 `AppError`，否则 400/401/409 的映射会被压平成 500，`&'static str` 提示语
/// 提供的泄露防护也会失效。
async fn run() -> anyhow::Result<()> {
    let config = AppConfig::new().context("加载应用配置失败")?;
    // 先校验配置再初始化其他组件：弱 JWT 密钥等问题必须在监听端口之前暴露，
    // 否则服务会带着可被伪造的令牌签发能力对外提供服务。
    config.validate().context("应用配置校验失败")?;
    init_tracing(&config.log.level); // 初始化日志系统

    let db_pool: PgPool = database::create_pool(&config.database.url, config.database.max_connections)
        .await
        .context("创建数据库连接池失败")?; // 创建数据库连接池
    database::run_migrations(&db_pool).await.context("数据库迁移失败")?; // 在接收请求前完成数据库结构迁移
    // 迁移之后、监听端口之前创建引导账号：数据库为空时没有任何账号能通过登录，
    // 而创建用户的接口自身也要求登录，这一步是唯一能让空库变得可用的出口。
    bootstrap::ensure_bootstrap_user(&db_pool, &config.bootstrap)
        .await
        .context("引导账号初始化失败")?;
    // Redis 是可选加速组件：创建失败或首次探测失败都只告警，不阻断启动。
    // 理由是本项目的每一项 Redis 功能都有降级路径（详见 docs/reference/REDIS_GUIDE.md），
    // 为了让一个缓存组件拖垮整个服务并不划算；同时 /ready 会把实例标记为未就绪，
    // 生产环境的编排系统据此不会把流量路由到没有 Redis 的实例上。
    let redis_store = RedisStore::from_config(&config.redis).context("Redis 初始化失败")?;
    if let Err(error) = redis_store.ping().await {
        tracing::warn!(%error, "Redis 不可用，登录限流、令牌吊销与用户缓存将降级运行");
    }
    let app = create_app(db_pool, redis_store, &config); // 创建应用程序实例
    let address: String = config.server_address(); // 获取服务器地址
    // 绑定失败携带地址：端口被占用是启动期最常见的故障，日志里没有地址就只能去翻配置。
    let listener: tokio::net::TcpListener = tokio::net::TcpListener::bind(&address)
        .await
        .with_context(|| format!("绑定监听地址 {address} 失败"))?;

    tracing::info!(%address, "server started"); // 记录服务器启动信息
    // 使用带连接信息的服务：登录与刷新接口依赖客户端 IP 做审计记录。
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .await
        .context("HTTP 服务异常退出")?; // 启动服务器并监听请求
    Ok(())
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info")); // 设置日志过滤器，默认级别为 info
    tracing_subscriber::fmt().with_env_filter(filter).init(); // 初始化日志订阅器，使用环境变量过滤器
}
