//! Redis 适配层：连接池、键命名空间与三类读写原语。
//!
//! 为什么把 Redis 相关代码集中在一个文件里：
//!
//! - 连接获取、超时、命令拼装和错误分类只实现一次，业务层只表达「要什么」，
//!   不会各自写出略有差异的 `EVAL` 或 `SET` 调用；
//! - 所有键统一加命名空间前缀。多个环境共用一个 Redis 实例时，前缀是唯一的
//!   隔离手段；手工拼 `format!` 的键迟早会出现遗漏，最终在一次 `FLUSHDB` 中
//!   同时清掉开发、测试与生产数据；
//! - 本模块只负责如实返回错误，**不决定降级策略**。哪些功能可以在 Redis 故障时
//!   继续工作（缓存、限流计数）由调用方根据自己的安全语义决定，适配层擅自
//!   重试、吞错或返回默认值会让行为变得不可推断。
//!
//! 本项目用到的三类数据都具备「可重建、可过期」的特征：
//!
//! | 用途 | 数据类型 | 过期策略 |
//! | --- | --- | --- |
//! | 登录失败计数 | ZSET（按失败时间打分） | 整个键带 TTL，读取时修剪窗口外成员 |
//! | access token 吊销列表 | STRING | TTL 等于令牌剩余有效期，到期即自动清理 |
//! | 用户资料缓存 | STRING（JSON） | 固定 TTL + 写操作主动删除 |

use std::sync::LazyLock;
use std::time::Duration;

use bb8::Pool;
use redis::{AsyncCommands, Client, RedisError, Script};
use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;

use crate::config::RedisConfig;

/// Redis 连接池。
///
/// 池中保存的是 `MultiplexedConnection`：redis-rs 的一条多路复用连接本身就能
/// 承载并发请求，因此这里的池**不是**「一个请求一条连接」，而是限制突发并发与
/// 连接重建次数。连接被复用时 bb8 会先执行一次 `PING`（见 [`RedisStore::from_config`]）。
pub type RedisPool = Pool<Client>;

/// Redis 适配层错误。
///
/// 区分「取连接失败」与「命令失败」：前者说明 Redis 不可达，通常应降级；
/// 后者说明命令本身有问题（例如键类型不匹配、脚本报错），属于需要修复的缺陷。
/// 把两者混成一个字符串会让排障时无法判断该重试还是该改代码。
#[derive(Debug)]
pub enum RedisStoreError {
    /// 连接地址无法解析。
    Config(String),
    /// 从连接池取得连接失败：Redis 不可达、连接超时或连接创建被拒绝。
    Connection(String),
    /// Redis 命令执行失败。
    Command(RedisError),
    /// 缓存值的序列化或反序列化失败。属于程序缺陷，不是 Redis 故障。
    Serde(serde_json::Error),
}

impl std::fmt::Display for RedisStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(message) => write!(formatter, "Redis 配置不合法：{message}"),
            // 连接错误只保留原始描述：这里不需要区分具体失败原因，
            // 调用方处理方式都是「降级并记录」。
            Self::Connection(message) => write!(formatter, "Redis 连接不可用：{message}"),
            Self::Command(error) => write!(formatter, "Redis 命令执行失败：{error}"),
            Self::Serde(error) => write!(formatter, "缓存值序列化失败：{error}"),
        }
    }
}

impl std::error::Error for RedisStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Command(error) => Some(error),
            Self::Serde(error) => Some(error),
            Self::Config(_) | Self::Connection(_) => None,
        }
    }
}

/// 统计滑动窗口内登录失败次数。
///
/// 用有序集合而不是简单的 `INCR` 计数器：`INCR` + `EXPIRE` 是固定窗口，
/// 窗口边界处计数会被整体清零，攻击者只要卡在边界两侧就能把允许次数翻倍。
/// 有序集合按失败时间打分，读取时先删除过期成员，得到的是真正的滚动窗口。
///
/// 读取与修剪放在同一个脚本里执行，保证「删除过期成员」与「统计数量」之间
/// 不被并发写入插入；单独发两条命令在高并发下会得到不自洽的计数。
///
/// `KEYS[1]` 是统计键，`ARGV[1]` 是当前时间（毫秒），`ARGV[2]` 是窗口长度（毫秒）。
static LOGIN_FAILURE_COUNT_SCRIPT: LazyLock<Script> = LazyLock::new(|| {
    Script::new(
        r#"
        local key = KEYS[1]
        local now = tonumber(ARGV[1])
        local window = tonumber(ARGV[2])
        redis.call('ZREMRANGEBYSCORE', key, '-inf', now - window)
        return redis.call('ZCARD', key)
        "#,
    )
});

