use argon2::{Argon2, PasswordHash, PasswordVerifier};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

use crate::common::error::AppError;
use crate::config::{AuthConfig, JwtConfig};
use crate::infrastructure::redis::RedisStore;
use crate::modules::auth::model::{CurrentUserResponse, TokenPair};
use crate::modules::auth::repository::AuthRepository;
use crate::modules::user::repository::UserRepository;

/// access token 的令牌类型标识。
const TOKEN_TYPE_ACCESS: &str = "access";
/// 刷新令牌的随机字节数。
///
/// 256 位随机量使刷新令牌无法被枚举或猜测，这是它唯一的防伪手段——
/// 与 access token 不同，刷新令牌不携带签名。
const REFRESH_TOKEN_BYTES: usize = 32;

/// 认证业务逻辑层。
///
/// 依赖配置由调用方以参数传入而不是从全局读取，使该层保持可测试，
/// 也避免在业务代码中散落配置访问。
pub struct AuthService;

/// 校验通过的 access token 中与授权、吊销有关的信息。
///
/// 中间件除了用户身份之外还需要 `jti`（吊销列表的键），因此不再只返回用户主键。
#[derive(Debug, Clone)]
pub struct VerifiedAccessToken {
    /// 用户内部主键，取自 `sub`。
    pub user_id: i64,
    /// 令牌唯一标识，作为吊销记录的键。
    pub jti: String,
}

/// access token 负载。
///
/// 字段名使用 JWT 注册声明的短名（`sub`/`iss`/`exp`/`iat`/`jti`），
/// 其余为私有声明。
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct AccessClaims {
    /// 内部自增主键。不放 `public_id` 是为了让受保护接口可以直接按主键
    /// 命中主键索引加载用户，避免额外一次公开 ID 到内部 ID 的转换查询。
    sub: String,
    iss: String,
    exp: i64,
    iat: i64,
    /// 令牌唯一标识，便于把日志与单次签发对应起来。
    jti: String,
    /// 令牌类型。
    ///
    /// 该字段是必需的安全约束：缺少它时，长效刷新令牌可以被直接当作
    /// access token 使用，从而绕过"access token 短期有效"的整个设计。
    typ: String,
}

/// 登录请求的客户端信息，仅用于安全审计。
///
/// 单独成结构体而不是继续增加参数：登录需要数据库、Redis、两份配置和凭证，
/// 参数过多时调用点很容易把 `client_ip` 与 `user_agent` 传反，
/// 而两者一旦互换不会编译失败，只会把审计数据写成垃圾。
#[derive(Debug, Clone, Copy, Default)]
pub struct LoginContext<'a> {
    pub client_ip: Option<&'a str>,
    pub user_agent: Option<&'a str>,
}

