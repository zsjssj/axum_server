use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use std::env;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub log: LogConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogConfig {
    pub level: String,
}

impl AppConfig {
    pub fn new() -> Result<Self, ConfigError> {
        // 加载 .env 文件（如果存在）
        dotenv::dotenv().ok();

        // 获取运行环境，默认为 development
        let run_mode = env::var("RUN_MODE").unwrap_or_else(|_| "development".into());

        let config = Config::builder()
            // 加载默认配置
            .add_source(File::with_name("config/default"))
            // 根据环境加载配置文件（如 config/development.toml 或 config/production.toml）
            .add_source(File::with_name(&format!("config/{}", run_mode)).required(false))
            // 本地配置覆盖（不提交到版本控制）
            .add_source(File::with_name("config/local").required(false))
            // 环境变量覆盖（使用 APP_ 前缀）
            .add_source(Environment::with_prefix("APP").separator("__"))
            .build()?;

        config.try_deserialize()
    }

    pub fn server_address(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }
}
