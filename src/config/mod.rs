use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use std::env;

/// 应用配置结构体, 包含服务器配置、数据库配置和日志配置，与config/下的配置文件中的结构对应
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub log: LogConfig,
    pub redis: RedisConfig,
    pub jwt: JwtConfig,
    pub auth: AuthConfig,
    pub bootstrap: BootstrapConfig,
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

/// Redis 配置。
///
/// Redis 在本项目中承担三类「读写频繁、可以重建」的状态：
/// 登录失败计数、access token 吊销列表、用户资料缓存。
/// 它们都有一个共同点：丢失后可以接受，因此 Redis 停机时各项功能会降级
///（详见 `docs/REDIS_GUIDE.md`），而数据库中的权威数据不受影响。
#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    /// 连接地址，例如 `redis://127.0.0.1:6379` 或带密码的
    /// `redis://:password@host:6379/0`。
    pub url: String,
    /// 连接池上限。Redis 单条连接即可复用，该值主要限制突发并发，
    /// 不宜按「每个请求一条连接」来设置。
    pub pool_size: u32,
    /// 键命名空间前缀。
    ///
    /// 多个环境或多个服务共用一个 Redis 实例时，前缀是唯一的隔离手段；
    /// 务必区分开发、测试与生产，否则一次 `FLUSHDB` 会同时清掉三者。
    pub key_prefix: String,
    /// 用户详情缓存的存活时间（秒）。
    ///
    /// TTL 是缓存一致性的最后一道兜底：即使某次更新漏了主动失效，
    /// 陈旧数据最多存在这么久。
    pub user_cache_ttl_seconds: i64,
}

/// JWT 签发与校验配置。
///
/// `secret` 必须通过环境变量注入生产值，配置文件中的默认值仅用于本地开发；
/// 启动时会校验强度，避免弱密钥导致令牌可被离线伪造。
#[derive(Debug, Clone, Deserialize)]
pub struct JwtConfig {
    pub secret: String,
    pub issuer: String,
    /// access token 有效期（分钟）。取值较短，因为无状态令牌无法单独吊销。
    pub access_ttl_minutes: i64,
    /// refresh token 有效期（天）。
    pub refresh_ttl_days: i64,
}

/// 登录保护配置。
#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    /// 滑动窗口内允许的最大失败次数。
    pub max_login_failures: i64,
    /// 失败次数统计窗口（分钟）。
    pub login_window_minutes: i64,
    /// 令牌 Cookie 配置。
    pub cookie: CookieConfig,
}

/// 令牌 Cookie 配置。
///
/// 令牌通过响应头下发而不是放在响应体中：`HttpOnly` 使页面脚本无法读取令牌，
/// 从而把 XSS 的影响限制在"攻击者可以代为发请求"，而无法直接窃取令牌长期冒用。
#[derive(Debug, Clone, Deserialize)]
pub struct CookieConfig {
    /// access token 的 Cookie 名称。
    pub access_name: String,
    /// refresh token 的 Cookie 名称。
    pub refresh_name: String,
    /// 是否仅通过 HTTPS 传输。
    ///
    /// 生产环境必须为 `true`，否则令牌会在明文 HTTP 中被中间人截获。
    pub secure: bool,
    /// SameSite 属性：`strict` 或 `lax`。
    ///
    /// 默认 `strict`，禁止浏览器在跨站请求中携带令牌 Cookie，
    /// 这是本方案防 CSRF 的主要手段。
    pub same_site: String,
    /// Cookie 作用路径。
    pub path: String,
}

impl CookieConfig {
    /// 返回规范化后的 SameSite 属性值。
    ///
    /// 取值不合法时回退到最严格的 `Strict`，避免配置写错导致防护被静默放宽。
    pub fn same_site_attribute(&self) -> &'static str {
        if self.same_site.eq_ignore_ascii_case("lax") {
            "Lax"
        } else {
            "Strict"
        }
    }
}

/// 引导账号配置。
///
/// 用途是在数据库还没有任何用户时创建首个可登录账号，解开「登录需要用户、
/// 建用户需要登录」的死锁。默认关闭，且只有 `users` 表为空时才会生效，
/// 因此它不会成为绕过认证的长期建号通道。
///
/// 口令属于凭据，必须通过环境变量或未提交的 `config/local.toml` 注入，
/// 不应写进 `config/default.toml` 这类会提交的文件。
#[derive(Debug, Clone, Deserialize)]
pub struct BootstrapConfig {
    /// 是否启用引导账号创建。
    pub enabled: bool,
    /// 引导账号用户名。
    pub username: String,
    /// 引导账号邮箱。
    pub email: String,
    /// 引导账号明文口令，仅在启动期用于计算 Argon2 哈希，不会被持久化。
    pub password: String,
    /// 引导账号昵称，可选。
    ///
    /// 显式 `default`：配置文件省略该字段时按 `None` 处理，而不是反序列化失败。
    #[serde(default)]
    pub nickname: Option<String>,
}

