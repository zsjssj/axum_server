use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
};

use crate::app::state::AppState;
use crate::common::{
    error::AppError,
    pagination::{PaginatedResponse, Pagination},
};
use crate::modules::user::{
    model::{CreateUserRequest, UpdateUserRequest},
    service::UserService,
};

/// 用户路由
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users).post(create_user))
        .route(
            "/users/:id",
            get(get_user).put(update_user).delete(delete_user),
        )
}

/// 创建用户
async fn create_user(
    State(state): State<AppState>,
    Json(req): Json<CreateUserRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user = UserService::create_user(&state.db, req).await?;

    Ok((StatusCode::CREATED, Json(user)))
}

/// 获取用户列表
async fn list_users(
    State(state): State<AppState>,
    Query(pagination): Query<Pagination>,
) -> Result<impl IntoResponse, AppError> {
    let (users, total) =
        UserService::get_users(&state.db, pagination.page(), pagination.page_size()).await?;
    let response = PaginatedResponse::new(users, total, &pagination);

    Ok(Json(response))
}

/// 获取单个用户
async fn get_user(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, AppError> {
    let user = UserService::get_user_by_id(&state.db, id).await?;

    Ok(Json(user))
}

/// 更新用户
async fn update_user(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user = UserService::update_user(&state.db, id, req).await?;

    Ok(Json(user))
}

/// 删除用户
async fn delete_user(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Result<impl IntoResponse, AppError> {
    UserService::delete_user(&state.db, id).await?;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "message": "用户删除成功"
        })),
    ))
}
