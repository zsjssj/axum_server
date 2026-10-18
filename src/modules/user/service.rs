use crate::common::error::AppError;
use crate::common::password::hash_password;
use crate::infrastructure::redis::RedisStore;
use crate::modules::user::model::{CreateUserRequest, UpdateUserRequest, User};
use crate::modules::user::repository::UserRepository;
use sqlx::PgPool;
use uuid::Uuid;

/// 用户业务逻辑层
pub struct UserService;

impl UserService {
    /// 创建用户
    ///
    /// 不涉及缓存失效：列表接口不分页缓存，详情键按 `public_id` 命名，
    /// 新用户不可能已经存在于缓存中。
    pub async fn create_user(db: &PgPool, req: CreateUserRequest) -> Result<User, AppError> {
        let CreateUserRequest {
            username,
            email,
            password,
            nickname,
        } = req;
        let public_id = generate_public_id();
        let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
            .await
            .map_err(|_| AppError::Internal("密码哈希任务失败"))??;

        match UserRepository::create(
            db,
            public_id,
            &username,
            &email,
            &password_hash,
            nickname.as_deref(),
        )
        .await
        {
            Ok(user) => Ok(user),
            Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
                Err(AppError::Conflict("用户名已存在"))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// 获取用户列表（分页）
    ///
    /// 刻意不做缓存：分页结果与总数会随任意一次写入整体失效，命中率低而
    /// 失效成本高；真正的热点是单个用户详情，缓存收益远大于列表。
    pub async fn get_users(
        db: &PgPool,
        page: i64,
        page_size: i64,
    ) -> Result<(Vec<User>, i64), AppError> {
        let users = UserRepository::find_all(db, page, page_size).await?;
        let total = UserRepository::count(db).await?;

        Ok((users, total))
    }

    /// 根据公开 ID 获取用户，优先读 Redis 缓存。
    ///
    /// 采用 Cache-Aside（旁路缓存）模式：读请求先查 Redis，未命中再查数据库
    /// 并回填。缓存不可用时直接查库——它是加速层，不能成为接口可用性的前提。
    pub async fn get_user_by_public_id(
        db: &PgPool,
        redis: &RedisStore,
        public_id: Uuid,
    ) -> Result<User, AppError> {
        let key = redis.user_detail_key(public_id);

        match redis.get_json::<User>(&key).await {
            Ok(Some(user)) => {
                tracing::debug!(key, "用户详情命中缓存");
                return Ok(user);
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "读取用户缓存失败，直接查询数据库"),
        }

        let user = UserRepository::find_by_public_id(db, public_id)
            .await?
            .ok_or(AppError::NotFound("用户不存在"))?;

        // 回填失败只告警：本次响应已经有正确数据，缓存写不进去不应变成 500。
        if let Err(error) = redis
            .set_json(&key, &user, redis.user_cache_ttl())
            .await
        {
            tracing::warn!(%error, "回填用户缓存失败");
        }

        Ok(user)
    }

    /// 更新用户
    pub async fn update_user(
        db: &PgPool,
        redis: &RedisStore,
        public_id: Uuid,
        req: UpdateUserRequest,
    ) -> Result<User, AppError> {
        let user = UserRepository::update(db, public_id, req.email.as_deref(), req.nickname.as_deref())
            .await?
            .ok_or(AppError::NotFound("用户不存在"))?;

        // 先写数据库、再删缓存：若顺序颠倒，并发请求可能在两步之间把旧值
        // 重新写回缓存，留下一个直到 TTL 到期才自愈的脏数据。
        evict_user_cache(redis, public_id).await;

        Ok(user)
    }

    /// 删除用户
    pub async fn delete_user(
        db: &PgPool,
        redis: &RedisStore,
        public_id: Uuid,
    ) -> Result<(), AppError> {
        let deleted = UserRepository::delete(db, public_id).await?;

        if !deleted {
            return Err(AppError::NotFound("用户不存在"));
        }

        evict_user_cache(redis, public_id).await;

        Ok(())
    }
}

/// 删除用户详情缓存。
///
/// 采用「删除」而不是「写入新值」：并发更新时写入会让两次写的先后顺序
/// 决定缓存内容，而删除之后由下一次读请求回填，结果必然与数据库一致。
/// 删除失败只告警——缓存键带 TTL，陈旧数据最多存活 `redis.user_cache_ttl_seconds`。
async fn evict_user_cache(redis: &RedisStore, public_id: Uuid) {
    let key = redis.user_detail_key(public_id);

    if let Err(error) = redis.delete(&key).await {
        tracing::warn!(%error, "删除用户缓存失败，陈旧数据将在 TTL 到期后自动失效");
    }
}

fn generate_public_id() -> Uuid {
    Uuid::now_v7()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_uuid_v7_public_id() {
        let public_id = generate_public_id();

        assert_eq!(public_id.get_version_num(), 7);
    }
}
