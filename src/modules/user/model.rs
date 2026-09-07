use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// 用户实体
#[derive(Debug, Serialize, FromRow)]
pub struct User {
    pub id: i32,
    pub username: String,
    pub email: String,
    pub nickname: Option<String>,
    #[serde(serialize_with = "serialize_datetime")]
    pub created_at: chrono::NaiveDateTime,
    #[serde(serialize_with = "serialize_datetime")]
    pub updated_at: chrono::NaiveDateTime,
}

/// 创建用户请求
#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    pub nickname: Option<String>,
}

/// 更新用户请求
#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub email: Option<String>,
    pub nickname: Option<String>,
}

fn serialize_datetime<S>(value: &chrono::NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.format("%Y-%m-%d %H:%M:%S").to_string())
}
