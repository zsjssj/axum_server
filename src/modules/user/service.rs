use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};

use crate::common::error::AppError;
use crate::modules::user::model::{CreateUserRequest, UpdateUserRequest, User};
use crate::modules::user::repository::UserRepository;
use sqlx::PgPool;
use uuid::Uuid;

/// 用户业务逻辑层
pub struct UserService;

impl UserService {
    /// 创建用户
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
    pub async fn get_users(
        db: &PgPool,
        page: i64,
        page_size: i64,
    ) -> Result<(Vec<User>, i64), AppError> {
        let users = UserRepository::find_all(db, page, page_size).await?;
        let total = UserRepository::count(db).await?;

        Ok((users, total))
    }

    /// 根据公开 ID 获取用户
    pub async fn get_user_by_public_id(db: &PgPool, public_id: Uuid) -> Result<User, AppError> {
        UserRepository::find_by_public_id(db, public_id)
            .await?
            .ok_or(AppError::NotFound("用户不存在"))
    }

    /// 更新用户
    pub async fn update_user(
        db: &PgPool,
        public_id: Uuid,
        req: UpdateUserRequest,
    ) -> Result<User, AppError> {
        UserRepository::update(db, public_id, req.email.as_deref(), req.nickname.as_deref())
            .await?
            .ok_or(AppError::NotFound("用户不存在"))
    }

    /// 删除用户
    pub async fn delete_user(db: &PgPool, public_id: Uuid) -> Result<(), AppError> {
        let deleted = UserRepository::delete(db, public_id).await?;

        if !deleted {
            return Err(AppError::NotFound("用户不存在"));
        }

        Ok(())
    }
}

fn hash_password(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| AppError::Internal("密码哈希失败"))
}

fn generate_public_id() -> Uuid {
    Uuid::now_v7()
}

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::{PasswordHash, PasswordVerifier};

    #[test]
    fn hashes_password_with_argon2() {
        let encoded = hash_password("secret").expect("password should be hashed");
        let parsed = PasswordHash::new(&encoded).expect("hash should be valid");

        assert!(
            Argon2::default()
                .verify_password(b"secret", &parsed)
                .is_ok()
        );
        assert_ne!(encoded, "secret");
    }

    #[test]
    fn generates_uuid_v7_public_id() {
        let public_id = generate_public_id();

        assert_eq!(public_id.get_version_num(), 7);
    }
}