impl AuthService {
    /// 用户登录：核验凭证并签发令牌对。
    ///
    /// `context` 中的客户端信息仅用于安全审计记录，不参与认证判定。
    pub async fn login(
        db: &PgPool,
        redis: &RedisStore,
        jwt_config: &JwtConfig,
        auth_config: &AuthConfig,
        username: &str,
        password: &str,
        context: LoginContext<'_>,
    ) -> Result<TokenPair, AppError> {
        let now = Utc::now();
        let window = rate_limit_window(auth_config);
        let window_start = now - ChronoDuration::minutes(auth_config.login_window_minutes);
        // 先判定限流，再执行密码校验。顺序不能颠倒：Argon2 校验被刻意设计得很慢
        // 若先校验密码，大量失败请求会持续占用阻塞线程池，使限流本身变成
        // 消耗服务端算力的攻击面。
        let recent_failures = Self::count_login_failures(db, redis, username, window, window_start).await?;
        if recent_failures >= auth_config.max_login_failures {
            tracing::warn!(%username, recent_failures, "登录失败次数超过阈值，暂时拒绝登录");
            return Err(AppError::TooManyRequests("登录失败次数过多，请稍后再试"));
        }
        let credentials = UserRepository::find_credentials_by_username(db, username).await?;
        // 无论用户是否存在都要执行一次 Argon2 校验。若用户不存在时直接返回，
        // 响应耗时会明显短于"用户存在但密码错误"，攻击者可据此枚举有效用户名。
        let Some(credentials) = credentials else {
            let _ = verify_password_dummy(password).await;
            Self::record_login_failure(db, redis, username, context.client_ip, window).await?;
            return Err(invalid_credentials());
        };
        // Argon2 是 CPU 密集型操作，必须放入阻塞线程池，否则会占满
        // Tokio 工作线程并拖慢所有并发请求。
        let password_valid = verify_password(password, &credentials.password_hash).await?;
        if !password_valid {
            Self::record_login_failure(db, redis, username, context.client_ip, window).await?;
            return Err(invalid_credentials());
        }
        // 登录成功即视为已恢复访问，清理窗口外的历史失败记录。
        // 顺带清理避免为此单独引入定时任务，窗口外记录已不参与判定。
        //
        // 注意这里只清理数据库中的过期审计记录，不清空 Redis 中的窗口内计数：
        // 成功登录不能证明之前的失败不是攻击行为，清空计数等于给攻击者一个
        // 「只要偶尔成功一次就能重置限额」的入口。窗口外的成员会在下次读取时
        // 被脚本修剪，键本身也带 TTL，不需要额外维护。
        AuthRepository::clear_failures_before(db, username, window_start).await?;
        Self::issue_token_pair(db, jwt_config, credentials.id, credentials.public_id, context.client_ip, context.user_agent).await
    }

    /// 统计窗口内的登录失败次数。
    ///
    /// Redis 是判定用的加速层，数据库是权威审计来源：Redis 不可用时回退为
    /// 直接统计 `login_failures` 表。这条回退路径保证 Redis 故障不会让登录
    /// 保护整体失效，代价只是这一次判定慢一点。
    async fn count_login_failures(db: &PgPool, redis: &RedisStore, username: &str, window: Duration, window_start: DateTime<Utc>) -> Result<i64, AppError> {
        match redis.login_failure_count(username, window).await {
            Ok(count) => Ok(count),
            Err(error) => {
                tracing::warn!(%error, %username, "Redis 登录失败计数不可用，回退到数据库统计");
                Ok(AuthRepository::count_failures_since(db, username, window_start).await?)
            }
        }
    }

    /// 记录一次登录失败。
    ///
    /// 先写数据库审计，再写 Redis 计数。Redis 写入失败只告警：审计记录已经落库，
    /// 后续判定会自动回退到数据库统计，因此本次请求不需要以失败告终。
    async fn record_login_failure(db: &PgPool, redis: &RedisStore, username: &str, client_ip: Option<&str>, window: Duration) -> Result<(), AppError> {
        AuthRepository::record_failure(db, username, client_ip).await?;
        if let Err(error) = redis.record_login_failure(username, window).await {
            tracing::warn!(
                %error,
                %username,
                "Redis 登录失败计数写入失败，后续判定将回退到数据库统计"
            );
        }
        Ok(())
    }

