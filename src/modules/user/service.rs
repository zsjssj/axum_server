use argon2::{
    Argon2, PasswordHasher,
    password_hash::{SaltString, rand_core::OsRng},
};

use crate::common::error::AppError;
use crate::modules::user::model::{CreateUserRequest, UpdateUserRequest, User};
use crate::modules::user::repository::UserRepository;
use sqlx::PgPool;

/// 用户业务逻辑层
pub struct UserService;

impl UserService {
    /// 创建用户
    pub async fn create_user(db: &PgPool, req: CreateUserRequest) -> Result<User, AppError> {
        let password_hash = hash_password(&req.password)?;

        match UserRepository::create(
            db,
            &req.username,
            &req.email,
            &password_hash,
            req.nickname.as_deref(),
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

    /// 根据 ID 获取用户
    pub async fn get_user_by_id(db: &PgPool, id: i32) -> Result<User, AppError> {
        UserRepository::find_by_id(db, id)
            .await?
            .ok_or(AppError::NotFound("用户不存在"))
    }

    /// 更新用户
    pub async fn update_user(
        db: &PgPool,
        id: i32,
        req: UpdateUserRequest,
    ) -> Result<User, AppError> {
        UserRepository::update(db, id, req.email.as_deref(), req.nickname.as_deref())
            .await?
            .ok_or(AppError::NotFound("用户不存在"))
    }

    /// 删除用户
    pub async fn delete_user(db: &PgPool, id: i32) -> Result<(), AppError> {
        let deleted = UserRepository::delete(db, id).await?;

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
}
