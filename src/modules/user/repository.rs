use crate::modules::user::model::User;
use sqlx::PgPool;
use uuid::Uuid;

/// 用户数据访问层
pub struct UserRepository;

impl UserRepository {
    /// 创建用户
    pub async fn create(
        db: &PgPool,
        public_id: Uuid,
        username: &str,
        email: &str,
        password_hash: &str,
        nickname: Option<&str>,
    ) -> Result<User, sqlx::Error> {
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
    pub async fn find_by_public_id(
        db: &PgPool,
        public_id: Uuid,
    ) -> Result<Option<User>, sqlx::Error> {
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

    /// 查询所有用户（分页）
    pub async fn find_all(
        db: &PgPool,
        page: i64,
        page_size: i64,
    ) -> Result<Vec<User>, sqlx::Error> {
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
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(db)
            .await?;

        Ok(count)
    }

    /// 更新用户
    pub async fn update(
        db: &PgPool,
        public_id: Uuid,
        email: Option<&str>,
        nickname: Option<&str>,
    ) -> Result<Option<User>, sqlx::Error> {
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
        let result = sqlx::query("DELETE FROM users WHERE public_id = $1")
            .bind(public_id)
            .execute(db)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}