    /// 使用刷新令牌换取新的令牌对。
    ///
    /// 采用轮换策略：每次刷新都签发全新的刷新令牌并吊销旧的。
    pub async fn refresh(
        db: &PgPool,
        jwt_config: &JwtConfig,
        refresh_token: &str,
        client_ip: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<TokenPair, AppError> {
        let token_hash = hash_token(refresh_token);
        let session = AuthRepository::find_session_with_user(db, &token_hash)
            .await?
            .ok_or_else(|| AppError::Unauthorized("刷新令牌无效"))?;
        // 已吊销的刷新令牌再次被使用，说明令牌可能已泄露并被重放：
        // 合法客户端在轮换后不会再用旧令牌。此时无法分辨谁是攻击者，
        // 只能吊销该用户全部会话，让双方都重新登录。
        if session.is_revoked {
            tracing::warn!(user_id = session.user_id, "检测到已吊销的刷新令牌被再次使用，强制该用户全部会话下线");
            AuthRepository::revoke_all_sessions_for_user(db, session.user_id).await?;
            return Err(AppError::Unauthorized("刷新令牌已失效，请重新登录"));
        }
        if session.expires_at <= Utc::now() {
            return Err(AppError::Unauthorized("刷新令牌已过期，请重新登录"));
        }
        let token_pair = Self::issue_token_pair(db, jwt_config, session.user_id, session.public_id, client_ip, user_agent).await?;
        // 新会话已经落库，再把旧会话标记为已吊销并记录轮换关系。
        let new_session_id = AuthRepository::find_session_with_user(db, &hash_token(&token_pair.refresh_token))
            .await?
            .ok_or(AppError::Internal("令牌会话写入后无法读取"))?
            .id;
        AuthRepository::rotate_session(db, session.id, new_session_id).await?;
        Ok(token_pair)
    }

    /// 登出：吊销刷新令牌对应的会话。
    ///
    /// 操作设计为幂等——已吊销或已过期的令牌同样返回成功。
    /// 客户端在网络抖动后重试登出不应收到 401，否则会陷入无法退出的状态。
    pub async fn logout(db: &PgPool, refresh_token: &str) -> Result<(), AppError> {
        let token_hash = hash_token(refresh_token);
        let Some(session) = AuthRepository::find_session_by_token_hash(db, &token_hash).await? else {
            // 令牌无法对应任何会话，说明它从未被签发或已被清理。
            // 从调用方视角登出目标已达成，因此同样返回成功而不是报错。
            return Ok(());
        };
        AuthRepository::revoke_session(db, session.id).await?;
        Ok(())
    }

    /// 把 access token 写入吊销列表，使它在自然过期前立即失效。
    ///
    /// 换入 Redis 之后，「登出立即失效 access token」才真正成立：
    /// 在此之前只能等令牌自然过期（`access_ttl_minutes`），因为服务端不保存
    /// 任何 access token 状态。吊销记录只需要「令牌唯一标识 + 剩余有效期」，
    /// 用一条带 TTL 的字符串即可表达，令牌过期后记录也会自动消失。
    ///
    /// 无法解析或已过期的令牌直接返回成功：这类令牌本来就通不过认证中间件，
    /// 写入一条永远不会被查询的记录没有意义。
    pub async fn revoke_access_token(redis: &RedisStore, jwt_config: &JwtConfig, token: &str) -> Result<(), AppError> {
        let Ok(claims) = decode_access_claims(jwt_config, token) else {
            return Ok(());
        };
        let remaining = claims.exp - Utc::now().timestamp();
        if remaining <= 0 {
            return Ok(());
        }
        redis
            .revoke_access_token(&claims.jti, Duration::from_secs(remaining as u64))
            .await
            .map_err(|error| {
                tracing::warn!(%error, "写入 access token 吊销列表失败");
                AppError::Internal("令牌吊销失败")
            })
    }

    /// 解析 access token，返回用户身份、吊销标识与到期时间。
    pub fn verify_access_token(jwt_config: &JwtConfig, token: &str) -> Result<VerifiedAccessToken, AppError> {
        let claims = decode_access_claims(jwt_config, token)?;
        let user_id = claims.sub.parse::<i64>().map_err(|_| AppError::Unauthorized("认证令牌无效"))?;
        Ok(VerifiedAccessToken { user_id, jti: claims.jti })
    }

    /// 签发一个 access token，仅供测试构造已登录请求使用。
    ///
    /// 存在的原因：认证中间件挂在 `/api/v1` 整组路由上，任何端到端测试都必须
    /// 先构造合法令牌。测试若自行拼装 JWT，就会与生产签发逻辑分叉——真实实现
    /// 改变声明后测试仍会通过，覆盖价值随之消失。因此这里复用同一个
    /// [`AccessClaims`] 结构，只把有效期交给调用方指定。
    ///
    /// 使用 `#[doc(hidden)]` 而不是 `#[cfg(test)]`：`cfg(test)` 只在编译 crate
    /// 自身的测试目标时生效，`tests/` 下的集成测试属于独立 crate，看不到该函数。
    /// 生产构建中它不会被任何代码调用，因此显式允许 `dead_code`，
    /// 而不是为它引入一个只为测试存在的 feature 开关。
    ///
    /// `expires_at` 是 `exp` 声明的绝对值（Unix 秒），测试可传一个极大的取值
    /// 得到「长期有效」的令牌；`iat` 固定为 0，避免用例结果受当前时间影响。
    #[doc(hidden)]
    #[allow(dead_code)]
    pub fn issue_test_access_token(jwt_config: &JwtConfig, user_id: i64, expires_at: i64) -> String {
        let claims = AccessClaims {
            sub: user_id.to_string(),
            iss: jwt_config.issuer.clone(),
            exp: expires_at,
            iat: 0,
            jti: format!("test-{user_id}"),
            typ: TOKEN_TYPE_ACCESS.to_owned(),
        };

        encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(jwt_config.secret.as_bytes())).expect("测试用 access token 应签发成功")
    }

