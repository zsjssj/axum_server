//! 聊天消息模块的 HTTP 适配层。
//!
//! 全部路由都挂在 `/api/v1` 之下的受保护路由组里，因此**自动需要登录**：
//! 认证由 `common::auth::require_login` 中间件统一强制，这里不做任何认证声明。
//! Handler 签名中的 [`CurrentUser`] 有两个作用：表达「该接口需要身份」，
//! 以及取出用户内部主键交给 Service——归属永远来自令牌，绝不来自请求体。

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{delete, get, post},
};

use crate::app::state::AppState;
use crate::common::{
    auth_extractor::CurrentUser,
    error::{AppError, ErrorData},
    extractor::{ApiPath, ValidatedJson, ValidatedQuery},
    pagination::{PaginatedResponse, Pagination},
    response::{ApiResponse, EmptyData},
};
use crate::modules::information::{
    model::{
        AddMembersRequest, ConversationDetail, ConversationSummary, CreateConversationRequest, Message, MessagePage,
        MessagePageQuery, ReadReceipt, SendMessageRequest,
    },
    service::InformationService,
};
use uuid::Uuid;

/// 聊天消息路由。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/conversations", get(list_conversations).post(create_conversation))
        .route("/conversations/:public_id", get(get_conversation))
        .route("/conversations/:public_id/messages", get(list_messages).post(send_message))
        .route("/conversations/:public_id/read", post(mark_read))
        .route("/conversations/:public_id/members", post(add_members))
        .route("/conversations/:public_id/members/me", delete(leave_conversation))
        .route("/messages/:public_id/recall", post(recall_message))
}