/// JWT 密钥的最小长度（字节）。
///
/// HS256 的安全性完全依赖密钥熵，低于该长度的密钥存在被离线爆破的风险。
const MIN_JWT_SECRET_LENGTH: usize = 32;

impl AppConfig {
    pub fn new() -> Result<Self, ConfigError> {
        dotenvy::dotenv().ok(); // 加载 .env 文件（如果存在）
        let run_mode: String = env::var("RUN_MODE").unwrap_or_else(|_| "development".into()); // 获取运行环境，默认为 development

        let config: Config = Config::builder()
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

    /// 校验配置中不能等到运行期才暴露的问题。
    ///
    /// 在启动阶段就拒绝弱密钥，而不是打印一条告警继续运行：一旦以弱密钥上线，
    /// 攻击者可以离线伪造任意用户的令牌，运行期再发现已经太晚。
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.jwt.secret.len() < MIN_JWT_SECRET_LENGTH {
            return Err(ConfigError::Message(format!(
                "JWT 密钥长度不得少于 {MIN_JWT_SECRET_LENGTH} 个字符"
            )));
        }
        if self.jwt.access_ttl_minutes <= 0 || self.jwt.refresh_ttl_days <= 0 {
            return Err(ConfigError::Message("JWT 有效期必须为正数".to_owned()));
        }
        if self.auth.max_login_failures <= 0 || self.auth.login_window_minutes <= 0 {
            return Err(ConfigError::Message(
                "登录保护的失败次数与统计窗口必须为正数".to_owned(),
            ));
        }
        if self.auth.cookie.access_name.trim().is_empty()
            || self.auth.cookie.refresh_name.trim().is_empty()
        {
            return Err(ConfigError::Message("Cookie 名称不能为空".to_owned()));
        }
        if self.auth.cookie.access_name == self.auth.cookie.refresh_name {
            return Err(ConfigError::Message(
                "access token 与 refresh token 的 Cookie 名称不能相同".to_owned(),
            ));
        }
        // Redis 是可选加速组件，但一旦配置了就应当是可用配置：
        // 地址写成拼写错误时，运行期所有 Redis 功能都会静默降级为「未启用」，
        // 排障时极难联想到是配置写错。这里在启动阶段直接拒绝明显非法的取值。
        if self.redis.url.trim().is_empty()
            || !(self.redis.url.starts_with("redis://") || self.redis.url.starts_with("rediss://"))
        {
            return Err(ConfigError::Message(
                "redis.url 必须以 redis:// 或 rediss:// 开头".to_owned(),
            ));
        }
        if self.redis.pool_size == 0 {
            return Err(ConfigError::Message(
                "redis.pool_size 必须大于 0".to_owned(),
            ));
        }
        if self.redis.key_prefix.trim().is_empty() {
            return Err(ConfigError::Message(
                "redis.key_prefix 不能为空：缺少命名空间会让不同环境的键互相覆盖".to_owned(),
            ));
        }
        if self.redis.user_cache_ttl_seconds <= 0 {
            return Err(ConfigError::Message(
                "redis.user_cache_ttl_seconds 必须为正数".to_owned(),
            ));
        }
        // 启用引导账号却缺少必要字段，会在服务已经启动之后才暴露为「无人能登录」。
        // 这里只做非空检查，长度、邮箱格式等细则由创建用户请求模型的校验统一负责，
        // 避免同一套规则在配置层和第二份实现中逐渐分叉。
        if self.bootstrap.enabled {
            for (key, value) in [
                ("bootstrap.username", &self.bootstrap.username),
                ("bootstrap.email", &self.bootstrap.email),
                ("bootstrap.password", &self.bootstrap.password),
            ] {
                if value.trim().is_empty() {
                    return Err(ConfigError::Message(format!("启用引导账号时 {key} 不能为空")));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一份合法配置，供各用例按需覆盖单个字段。
    fn valid_config() -> AppConfig {
        AppConfig {
            server: ServerConfig {
                host: "127.0.0.1".to_owned(),
                port: 3000,
            },
            database: DatabaseConfig {
                url: "postgresql://localhost:5432/postgres".to_owned(),
                max_connections: 5,
            },
            log: LogConfig {
                level: "info".to_owned(),
            },
            redis: RedisConfig {
                url: "redis://127.0.0.1:6379".to_owned(),
                pool_size: 8,
                key_prefix: "axum-test".to_owned(),
                user_cache_ttl_seconds: 60,
            },
            jwt: JwtConfig {
                secret: "a".repeat(MIN_JWT_SECRET_LENGTH),
                issuer: "rust-axum-server".to_owned(),
                access_ttl_minutes: 15,
                refresh_ttl_days: 7,
            },
            auth: AuthConfig {
                max_login_failures: 5,
                login_window_minutes: 15,
                cookie: CookieConfig {
                    access_name: "access_token".to_owned(),
                    refresh_name: "refresh_token".to_owned(),
                    secure: false,
                    same_site: "strict".to_owned(),
                    path: "/".to_owned(),
                },
            },
            bootstrap: BootstrapConfig {
                enabled: false,
                username: "admin".to_owned(),
                email: "admin@example.com".to_owned(),
                password: String::new(),
                nickname: None,
            },
        }
    }

    #[test]
    fn accepts_valid_config() {
        valid_config().validate().expect("合法配置应通过校验");
    }

    #[test]
    fn rejects_short_jwt_secret() {
        let mut config = valid_config();
        config.jwt.secret = "too-short".to_owned();

        let error = config.validate().expect_err("弱密钥应被拒绝");
        assert!(error.to_string().contains("JWT 密钥长度"));
    }

    #[test]
    fn rejects_non_positive_ttl() {
        let mut config = valid_config();
        config.jwt.access_ttl_minutes = 0;

        assert!(config.validate().is_err(), "非正数有效期应被拒绝");
    }

    #[test]
    fn rejects_non_positive_login_protection() {
        let mut config = valid_config();
        config.auth.max_login_failures = 0;

        assert!(config.validate().is_err(), "非正数失败次数应被拒绝");
    }

    #[test]
    fn rejects_identical_cookie_names() {
        let mut config = valid_config();
        config.auth.cookie.refresh_name = config.auth.cookie.access_name.clone();

        assert!(
            config.validate().is_err(),
            "两个令牌共用同一 Cookie 名称会互相覆盖，必须被拒绝"
        );
    }

    #[test]
    fn rejects_blank_cookie_name() {
        let mut config = valid_config();
        config.auth.cookie.access_name = "  ".to_owned();

        assert!(config.validate().is_err(), "空 Cookie 名称应被拒绝");
    }

    /// Redis 地址写错时运行期只会表现为「各项 Redis 功能静默降级」，
    /// 因此在启动阶段就拒绝非法协议，避免把配置问题拖到排障阶段。
    #[test]
    fn rejects_invalid_redis_url() {
        let mut config = valid_config();
        config.redis.url = "http://127.0.0.1:6379".to_owned();

        let error = config.validate().expect_err("非 Redis 协议应被拒绝");
        assert!(error.to_string().contains("redis.url"));
    }

    /// 空前缀会让不同环境共用同名键，因此必须拒绝。
    #[test]
    fn rejects_blank_redis_key_prefix() {
        let mut config = valid_config();
        config.redis.key_prefix = "   ".to_owned();

        assert!(config.validate().is_err(), "空键前缀应被拒绝");
    }

    #[test]
    fn rejects_zero_redis_pool_size() {
        let mut config = valid_config();
        config.redis.pool_size = 0;

        assert!(config.validate().is_err(), "连接池上限为 0 应被拒绝");
    }

    /// TTL 是缓存一致性的兜底，非正数会让缓存永不过期。
    #[test]
    fn rejects_non_positive_user_cache_ttl() {
        let mut config = valid_config();
        config.redis.user_cache_ttl_seconds = 0;

        assert!(config.validate().is_err(), "非正数缓存 TTL 应被拒绝");
    }

    /// SameSite 配置写错时必须回退到最严格的取值，
    /// 而不是静默放宽为无限制。
    #[test]
    fn falls_back_to_strict_same_site() {
        let mut config = valid_config();

        config.auth.cookie.same_site = "lax".to_owned();
        assert_eq!(config.auth.cookie.same_site_attribute(), "Lax");

        config.auth.cookie.same_site = "None".to_owned();
        assert_eq!(
            config.auth.cookie.same_site_attribute(),
            "Strict",
            "未知取值应回退到 Strict"
        );
    }

    /// 引导账号默认必须关闭：默认值一旦变成 `true`，任何一次误部署都会
    /// 在空库上自动建出账号。
    #[test]
    fn bootstrap_is_disabled_by_default() {
        assert!(!valid_config().bootstrap.enabled);
    }

    /// 启用引导账号但缺少口令时必须在启动阶段失败，
    /// 而不是先启动服务、再让所有接口返回 401。
    #[test]
    fn rejects_bootstrap_without_password() {
        let mut config = valid_config();
        config.bootstrap.enabled = true;
        config.bootstrap.password = "   ".to_owned();

        let error = config.validate().expect_err("启用引导账号但口令为空应被拒绝");
        assert!(error.to_string().contains("bootstrap.password"));
    }

    /// 启用引导账号且三个字段齐备时应通过校验。
    #[test]
    fn accepts_complete_bootstrap_config() {
        let mut config = valid_config();
        config.bootstrap.enabled = true;
        config.bootstrap.password = "bootstrap-password".to_owned();

        config.validate().expect("完整引导配置应通过校验");
    }
}
