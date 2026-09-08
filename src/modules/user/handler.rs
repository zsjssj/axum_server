use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
};

use crate::app::state::AppState;
use crate::common::{
    error::{AppError, ErrorResponse},
    pagination::{PaginatedResponse, Pagination},
};
use crate::modules::user::{
    model::{CreateUserRequest, DeleteUserResponse, UpdateUserRequest, User},
    service::UserService,
};
use uuid::Uuid;

/// 用户路由
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users).post(create_user))
        .route(
            "/users/:public_id",
            get(get_user).put(update_user).delete(delete_user),
        )
}

/// 创建用户
#[utoipa::path(
    post,
    path = "/api/v1/users",
    tag = "用户",
    request_body = CreateUserRequest,
    responses(
        (status = 201, description = "用户创建成功", body = User),
        (status = 409, description = "用户名已存在", body = ErrorResponse),
        (status = 500, description = "服务器内部错误", body = ErrorResponse)
    )
)]
pub(crate) async fn create_user(
    State(state): State<AppState>,
    Json(req): Json<CreateUserRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user = UserService::create_user(&state.db, req).await?;

    Ok((StatusCode::CREATED, Json(user)))
}

/// 获取用户列表
#[utoipa::path(
    get,
    path = "/api/v1/users",
    tag = "用户",
    params(Pagination),
    responses(
        (status = 200, description = "分页用户列表", body = PaginatedResponse<User>),
        (status = 500, description = "服务器内部错误", body = ErrorResponse)
    )
)]
pub(crate) async fn list_users(
    State(state): State<AppState>,
    Query(pagination): Query<Pagination>,
) -> Result<impl IntoResponse, AppError> {
    let (users, total) =
        UserService::get_users(&state.db, pagination.page(), pagination.page_size()).await?;
    let response = PaginatedResponse::new(users, total, &pagination);

    Ok(Json(response))
}

/// 获取单个用户
#[utoipa::path(
    get,
    path = "/api/v1/users/{public_id}",
    tag = "用户",
    params(("public_id" = Uuid, Path, description = "用户公开 ID（UUIDv7）")),
    responses(
        (status = 200, description = "用户详情", body = User),
        (status = 404, description = "用户不存在", body = ErrorResponse),
        (status = 500, description = "服务器内部错误", body = ErrorResponse)
    )
)]
pub(crate) async fn get_user(
    State(state): State<AppState>,
    Path(public_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let user = UserService::get_user_by_public_id(&state.db, public_id).await?;

    Ok(Json(user))
}

/// 更新用户
#[utoipa::path(
    put,
    path = "/api/v1/users/{public_id}",
    tag = "用户",
    params(("public_id" = Uuid, Path, description = "用户公开 ID（UUIDv7）")),
    request_body = UpdateUserRequest,
    responses(
        (status = 200, description = "用户更新成功", body = User),
        (status = 404, description = "用户不存在", body = ErrorResponse),
        (status = 500, description = "服务器内部错误", body = ErrorResponse)
    )
)]
pub(crate) async fn update_user(
    State(state): State<AppState>,
    Path(public_id): Path<Uuid>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user = UserService::update_user(&state.db, public_id, req).await?;

    Ok(Json(user))
}

/// 删除用户
#[utoipa::path(
    delete,
    path = "/api/v1/users/{public_id}",
    tag = "用户",
    params(("public_id" = Uuid, Path, description = "用户公开 ID（UUIDv7）")),
    responses(
        (status = 200, description = "用户删除成功", body = DeleteUserResponse),
        (status = 404, description = "用户不存在", body = ErrorResponse),
        (status = 500, description = "服务器内部错误", body = ErrorResponse)
    )
)]
pub(crate) async fn delete_user(
    State(state): State<AppState>,
    Path(public_id): Path<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    UserService::delete_user(&state.db, public_id).await?;

    Ok((
        StatusCode::OK,
        Json(DeleteUserResponse {
            message: "用户删除成功".to_owned(),
        }),
    ))
}
