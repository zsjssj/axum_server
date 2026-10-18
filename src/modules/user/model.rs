use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;
use validator::{Validate, ValidationError};

/// 用户实体
///
/// 同时派生 `Serialize` 与 `Deserialize`：除了作为响应模型，它还是 Redis
/// 中用户资料缓存的序列化格式。两端使用同一类型可以避免「缓存结构体」与
/// 响应模型各自演进、最终返回字段不一致的问题；两个时间字段的序列化格式
/// 也因此被固定为同一种（见 [`serialize_datetime`] 与 [`deserialize_datetime`]）。
#[derive(Debug, Serialize, Deserialize, FromRow, ToSchema)]
pub struct User {
    pub public_id: Uuid,
    pub username: String,
    pub email: String,
    pub nickname: Option<String>,
    #[serde(serialize_with = "serialize_datetime", deserialize_with = "deserialize_datetime")]
    #[schema(value_type = String)]
    pub created_at: chrono::NaiveDateTime,
    #[serde(serialize_with = "serialize_datetime", deserialize_with = "deserialize_datetime")]
    #[schema(value_type = String)]
    pub updated_at: chrono::NaiveDateTime,
}

/// 认证专用的用户凭证视图。
///
/// 刻意与公开的 [`User`] 分离，且不派生 `Serialize` 与 `ToSchema`：
/// 一旦把 `password_hash` 挂到 `User` 上，它会顺着响应序列化和 OpenAPI
/// Schema 一起对外暴露。类型分离是这里唯一可靠的防线。
#[derive(Debug, sqlx::FromRow)]
pub struct UserCredentials {
    /// 内部自增主键，仅用于数据库关联与令牌 `sub`，不通过 API 暴露。
    pub id: i64,
    pub public_id: Uuid,
    pub password_hash: String,
}

/// 创建用户请求
#[derive(Debug, Deserialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct CreateUserRequest {
    #[validate(length(min = 3, max = 50, message = "用户名长度必须在 3 到 50 个字符之间"))]
    #[schema(min_length = 3, max_length = 50)]
    pub username: String,
    #[validate(length(max = 100, message = "邮箱长度不能超过 100 个字符"))]
    #[validate(email(message = "邮箱格式不正确"))]
    #[schema(format = Email, max_length = 100)]
    pub email: String,
    #[validate(length(min = 8, max = 72, message = "密码长度必须在 8 到 72 个字符之间"))]
    #[schema(min_length = 8, max_length = 72, write_only)]
    pub password: String,
    #[validate(length(max = 100, message = "昵称长度不能超过 100 个字符"))]
    #[schema(max_length = 100)]
    pub nickname: Option<String>,
}

/// 更新用户请求
#[derive(Debug, Deserialize, ToSchema, Validate)]
#[validate(schema(function = "validate_update_user_request"))]
#[serde(deny_unknown_fields)]
pub struct UpdateUserRequest {
    #[validate(length(max = 100, message = "邮箱长度不能超过 100 个字符"))]
    #[validate(email(message = "邮箱格式不正确"))]
    #[schema(format = Email, max_length = 100)]
    pub email: Option<String>,
    #[validate(length(max = 100, message = "昵称长度不能超过 100 个字符"))]
    #[schema(max_length = 100)]
    pub nickname: Option<String>,
}

fn validate_update_user_request(request: &UpdateUserRequest) -> Result<(), ValidationError> {
    if request.email.is_none() && request.nickname.is_none() {
        let mut error = ValidationError::new("empty_update");
        error.message = Some("至少需要提供一个待更新字段".into());
        return Err(error);
    }

    Ok(())
}

fn serialize_datetime<T>(value: &chrono::NaiveDateTime, serializer: T) -> Result<T::Ok, T::Error>
where
    T: serde::Serializer,
{
    serializer.serialize_str(&value.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// 按与 [`serialize_datetime`] 完全相同的格式解析时间。
///
/// 缓存读取必须使用对应的解析规则：JSON 中日期是字符串，默认的
/// `chrono` 反序列化期待 RFC3339，格式不一致会让缓存命中直接变成反序列化错误。
fn deserialize_datetime<'de, D>(deserializer: D) -> Result<chrono::NaiveDateTime, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;

    chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S")
        .map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_create_request() {
        let request = CreateUserRequest {
            username: "ab".to_owned(),
            email: "invalid-email".to_owned(),
            password: "short".to_owned(),
            nickname: None,
        };
        let errors = request.validate().expect_err("非法创建参数应校验失败");

        assert!(errors.field_errors().contains_key("username"));
        assert!(errors.field_errors().contains_key("email"));
        assert!(errors.field_errors().contains_key("password"));
    }

    #[test]
    fn rejects_empty_update_request() {
        let request = UpdateUserRequest {
            email: None,
            nickname: None,
        };
        let errors = request.validate().expect_err("空更新请求应校验失败");

        assert!(errors.field_errors().contains_key("__all__"));
    }

    /// 缓存与响应共用同一个 `User` 结构，序列化结果必须能被反序列化回来。
    /// 两个时间字段的格式一旦不一致，缓存命中会直接变成 500，
    /// 而走数据库的路径却完全正常——这类问题只在有缓存时暴露，必须由测试锁定。
    #[test]
    fn user_round_trips_through_cache_json() {
        let timestamp = chrono::NaiveDateTime::parse_from_str("2024-05-06 07:08:09", "%Y-%m-%d %H:%M:%S")
            .expect("测试时间应合法");
        let user = User {
            public_id: Uuid::now_v7(),
            username: "cache-user".to_owned(),
            email: "cache@example.com".to_owned(),
            nickname: Some("缓存用户".to_owned()),
            created_at: timestamp,
            updated_at: timestamp,
        };

        let json = serde_json::to_string(&user).expect("用户应可序列化为缓存值");
        let restored: User = serde_json::from_str(&json).expect("缓存值应可反序列化回用户");

        assert_eq!(restored.public_id, user.public_id);
        assert_eq!(restored.username, user.username);
        assert_eq!(restored.nickname, user.nickname);
        assert_eq!(restored.created_at, timestamp);
        assert_eq!(restored.updated_at, timestamp);
    }
}