/// 创建会话
///
/// 单聊是幂等的：同一对用户重复调用会返回已存在的会话（`200`），
/// 只有真正新建时才返回 `201`。
#[utoipa::path(
    post,
    path = "/api/v1/conversations",
    tag = "聊天",
    request_body = CreateConversationRequest,
    responses(
        (status = 201, description = "会话创建成功", body = ApiResponse<ConversationDetail>),
        (status = 200, description = "单聊会话已存在，返回已有会话", body = ApiResponse<ConversationDetail>),
        (status = 400, description = "JSON 请求体格式错误", body = ApiResponse<ErrorData>),
        (status = 401, description = "未登录或登录已过期", body = ApiResponse<ErrorData>),
        (status = 404, description = "指定的用户不存在", body = ApiResponse<ErrorData>),
        (status = 415, description = "请求体媒体类型错误", body = ApiResponse<ErrorData>),
        (status = 422, description = "请求参数校验失败", body = ApiResponse<ErrorData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn create_conversation(
    State(state): State<AppState>,
    current_user: CurrentUser,
    ValidatedJson(req): ValidatedJson<CreateConversationRequest>,
) -> Result<impl IntoResponse, AppError> {
    let (conversation, created) = InformationService::create_conversation(&state.db, current_user.user_id, req).await?;

    let status = if created { StatusCode::CREATED } else { StatusCode::OK };
    let message = if created { "会话创建成功" } else { "会话已存在" };

    Ok((status, Json(ApiResponse::success(message, conversation))))
}

/// 我的会话列表
#[utoipa::path(
    get,
    path = "/api/v1/conversations",
    tag = "聊天",
    params(Pagination),
    responses(
        (status = 200, description = "分页会话列表，含未读数与最后一条消息", body = ApiResponse<PaginatedResponse<ConversationSummary>>),
        (status = 400, description = "查询参数格式错误", body = ApiResponse<ErrorData>),
        (status = 401, description = "未登录或登录已过期", body = ApiResponse<ErrorData>),
        (status = 422, description = "分页参数校验失败", body = ApiResponse<ErrorData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn list_conversations(
    State(state): State<AppState>,
    current_user: CurrentUser,
    ValidatedQuery(pagination): ValidatedQuery<Pagination>,
) -> Result<impl IntoResponse, AppError> {
    let (conversations, total) =
        InformationService::list_conversations(&state.db, current_user.user_id, pagination.page(), pagination.page_size()).await?;

    Ok(Json(ApiResponse::success(
        "查询成功",
        PaginatedResponse::new(conversations, total, &pagination),
    )))
}

/// 获取会话详情
#[utoipa::path(
    get,
    path = "/api/v1/conversations/{public_id}",
    tag = "聊天",
    params(("public_id" = Uuid, Path, description = "会话公开 ID（UUIDv7）")),
    responses(
        (status = 200, description = "会话详情，含成员列表", body = ApiResponse<ConversationDetail>),
        (status = 400, description = "路径参数格式错误", body = ApiResponse<ErrorData>),
        (status = 401, description = "未登录或登录已过期", body = ApiResponse<ErrorData>),
        (status = 404, description = "会话不存在或当前用户不在会话中", body = ApiResponse<ErrorData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn get_conversation(
    State(state): State<AppState>,
    current_user: CurrentUser,
    ApiPath(public_id): ApiPath<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let conversation = InformationService::get_conversation(&state.db, current_user.user_id, public_id).await?;

    Ok(Json(ApiResponse::success("查询成功", conversation)))
}

/// 发送消息
#[utoipa::path(
    post,
    path = "/api/v1/conversations/{public_id}/messages",
    tag = "聊天",
    params(("public_id" = Uuid, Path, description = "会话公开 ID（UUIDv7）")),
    request_body = SendMessageRequest,
    responses(
        (status = 201, description = "消息已发送", body = ApiResponse<Message>),
        (status = 400, description = "JSON 请求体或路径参数格式错误", body = ApiResponse<ErrorData>),
        (status = 401, description = "未登录或登录已过期", body = ApiResponse<ErrorData>),
        (status = 404, description = "会话不存在或当前用户不在会话中", body = ApiResponse<ErrorData>),
        (status = 415, description = "请求体媒体类型错误", body = ApiResponse<ErrorData>),
        (status = 422, description = "请求参数校验失败，或被引用消息不属于该会话", body = ApiResponse<ErrorData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn send_message(
    State(state): State<AppState>,
    current_user: CurrentUser,
    ApiPath(public_id): ApiPath<Uuid>,
    ValidatedJson(req): ValidatedJson<SendMessageRequest>,
) -> Result<impl IntoResponse, AppError> {
    let message = InformationService::send_message(&state.db, current_user.user_id, public_id, req).await?;

    Ok((StatusCode::CREATED, Json(ApiResponse::success("消息发送成功", message))))
}

/// 拉取历史消息
#[utoipa::path(
    get,
    path = "/api/v1/conversations/{public_id}/messages",
    tag = "聊天",
    params(("public_id" = Uuid, Path, description = "会话公开 ID（UUIDv7）"), MessagePageQuery),
    responses(
        (status = 200, description = "按序号倒序的历史消息分页", body = ApiResponse<MessagePage>),
        (status = 400, description = "查询参数或路径参数格式错误", body = ApiResponse<ErrorData>),
        (status = 401, description = "未登录或登录已过期", body = ApiResponse<ErrorData>),
        (status = 404, description = "会话不存在或当前用户不在会话中", body = ApiResponse<ErrorData>),
        (status = 422, description = "分页参数校验失败", body = ApiResponse<ErrorData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn list_messages(
    State(state): State<AppState>,
    current_user: CurrentUser,
    ApiPath(public_id): ApiPath<Uuid>,
    ValidatedQuery(query): ValidatedQuery<MessagePageQuery>,
) -> Result<impl IntoResponse, AppError> {
    let page = InformationService::list_messages(&state.db, current_user.user_id, public_id, query).await?;

    Ok(Json(ApiResponse::success("查询成功", page)))
}

/// 标记会话已读
///
/// 已读位置固定推进到会话当前最新序号，不接受客户端指定位置：
/// 由服务端决定「读到哪」，就不存在「客户端上报一个很大的数字把未读一次清空」的问题。
#[utoipa::path(
    post,
    path = "/api/v1/conversations/{public_id}/read",
    tag = "聊天",
    params(("public_id" = Uuid, Path, description = "会话公开 ID（UUIDv7）")),
    responses(
        (status = 200, description = "已读位置已推进", body = ApiResponse<ReadReceipt>),
        (status = 400, description = "路径参数格式错误", body = ApiResponse<ErrorData>),
        (status = 401, description = "未登录或登录已过期", body = ApiResponse<ErrorData>),
        (status = 404, description = "会话不存在或当前用户不在会话中", body = ApiResponse<ErrorData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn mark_read(
    State(state): State<AppState>,
    current_user: CurrentUser,
    ApiPath(public_id): ApiPath<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let receipt = InformationService::mark_read(&state.db, current_user.user_id, public_id).await?;

    Ok(Json(ApiResponse::success("已标记为已读", receipt)))
}

/// 撤回消息
#[utoipa::path(
    post,
    path = "/api/v1/messages/{public_id}/recall",
    tag = "聊天",
    params(("public_id" = Uuid, Path, description = "消息公开 ID（UUIDv7）")),
    responses(
        (status = 200, description = "消息已撤回，返回的正文为空", body = ApiResponse<Message>),
        (status = 400, description = "路径参数格式错误", body = ApiResponse<ErrorData>),
        (status = 401, description = "未登录或登录已过期", body = ApiResponse<ErrorData>),
        (status = 403, description = "无权撤回该消息（非发送者且不是群主）", body = ApiResponse<ErrorData>),
        (status = 404, description = "消息不存在或当前用户不在会话中", body = ApiResponse<ErrorData>),
        (status = 409, description = "消息已撤回或已超过可撤回时间", body = ApiResponse<ErrorData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn recall_message(
    State(state): State<AppState>,
    current_user: CurrentUser,
    ApiPath(public_id): ApiPath<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    let message = InformationService::recall_message(&state.db, current_user.user_id, public_id).await?;

    Ok(Json(ApiResponse::success("消息已撤回", message)))
}

/// 添加群成员
#[utoipa::path(
    post,
    path = "/api/v1/conversations/{public_id}/members",
    tag = "聊天",
    params(("public_id" = Uuid, Path, description = "会话公开 ID（UUIDv7）")),
    request_body = AddMembersRequest,
    responses(
        (status = 200, description = "成员已添加，返回更新后的会话详情", body = ApiResponse<ConversationDetail>),
        (status = 400, description = "JSON 请求体或路径参数格式错误", body = ApiResponse<ErrorData>),
        (status = 401, description = "未登录或登录已过期", body = ApiResponse<ErrorData>),
        (status = 403, description = "当前用户不是群主", body = ApiResponse<ErrorData>),
        (status = 404, description = "会话或指定用户不存在", body = ApiResponse<ErrorData>),
        (status = 409, description = "单聊不能加人、成员已达上限或所选用户已在会话中", body = ApiResponse<ErrorData>),
        (status = 415, description = "请求体媒体类型错误", body = ApiResponse<ErrorData>),
        (status = 422, description = "请求参数校验失败", body = ApiResponse<ErrorData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn add_members(
    State(state): State<AppState>,
    current_user: CurrentUser,
    ApiPath(public_id): ApiPath<Uuid>,
    ValidatedJson(req): ValidatedJson<AddMembersRequest>,
) -> Result<impl IntoResponse, AppError> {
    let conversation = InformationService::add_members(&state.db, current_user.user_id, public_id, req).await?;

    Ok(Json(ApiResponse::success("成员添加成功", conversation)))
}

/// 退出群聊
///
/// 群主退出时会把群主转让给最早加入的成员；最后一个成员退出时会话自动解散。
#[utoipa::path(
    delete,
    path = "/api/v1/conversations/{public_id}/members/me",
    tag = "聊天",
    params(("public_id" = Uuid, Path, description = "会话公开 ID（UUIDv7）")),
    responses(
        (status = 200, description = "已退出群聊", body = ApiResponse<EmptyData>),
        (status = 400, description = "路径参数格式错误", body = ApiResponse<ErrorData>),
        (status = 401, description = "未登录或登录已过期", body = ApiResponse<ErrorData>),
        (status = 404, description = "会话不存在或当前用户不在会话中", body = ApiResponse<ErrorData>),
        (status = 409, description = "单聊会话不支持退出", body = ApiResponse<ErrorData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn leave_conversation(
    State(state): State<AppState>,
    current_user: CurrentUser,
    ApiPath(public_id): ApiPath<Uuid>,
) -> Result<impl IntoResponse, AppError> {
    InformationService::leave_conversation(&state.db, current_user.user_id, public_id).await?;

    Ok(Json(ApiResponse::<()>::success_without_data("已退出群聊")))
}