    /// 加载当前登录用户的公开资料。
    pub async fn current_user(db: &PgPool, user_id: i64) -> Result<CurrentUserResponse, AppError> {
        let user = UserRepository::find_by_id(db, user_id)
            .await?
            .ok_or_else(|| AppError::Unauthorized("认证令牌对应的用户不存在"))?;

        Ok(CurrentUserResponse {
            public_id: user.public_id,
            username: user.username,
            email: user.email,
            nickname: user.nickname,
        })
    }

    /// 生成令牌对并将会话写入数据库。
    async fn issue_token_pair(
        db: &PgPool,
        jwt_config: &JwtConfig,
        user_id: i64,
        public_id: Uuid,
        client_ip: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<TokenPair, AppError> {
        let now = Utc::now();
        let access_expires_in = ChronoDuration::minutes(jwt_config.access_ttl_minutes);
        let refresh_expires_at = now + ChronoDuration::days(jwt_config.refresh_ttl_days);

        let access_token = encode_access_token(jwt_config, user_id, public_id, now)?;

        // 刷新令牌使用独立的高熵随机值而不是 JWT：它只需要"不可猜测"，
        // 不需要被解析，把状态完全放在数据库里反而更容易做到即时吊销。
        let refresh_token = generate_refresh_token();
        let refresh_token_hash = hash_token(&refresh_token);

        // 只把摘要写入数据库：即使数据库泄露，攻击者也无法用摘要反推出
        // 可用于认证的原始刷新令牌。
        AuthRepository::create_session(db, user_id, &refresh_token_hash, refresh_expires_at, client_ip, user_agent).await?;

        Ok(TokenPair::new(access_token, refresh_token, access_expires_in.num_seconds()))
    }
}

/// 构造统一的凭证错误。
///
/// 用户名不存在与密码错误必须返回完全一致的响应，否则攻击者可以据此
/// 枚举出系统中真实存在的用户名。
fn invalid_credentials() -> AppError {
    AppError::Unauthorized("用户名或密码错误")
}

/// 把配置中的分钟窗口转换为 `Duration`。
///
/// 至少保证一分钟：零长度窗口会让 Redis 中所有失败记录立即过期，
/// 相当于关闭限流。配置校验已经拒绝非正数，这里只是防线内的兜底。
fn rate_limit_window(auth_config: &AuthConfig) -> Duration {
    Duration::from_secs((auth_config.login_window_minutes * 60).max(60) as u64)
}

/// 签发 access token。
fn encode_access_token(jwt_config: &JwtConfig, user_id: i64, public_id: Uuid, now: DateTime<Utc>) -> Result<String, AppError> {
    // jti 使用 public_id 与签发时间组合，保证同一用户在同一秒内多次登录
    // 也能得到不同取值，便于日志区分单次签发。
    let claims = AccessClaims {
        sub: user_id.to_string(),
        iss: jwt_config.issuer.clone(),
        exp: (now + ChronoDuration::minutes(jwt_config.access_ttl_minutes)).timestamp(),
        iat: now.timestamp(),
        jti: format!("{public_id}:{}", now.timestamp_millis()),
        typ: TOKEN_TYPE_ACCESS.to_owned(),
    };
    encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(jwt_config.secret.as_bytes())).map_err(|error| {
        tracing::error!(%error, "access token 签发失败");
        AppError::Internal("令牌签发失败")
    })
}

