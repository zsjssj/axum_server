use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// 登出时定位会话所需的最小记录。
///
/// 只查询主键：登出只需吊销目标会话，其余字段会让结构体出现
/// 永远不被读取的成员。
#[derive(Debug, sqlx::FromRow)]
pub struct SessionRecord {
    pub id: i64,
}

/// 会话及其所属用户的公开标识。
///
/// 轮换令牌时需要同时取得用户公开标识用于签发新的 access token，
/// 放在同一个查询里避免多发一次往返。
///
/// `is_revoked` 刻意让 SQL 返回布尔值而不是映射 `Option<DateTime<Utc>>`：
/// 驱动对 `Option` 的行为在 PostgreSQL 与 SQLite 之间并不一致，
/// 直接返回布尔值可以避开这个差异。
#[derive(Debug, sqlx::FromRow)]
pub struct SessionWithUser {
    pub id: i64,
    pub user_id: i64,
    pub public_id: Uuid,
    pub is_revoked: bool,
    pub expires_at: DateTime<Utc>,
}

/// 认证数据访问层。
///
/// 覆盖刷新令牌会话与登录失败计数两类持久化需求。
pub struct AuthRepository;

impl AuthRepository {
    /// 创建刷新令牌会话，返回新会话主键。
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

    /// 按令牌摘要查询会话及其用户公开标识。
    pub async fn find_session_with_user(
        db: &PgPool,
        token_hash: &str,
    ) -> Result<Option<SessionWithUser>, sqlx::Error> {
        let session = sqlx::query_as::<_, SessionWithUser>(
            r#"
            SELECT s.id, s.user_id, u.public_id,
                   (s.revoked_at IS NOT NULL) AS is_revoked, s.expires_at
            FROM user_sessions s
            JOIN users u ON u.id = s.user_id
            WHERE s.token_hash = $1
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

    /// 记录令牌轮换关系：旧会话标记吊销并指向新会话。
    ///
    /// 两条更新放在同一事务中，避免出现旧会话已吊销而轮换关系缺失的中间状态。
    pub async fn rotate_session(db: &PgPool, old_id: i64, new_id: i64) -> Result<(), sqlx::Error> {
        let mut transaction = db.begin().await?;

        sqlx::query(
            r#"
            UPDATE user_sessions
            SET revoked_at = NOW(), replaced_by = $2
            WHERE id = $1 AND revoked_at IS NULL
            "#,
        )
        .bind(old_id)
        .bind(new_id)
        .execute(&mut *transaction)
        .await?;

        transaction.commit().await?;

        Ok(())
    }

    /// 吊销指定用户的全部有效会话。
    ///
    /// 用于检测到刷新令牌重放时强制该用户重新登录：此时无法判断攻击者与
    /// 合法用户，只能同时失效双方的全部凭据。
    pub async fn revoke_all_sessions_for_user(
        db: &PgPool,
        user_id: i64,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE user_sessions
            SET revoked_at = NOW()
            WHERE user_id = $1 AND revoked_at IS NULL
            "#,
        )
        .bind(user_id)
        .execute(db)
        .await?;

        Ok(result.rows_affected())
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
