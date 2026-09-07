mod app;
mod common;
mod config;
mod infrastructure;
mod modules;

use app::create_app;
use config::AppConfig;
use infrastructure::database;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("server error: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = AppConfig::new()?;
    init_tracing(&config.log.level);

    let db_pool =
        database::create_pool(&config.database.url, config.database.max_connections).await?;
    let app = create_app(db_pool);
    let address = config.server_address();
    let listener = tokio::net::TcpListener::bind(&address).await?;

    tracing::info!(%address, "server started");
    axum::serve(listener, app).await?;
    Ok(())
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
