use crate::modules::user::model::{User, UserCredentials};
use sqlx::PgPool;
use uuid::Uuid;

/// 用户数据访问层
pub struct UserRepository;

impl UserRepository {
    /// 创建用户
    pub async fn create(db: &PgPool, public_id: Uuid, username: &str, email: &str, password_hash: &str, nickname: Option<&str>) -> Result<User, sqlx::Error> {
        let user = sqlx::query_as::<_, User>(
            r#"
            INSERT INTO users (public_id, username, email, password_hash, nickname)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING public_id, username, email, nickname, created_at, updated_at
            "#,
        )
        .bind(public_id)
        .bind(username)
        .bind(email)
        .bind(password_hash)
        .bind(nickname)
        .fetch_one(db)
        .await?;

        Ok(user)
    }

    /// 根据公开 ID 查询用户
    pub async fn find_by_public_id(db: &PgPool, public_id: Uuid) -> Result<Option<User>, sqlx::Error> {
        let user = sqlx::query_as::<_, User>(
            r#"
            SELECT public_id, username, email, nickname, created_at, updated_at
            FROM users
            WHERE public_id = $1
            "#,
        )
        .bind(public_id)
        .fetch_optional(db)
        .await?;

        Ok(user)
    }

    /// 根据内部主键查询用户。
    ///
    /// 认证令牌的 `sub` 携带内部主键，受保护接口据此加载用户资料。
    /// 返回公开的 [`User`]，不包含密码哈希。
    pub async fn find_by_id(db: &PgPool, id: i64) -> Result<Option<User>, sqlx::Error> {
        let user = sqlx::query_as::<_, User>(
            r#"
            SELECT public_id, username, email, nickname, created_at, updated_at
            FROM users
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(db)
        .await?;

        Ok(user)
    }

    /// 认证专用：按用户名查询内部主键与密码哈希。
    ///
    /// 返回 [`UserCredentials`] 而不是 [`User`]，使密码哈希无法顺着公开模型
    /// 进入响应；查询条件使用唯一索引 `idx_users_username`。
    pub async fn find_credentials_by_username(db: &PgPool, username: &str) -> Result<Option<UserCredentials>, sqlx::Error> {
        let credentials = sqlx::query_as::<_, UserCredentials>(
            r#"
            SELECT id, public_id, password_hash
            FROM users
            WHERE username = $1
            "#,
        )
        .bind(username)
        .fetch_optional(db)
        .await?;

        Ok(credentials)
    }

    /// 查询所有用户（分页）
    pub async fn find_all(db: &PgPool, page: i64, page_size: i64) -> Result<Vec<User>, sqlx::Error> {
        let offset = (page - 1) * page_size;

        let users = sqlx::query_as::<_, User>(
            r#"
            SELECT public_id, username, email, nickname, created_at, updated_at
            FROM users
            ORDER BY id DESC
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(db)
        .await?;

        Ok(users)
    }

    /// 统计用户总数
    pub async fn count(db: &PgPool) -> Result<i64, sqlx::Error> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users").fetch_one(db).await?;
        Ok(count)
    }

    /// 更新用户
    pub async fn update(db: &PgPool, public_id: Uuid, email: Option<&str>, nickname: Option<&str>) -> Result<Option<User>, sqlx::Error> {
        let user = sqlx::query_as::<_, User>(
            r#"
            UPDATE users
            SET email = COALESCE($2, email),
                nickname = COALESCE($3, nickname),
                updated_at = NOW()
            WHERE public_id = $1
            RETURNING public_id, username, email, nickname, created_at, updated_at
            "#,
        )
        .bind(public_id)
        .bind(email)
        .bind(nickname)
        .fetch_optional(db)
        .await?;

        Ok(user)
    }

    /// 删除用户
    pub async fn delete(db: &PgPool, public_id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM users WHERE public_id = $1").bind(public_id).execute(db).await?;

        Ok(result.rows_affected() > 0)
    }
}
