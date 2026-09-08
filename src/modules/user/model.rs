use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::ToSchema;
use uuid::Uuid;

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
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateUserRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    pub nickname: Option<String>,
}

/// 更新用户请求
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateUserRequest {
    pub email: Option<String>,
    pub nickname: Option<String>,
}

/// 删除用户响应
#[derive(Debug, Serialize, ToSchema)]
pub struct DeleteUserResponse {
    pub message: String,
}

fn serialize_datetime<T>(value: &chrono::NaiveDateTime, serializer: T) -> Result<T::Ok, T::Error>
where
    T: serde::Serializer,
{
    serializer.serialize_str(&value.format("%Y-%m-%d %H:%M:%S").to_string())
}