/// 校验并解析 access token。
fn decode_access_claims(jwt_config: &JwtConfig, token: &str) -> Result<AccessClaims, AppError> {
    // 显式指定 HS256，绝不从令牌头部读取算法。若信任头部声明的 alg，
    // 攻击者可将算法改为 none 或把非对称公钥当作 HMAC 密钥，从而伪造任意令牌。
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&[jwt_config.issuer.as_str()]);
    validation.set_required_spec_claims(&["exp", "iss", "sub"]);
    validation.validate_aud = false;
    // 允许少量时钟偏移，避免多实例部署时因时钟不同步导致令牌被误判为过期。
    validation.leeway = 5;
    let token_data =
        decode::<AccessClaims>(token, &DecodingKey::from_secret(jwt_config.secret.as_bytes()), &validation).map_err(|error| match error.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => AppError::Unauthorized("认证令牌已过期"),
            other => {
                tracing::debug!(?other, "access token 校验失败");
                AppError::Unauthorized("认证令牌无效")
            }
        })?;
    // 断言类型，阻止长效刷新令牌被当作 access token 使用。
    if token_data.claims.typ != TOKEN_TYPE_ACCESS {
        return Err(AppError::Unauthorized("认证令牌类型错误"));
    }
    Ok(token_data.claims)
}

/// 生成高熵刷新令牌。
fn generate_refresh_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; REFRESH_TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    to_hex(&bytes)
}

/// 计算刷新令牌摘要。
///
/// 使用 SHA-256 而不是 Argon2：刷新令牌本身已是 256 位随机值，不存在
/// 字典攻击空间；摘要的作用是防止数据库泄露后令牌被直接复用，
/// 而 Argon2 的慢速特性会在每次刷新时带来不必要的延迟。
fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    to_hex(&digest)
}

/// 将字节序列编码为小写十六进制字符串。
fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

/// 在阻塞线程池中校验密码。
async fn verify_password(password: &str, encoded_hash: &str) -> Result<bool, AppError> {
    let password = password.to_owned();
    let encoded_hash = encoded_hash.to_owned();
    tokio::task::spawn_blocking(move || verify_password_blocking(&password, &encoded_hash))
        .await
        .map_err(|_| AppError::Internal("密码校验任务失败"))?
}
/// 同步校验密码，返回是否匹配。
fn verify_password_blocking(password: &str, encoded_hash: &str) -> Result<bool, AppError> {
    let parsed_hash = PasswordHash::new(encoded_hash).map_err(|_| AppError::Internal("密码哈希格式非法"))?;
    Ok(Argon2::default().verify_password(password.as_bytes(), &parsed_hash).is_ok())
}

/// 对不存在的用户执行等价的密码校验，用于抹平响应时间差异。
///
/// 这里必须真正执行 Argon2 计算，而不是简单地返回或 `sleep`：
/// 只有实际的计算量相同，耗时才能与"用户存在但密码错误"的分支对齐。
async fn verify_password_dummy(password: &str) -> Result<(), AppError> {
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || {
        // 对比一个固定的合法哈希，校验结果本身没有意义，只取其计算耗时。
        let _ = verify_password_blocking(&password, DUMMY_PASSWORD_HASH);
    })
    .await
    .map_err(|_| AppError::Internal("密码校验任务失败"))?;
    Ok(())
}