/// 记录一次登录失败，并返回记录后窗口内的失败次数。
///
/// 一次脚本调用完成「修剪 + 写入 + 续期 + 统计」四件事，避免出现成员已写入
/// 但键没有 TTL、导致计数永久存在的中间状态。
///
/// `ARGV[3]` 是成员标识：同一毫秒内的多次失败必须具有不同的成员，
/// 否则 `ZADD` 只会更新已有成员的分数，计数会少记。
static LOGIN_FAILURE_RECORD_SCRIPT: LazyLock<Script> = LazyLock::new(|| {
    Script::new(
        r#"
        local key = KEYS[1]
        local now = tonumber(ARGV[1])
        local window = tonumber(ARGV[2])
        local member = ARGV[3]
        redis.call('ZREMRANGEBYSCORE', key, '-inf', now - window)
        redis.call('ZADD', key, now, member)
        redis.call('PEXPIRE', key, window)
        return redis.call('ZCARD', key)
        "#,
    )
});

/// Redis 访问器：连接池 + 键命名空间 + 本项目用到的读写原语。
///
/// 实现 `Clone`：内部连接池本身就是共享句柄，克隆成本与 `Arc` 相同，
/// 因此可以随 [`crate::app::state::AppState`] 一起被每个请求克隆。
#[derive(Clone)]
pub struct RedisStore {
    pool: RedisPool,
    /// 已规范化的键前缀（去掉了首尾空白与多余冒号）。
    key_prefix: String,
    /// 用户资料缓存的 TTL，来自配置，由调用方通过 [`Self::user_cache_ttl`] 读取。
    user_cache_ttl: Duration,
}

impl RedisStore {
    /// 根据配置创建访问器。
    ///
    /// 这里**不会**建立网络连接：`build_unchecked` 只创建连接池骨架，
    /// 连接在第一次取用时才建立。这样有两个好处：
    ///
    /// - 启动流程可以自行决定「连接失败时是告警还是退出」，而不被构造函数绑定；
    /// - 单元测试可以构造出完整的 `AppState`，不需要一个真实运行的 Redis。
    ///
    /// 保留 `test_on_check_out` 的默认值（首次使用前 `PING`）：redis-rs 的
    /// `has_broken` 恒返回 `false`，池无法自行发现失效连接，检出前的 `PING`
    /// 是唯一能让坏连接被剔除的机制。代价是每次取连接多一次往返，
    /// 对「一次请求 1~3 条 Redis 命令」的负载可以接受；若将来单请求命令数变多，
    /// 应改为在业务层合并命令，而不是关掉这项检查。
    ///
    /// 必须在 Tokio 运行时内调用：bb8 创建连接池时会启动后台回收任务，
    /// 没有运行时就会 panic。因此构造本类型的单元测试要用 `#[tokio::test]`。
    pub fn from_config(config: &RedisConfig) -> Result<Self, RedisStoreError> {
        let client = Client::open(config.url.clone())
            .map_err(|error| RedisStoreError::Config(error.to_string()))?;

        let pool = Pool::builder()
            .max_size(config.pool_size)
            .connection_timeout(Duration::from_secs(5))
            .build_unchecked(client);

        Ok(Self {
            pool,
            key_prefix: normalize_prefix(&config.key_prefix),
            user_cache_ttl: Duration::from_secs(config.user_cache_ttl_seconds.max(1) as u64),
        })
    }

    /// 用户资料缓存的存活时间。
    pub fn user_cache_ttl(&self) -> Duration {
        self.user_cache_ttl
    }

    /// 探测 Redis 是否可用。启动期与 `/ready` 就绪检查共用。
    pub async fn ping(&self) -> Result<(), RedisStoreError> {
        let mut connection = self.connection().await?;
        let reply: String = redis::cmd("PING")
            .query_async(&mut *connection)
            .await
            .map_err(RedisStoreError::Command)?;

        // 服务器可能返回 `PONG` 以外的内容（例如被代理接管），
        // 只有明确的 `PONG` 才代表这就是目标 Redis。
        if reply == "PONG" {
            Ok(())
        } else {
            Err(RedisStoreError::Connection(format!(
                "PING 返回了非预期内容：{reply}"
            )))
        }
    }

    /// 统计滑动窗口内的登录失败次数。
    pub async fn login_failure_count(
        &self,
        username: &str,
        window: Duration,
    ) -> Result<i64, RedisStoreError> {
        let key = self.login_failure_key(username);
        let mut connection = self.connection().await?;

        LOGIN_FAILURE_COUNT_SCRIPT
            .key(key)
            .arg(now_millis())
            .arg(window_millis(window))
            .invoke_async(&mut *connection)
            .await
            .map_err(RedisStoreError::Command)
    }

