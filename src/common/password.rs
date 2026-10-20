use crate::common::error::AppError;
use argon2::{Argon2, PasswordHasher};
// argon2::password_hash::{SaltString, rand_core::OsRng},

/// 使用 Argon2 默认参数计算密码哈希。
///
/// 抽取为公共能力：用户创建与认证模块都需要生成哈希，两处独立实现很容易
/// 在参数上逐渐分叉，导致同一密码在不同入口得到强度不一致的哈希。
pub fn hash_password(password: &str) -> Result<String, AppError> {
    // 每次哈希都使用独立随机盐，使相同密码得到不同结果，
    // 从而阻止攻击者通过比对哈希值批量识别使用相同密码的账号。
    // let salt = SaltString::generate(&mut OsRng);
    // Argon2::default()
    //     .hash_password(password.as_bytes(), &salt)
    //     .map(|hash| hash.to_string())
    //     .map_err(|_| AppError::Internal("密码哈希失败"))
    // 0.6版本自动生成随机盐，不需要调用者手动生成
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|_| AppError::Internal("密码哈希失败"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::{PasswordHash, PasswordVerifier};

    #[test]
    fn hashes_password_with_argon2() {
        let encoded = hash_password("secret").expect("密码应哈希成功");
        let parsed = PasswordHash::new(&encoded).expect("哈希应为合法格式");

        assert!(Argon2::default().verify_password(b"secret", &parsed).is_ok());
        assert_ne!(encoded, "secret", "哈希结果不应等于原始密码");
    }

    /// 随机盐必须使相同密码产生不同哈希，否则哈希值本身会泄露
    /// "哪些账号使用了相同密码"这一信息。
    #[test]
    fn produces_different_hashes_for_same_password() {
        let first = hash_password("same-password").expect("密码应哈希成功");
        let second = hash_password("same-password").expect("密码应哈希成功");

        assert_ne!(first, second, "相同密码必须因随机盐而得到不同哈希");
    }
}
