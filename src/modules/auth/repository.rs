use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// 登出时定位会话所需的最小记录。
///
/// 只查询主键：登出只需吊销目标会话，其余字段会让结构体出现
/// 永远不被读取的成员。
#[derive(Debug, sqlx::FromRow)]
pub struct SessionRecord {
    pub id: i64,
}

/// 认证数据访问层。
///
/// 覆盖登录会话审计与登录失败计数两类持久化需求。会话表在单令牌方案下
/// **不参与认证**：每次请求的令牌校验只看签名与 Redis 吊销列表，
/// 这里写入的记录用于回答「谁在什么时候、从哪个客户端登录过」。
pub struct AuthRepository;

impl AuthRepository {
    /// 创建一条登录会话审计记录，返回新记录主键。
    ///
    /// `token_hash` 是登录令牌的摘要，不是令牌本身：数据库泄露时攻击者
    /// 无法用摘要反推出可用的令牌。
    pub async fn create_session(
        db: &PgPool,
        user_id: i64,
        token_hash: &str,
        expires_at: DateTime<Utc>,
        client_ip: Option<&str>,
        user_agent: Option<&str>,
    ) -> Result<i64, sqlx::Error> {
        // 使用 $5::inet 显式转换：绑定参数以文本传输，由 PostgreSQL 负责
        // 校验 IP 格式，非法输入会在这里报错而不是被静默写入。
        let (id,): (i64,) = sqlx::query_as(
            r#"
            INSERT INTO user_sessions (user_id, token_hash, expires_at, client_ip, user_agent)
            VALUES ($1, $2, $3, $4::inet, $5)
            RETURNING id
            "#,
        )
        .bind(user_id)
        .bind(token_hash)
        .bind(expires_at)
        .bind(client_ip)
        .bind(user_agent)
        .fetch_one(db)
        .await?;

        Ok(id)
    }

    /// 按令牌摘要查询用于登出的会话主键。
    pub async fn find_session_by_token_hash(
        db: &PgPool,
        token_hash: &str,
    ) -> Result<Option<SessionRecord>, sqlx::Error> {
        let session = sqlx::query_as::<_, SessionRecord>(
            r#"
            SELECT id
            FROM user_sessions
            WHERE token_hash = $1
            "#,
        )
        .bind(token_hash)
        .fetch_optional(db)
        .await?;

        Ok(session)
    }

    /// 吊销单个会话。
    ///
    /// 使用 `revoked_at IS NULL` 作为条件，使重复吊销成为幂等操作，
    /// 并为调用方保留"本次是否真的改变了状态"的信息。
    pub async fn revoke_session(db: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE user_sessions
            SET revoked_at = NOW()
            WHERE id = $1 AND revoked_at IS NULL
            "#,
        )
        .bind(id)
        .execute(db)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// 统计指定时间点之后的登录失败次数。
    pub async fn count_failures_since(
        db: &PgPool,
        username: &str,
        since: DateTime<Utc>,
    ) -> Result<i64, sqlx::Error> {
        let (count,): (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*)
            FROM login_failures
            WHERE username = $1 AND created_at >= $2
            "#,
        )
        .bind(username)
        .bind(since)
        .fetch_one(db)
        .await?;

        Ok(count)
    }

    /// 记录一次登录失败。
    pub async fn record_failure(
        db: &PgPool,
        username: &str,
        client_ip: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO login_failures (username, client_ip)
            VALUES ($1, $2::inet)
            "#,
        )
        .bind(username)
        .bind(client_ip)
        .execute(db)
        .await?;

        Ok(())
    }

    /// 清除指定时间点之前的失败记录，防止表随失败尝试无限增长。
    ///
    /// 在登录成功时顺带清理，避免为此单独引入定时任务；窗口外的记录
    /// 已经不参与限流判定，删除不影响安全语义。
    pub async fn clear_failures_before(
        db: &PgPool,
        username: &str,
        before: DateTime<Utc>,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM login_failures
            WHERE username = $1 AND created_at < $2
            "#,
        )
        .bind(username)
        .bind(before)
        .execute(db)
        .await?;

        Ok(result.rows_affected())
    }
}