    /// 记录一次登录失败，返回记录后窗口内的失败次数。
    pub async fn record_login_failure(
        &self,
        username: &str,
        window: Duration,
    ) -> Result<i64, RedisStoreError> {
        let key = self.login_failure_key(username);
        // 成员用 UUIDv7：同一毫秒内的两次失败也不会互相覆盖。
        // 计数的精度依赖成员唯一，这里不能退化成「时间戳作为成员」。
        let member = Uuid::now_v7().to_string();
        let mut connection = self.connection().await?;

        LOGIN_FAILURE_RECORD_SCRIPT
            .key(key)
            .arg(now_millis())
            .arg(window_millis(window))
            .arg(member)
            .invoke_async(&mut *connection)
            .await
            .map_err(RedisStoreError::Command)
    }

    /// 把 access token 的 `jti` 写入吊销列表。
    ///
    /// TTL 使用「令牌剩余有效期」而不是固定值：令牌本身过期后，
    /// 吊销记录不再有任何作用，让它随令牌一起消失可以避免键无限累积。
    /// 用 `SET ... EX` 单条命令写入，避免 `SET` 成功、`EXPIRE` 失败时
    /// 留下一个永不过期的键。
    pub async fn revoke_access_token(
        &self,
        jti: &str,
        ttl: Duration,
    ) -> Result<(), RedisStoreError> {
        let key = self.revoked_access_key(jti);
        let mut connection = self.connection().await?;

        connection
            .set_ex::<_, _, ()>(key, "1", ttl.as_secs().max(1))
            .await
            .map_err(RedisStoreError::Command)
    }

    /// 查询 access token 是否已被吊销。
    pub async fn is_access_token_revoked(&self, jti: &str) -> Result<bool, RedisStoreError> {
        let key = self.revoked_access_key(jti);
        let mut connection = self.connection().await?;

        // 用 `EXISTS` 而不是 `GET`：吊销记录只关心「在不在」，
        // 读取内容没有任何意义，`EXISTS` 也不受值类型的影响。
        let exists: i64 = connection
            .exists(&key)
            .await
            .map_err(RedisStoreError::Command)?;

        Ok(exists > 0)
    }

    /// 读取 JSON 缓存；键不存在时返回 `None`。
    pub async fn get_json<T: DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<T>, RedisStoreError> {
        let mut connection = self.connection().await?;
        let raw: Option<String> = connection
            .get(key)
            .await
            .map_err(RedisStoreError::Command)?;

        match raw {
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(RedisStoreError::Serde),
            None => Ok(None),
        }
    }

    /// 写入 JSON 缓存，并设置过期时间。
    pub async fn set_json<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        ttl: Duration,
    ) -> Result<(), RedisStoreError> {
        let payload = serde_json::to_string(value).map_err(RedisStoreError::Serde)?;
        let mut connection = self.connection().await?;

        connection
            .set_ex::<_, _, ()>(key, payload, ttl.as_secs().max(1))
            .await
            .map_err(RedisStoreError::Command)
    }

    /// 删除一个缓存键。
    pub async fn delete(&self, key: &str) -> Result<(), RedisStoreError> {
        let mut connection = self.connection().await?;

        connection
            .del::<_, ()>(key)
            .await
            .map_err(RedisStoreError::Command)
    }

    /// 登录失败计数键。
    pub fn login_failure_key(&self, username: &str) -> String {
        self.namespaced(&format!("auth:login_failures:{username}"))
    }

    /// access token 吊销记录键。
    pub fn revoked_access_key(&self, jti: &str) -> String {
        self.namespaced(&format!("auth:revoked_access:{jti}"))
    }

    /// 用户资料缓存键。
    pub fn user_detail_key(&self, public_id: Uuid) -> String {
        self.namespaced(&format!("user:detail:{public_id}"))
    }

    /// 拼接命名空间前缀。
    fn namespaced(&self, key: &str) -> String {
        format!("{}:{key}", self.key_prefix)
    }

    /// 从连接池取得一条连接。
    async fn connection(&self) -> Result<bb8::PooledConnection<'_, Client>, RedisStoreError> {
        self.pool.get().await.map_err(|error| match error {
            // 连接创建失败本身已经带有可读的原因（例如连接被拒绝）。
            bb8::RunError::User(error) => RedisStoreError::Command(error),
            bb8::RunError::TimedOut => {
                RedisStoreError::Connection("等待连接池超时".to_owned())
            }
        })
    }
}