/// 静态密码哈希，仅用于时间攻击防护。
///
/// 取值固定，但必须是格式合法的 Argon2 输出，否则会在校验前提前失败，
/// 反而让"用户不存在"分支更快。配套测试保证它始终可被解析。
const DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::password::hash_password;

    fn test_jwt_config() -> JwtConfig {
        JwtConfig {
            secret: "unit-test-secret-with-enough-length-32".to_owned(),
            issuer: "rust-axum-server-test".to_owned(),
            access_ttl_minutes: 15,
            refresh_ttl_days: 7,
        }
    }

    #[test]
    fn encodes_and_decodes_access_token() {
        let config = test_jwt_config();
        let now = Utc::now();
        let token = encode_access_token(&config, 42, Uuid::now_v7(), now).expect("access token 应签发成功");

        let verified = AuthService::verify_access_token(&config, &token).expect("刚签发的令牌应通过校验");

        assert_eq!(verified.user_id, 42);
        assert!(!verified.jti.is_empty(), "jti 用作吊销键，不能为空");
    }

    #[test]
    fn rejects_token_signed_with_another_secret() {
        let config = test_jwt_config();
        let token = encode_access_token(&config, 1, Uuid::now_v7(), Utc::now()).expect("access token 应签发成功");

        let mut other_config = test_jwt_config();
        other_config.secret = "another-unit-test-secret-32-characters".to_owned();

        assert!(
            AuthService::verify_access_token(&other_config, &token).is_err(),
            "使用不同密钥签发的令牌必须被拒绝"
        );
    }

    #[test]
    fn rejects_token_with_another_issuer() {
        let config = test_jwt_config();
        let token = encode_access_token(&config, 1, Uuid::now_v7(), Utc::now()).expect("access token 应签发成功");

        let mut other_config = test_jwt_config();
        other_config.issuer = "another-issuer".to_owned();

        assert!(AuthService::verify_access_token(&other_config, &token).is_err(), "签发者不匹配的令牌必须被拒绝");
    }

    #[test]
    fn rejects_expired_token() {
        let mut config = test_jwt_config();
        config.access_ttl_minutes = -10;

        let token = encode_access_token(&config, 1, Uuid::now_v7(), Utc::now()).expect("access token 应签发成功");

        // 恢复正数有效期，确保失败原因只可能是令牌过期。
        config.access_ttl_minutes = 15;

        match AuthService::verify_access_token(&config, &token) {
            Err(AppError::Unauthorized(message)) => assert_eq!(message, "认证令牌已过期"),
            other => panic!("过期令牌应返回 401，实际为 {other:?}"),
        }
    }

    /// 该用例锁定一条关键安全约束：伪造的 `typ` 不能通过校验。
    /// 缺少这项检查时，长效刷新令牌就能被当作 access token 使用。
    #[test]
    fn rejects_token_with_wrong_type() {
        let config = test_jwt_config();
        let now = Utc::now();
        let claims = AccessClaims {
            sub: "1".to_owned(),
            iss: config.issuer.clone(),
            exp: (now + ChronoDuration::minutes(15)).timestamp(),
            iat: now.timestamp(),
            jti: "test".to_owned(),
            typ: "refresh".to_owned(),
        };
        let token = encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(config.secret.as_bytes())).expect("测试令牌应签发成功");

        match AuthService::verify_access_token(&config, &token) {
            Err(AppError::Unauthorized(message)) => assert_eq!(message, "认证令牌类型错误"),
            other => panic!("类型错误的令牌应返回 401，实际为 {other:?}"),
        }
    }

    #[test]
    fn rejects_non_numeric_subject() {
        let config = test_jwt_config();
        let now = Utc::now();
        let claims = AccessClaims {
            sub: "not-a-number".to_owned(),
            iss: config.issuer.clone(),
            exp: (now + ChronoDuration::minutes(15)).timestamp(),
            iat: now.timestamp(),
            jti: "test".to_owned(),
            typ: TOKEN_TYPE_ACCESS.to_owned(),
        };
        let token = encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(config.secret.as_bytes())).expect("测试令牌应签发成功");

        assert!(AuthService::verify_access_token(&config, &token).is_err(), "主体不是数字的令牌必须被拒绝");
    }

    /// 窗口至少一分钟：零长度窗口等于关闭限流。
    #[test]
    fn rate_limit_window_is_never_shorter_than_a_minute() {
        let config = AuthConfig {
            max_login_failures: 5,
            login_window_minutes: 15,
            cookie: crate::config::CookieConfig {
                access_name: "access_token".to_owned(),
                refresh_name: "refresh_token".to_owned(),
                secure: false,
                same_site: "strict".to_owned(),
                path: "/".to_owned(),
            },
        };

        assert_eq!(rate_limit_window(&config), Duration::from_secs(900));

        let mut zero_window = config.clone();
        zero_window.login_window_minutes = 0;
        assert_eq!(rate_limit_window(&zero_window), Duration::from_secs(60), "非法窗口应兜底为一分钟，而不是变成零");
    }

    /// 无法解析的令牌不应触发任何 Redis 写入：它本来就通不过认证中间件。
    #[tokio::test]
    async fn skips_revocation_for_unparsable_token() {
        let store = crate::infrastructure::redis::RedisStore::without_connection();

        let result = AuthService::revoke_access_token(&store, &test_jwt_config(), "not-a-jwt").await;

        assert!(result.is_ok(), "无法解析的令牌应直接返回成功，且不访问 Redis");
    }

    /// 已过期的令牌无需写入吊销列表：它已经不可能通过认证。
    #[tokio::test]
    async fn skips_revocation_for_expired_token() {
        let store = crate::infrastructure::redis::RedisStore::without_connection();
        let mut config = test_jwt_config();
        config.access_ttl_minutes = -10;
        let expired = encode_access_token(&config, 1, Uuid::now_v7(), Utc::now()).expect("access token 应签发成功");
        config.access_ttl_minutes = 15;

        let result = AuthService::revoke_access_token(&store, &config, &expired).await;

        assert!(result.is_ok(), "过期令牌应直接跳过吊销写入");
    }

    #[test]
    fn generates_unique_refresh_tokens() {
        let first = generate_refresh_token();
        let second = generate_refresh_token();
        assert_ne!(first, second, "刷新令牌必须每次不同");
        // 32 字节十六进制编码后长度为 64。
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn hashes_token_deterministically() {
        let token = "sample-refresh-token";

        assert_eq!(hash_token(token), hash_token(token));
        assert_ne!(hash_token(token), hash_token("another-token"));
        assert_eq!(hash_token(token).len(), 64);
    }

    /// 摘要算法必须区分大小写且不做归一化，避免不同令牌映射到同一摘要。
    #[test]
    fn token_hash_is_case_sensitive() {
        assert_ne!(hash_token("ABC"), hash_token("abc"));
    }

    #[test]
    fn verifies_matching_password() {
        let hash = hash_password("correct-password").expect("密码应哈希成功");

        assert!(verify_password_blocking("correct-password", &hash).expect("校验应成功"), "正确密码应通过校验");
        assert!(!verify_password_blocking("wrong-password", &hash).expect("校验应成功"), "错误密码不应通过校验");
    }

    #[test]
    fn rejects_malformed_password_hash() {
        assert!(
            verify_password_blocking("any-password", "not-an-argon2-hash").is_err(),
            "非法哈希格式应返回内部错误"
        );
    }

    /// 防时间攻击用的占位哈希必须是合法 Argon2 输出。
    ///
    /// 若它无法被解析，"用户不存在"分支会在计算前就失败并提前返回，
    /// 使响应时间重新变得可区分，时间攻击防护随之失效。
    #[test]
    fn dummy_password_hash_is_parseable() {
        assert!(PasswordHash::new(DUMMY_PASSWORD_HASH).is_ok(), "占位哈希必须是合法的 Argon2 编码");
    }

    #[test]
    fn hex_encoding_uses_lowercase() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xff]), "000fff");
    }
}
