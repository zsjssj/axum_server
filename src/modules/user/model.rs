use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;
use validator::{Validate, ValidationError};

/// 用户实体
#[derive(Debug, Serialize, FromRow, ToSchema)]
pub struct User {
    pub public_id: Uuid,
    pub username: String,
    pub email: String,
    pub nickname: Option<String>,
    #[serde(serialize_with = "serialize_datetime")]
    #[schema(value_type = String)]
    pub created_at: chrono::NaiveDateTime,
    #[serde(serialize_with = "serialize_datetime")]
    #[schema(value_type = String)]
    pub updated_at: chrono::NaiveDateTime,
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
}