/// 规范化键前缀：去掉首尾空白和多余的冒号。
///
/// 允许用户把前缀写成 `axum:` 或 ` axum `，但最终拼接时只能出现一个冒号，
/// 否则键会变成 `axum::user:detail:...`，排查时很难看出哪一段是前缀。
fn normalize_prefix(prefix: &str) -> String {
    prefix.trim().trim_matches(':').to_owned()
}

/// 当前时间的毫秒时间戳。
fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 把时长转换为毫秒，并保证至少为 1：窗口为 0 会让所有成员立即过期，
/// 相当于关闭限流，属于配置错误而不是有效取值。
fn window_millis(window: Duration) -> i64 {
    window.as_millis().max(1) as i64
}

#[cfg(test)]
impl RedisStore {
    /// 构造一个不会建立任何网络连接的实例，供不依赖 Redis 的用例使用。
    pub fn without_connection() -> Self {
        Self::from_config(&RedisConfig {
            url: "redis://127.0.0.1:6379".to_owned(),
            pool_size: 2,
            key_prefix: "test".to_owned(),
            user_cache_ttl_seconds: 60,
        })
        .expect("测试用 Redis 配置必须合法")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(prefix: &str) -> RedisStore {
        RedisStore::from_config(&RedisConfig {
            url: "redis://127.0.0.1:6379".to_owned(),
            pool_size: 2,
            key_prefix: prefix.to_owned(),
            user_cache_ttl_seconds: 60,
        })
        .expect("测试用 Redis 配置必须合法")
    }

    /// 所有键都必须带命名空间：漏掉前缀会让不同环境的数据互相覆盖。
    #[tokio::test]
    async fn keys_are_namespaced() {
        let store = store("axum");
        let public_id = Uuid::now_v7();

        for key in [
            store.login_failure_key("alice"),
            store.revoked_access_key("jti-1"),
            store.user_detail_key(public_id),
        ] {
            assert!(key.starts_with("axum:"), "键必须以命名空间开头：{key}");
        }
    }

    /// 三类数据必须落在不同的键空间，避免「清缓存」误删限流计数。
    #[tokio::test]
    async fn key_categories_do_not_collide() {
        let store = store("axum");

        assert_eq!(store.login_failure_key("u1"), "axum:auth:login_failures:u1");
        assert_eq!(store.revoked_access_key("abc"), "axum:auth:revoked_access:abc");
        assert_ne!(store.login_failure_key("x"), store.revoked_access_key("x"));
    }

    /// 前缀首尾的空白与冒号必须被归一化，否则会出现 `axum::...` 这种键。
    #[tokio::test]
    async fn prefix_is_normalized() {
        for prefix in ["  axum: ", ":axum:", "axum"] {
            let store = store(prefix);

            assert_eq!(
                store.login_failure_key("u1"),
                "axum:auth:login_failures:u1",
                "前缀 {prefix:?} 应被规范化为 axum"
            );
        }
    }

    /// 不同前缀产生不同的键，保证多环境隔离真实生效。
    #[tokio::test]
    async fn different_prefixes_isolate_keys() {
        let dev = store("dev");
        let prod = store("prod");

        assert_ne!(dev.user_detail_key(Uuid::nil()), prod.user_detail_key(Uuid::nil()));
    }

    /// 缓存 TTL 来自配置，且不会因为配置为 0 变成「永不过期」。
    #[tokio::test]
    async fn cache_ttl_comes_from_config() {
        assert_eq!(store("axum").user_cache_ttl(), Duration::from_secs(60));
    }

    /// 连接错误与命令错误的文案必须可区分，便于排障时判断该重试还是该改代码。
    #[test]
    fn errors_describe_the_failure_kind() {
        let connection_error = RedisStoreError::Connection("连接被拒绝".to_owned());
        assert!(connection_error.to_string().contains("连接不可用"));

        let serde_error = RedisStoreError::Serde(
            serde_json::from_str::<u32>("\"not-a-number\"").expect_err("类型不匹配应失败"),
        );
        assert!(serde_error.to_string().contains("序列化"));
    }

    /// 连接地址写错时必须在创建阶段就报错，而不是等到第一次命令失败。
    #[test]
    fn rejects_unparsable_url() {
        let result = RedisStore::from_config(&RedisConfig {
            url: "not-a-redis-url".to_owned(),
            pool_size: 2,
            key_prefix: "axum".to_owned(),
            user_cache_ttl_seconds: 60,
        });

        match result {
            Err(RedisStoreError::Config(message)) => {
                assert!(!message.is_empty(), "配置错误应携带原始原因");
            }
            Err(other) => panic!("非法地址应返回配置错误，实际为 {other:?}"),
            Ok(_) => panic!("非法地址不应创建出可用的访问器"),
        }
    }
}
