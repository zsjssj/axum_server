use crate::app::state::AppState;
use crate::common::{
    error::{AppError, ErrorData},
    response::ApiResponse,
};
use crate::modules::user;
use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Json},
    routing::get,
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct ServerInfoResponse {
    name: &'static str,
    version: &'static str,
    status: &'static str,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HealthResponse {
    status: &'static str,
    timestamp: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct HelloResponse {
    message: &'static str,
    timestamp: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReadinessResponse {
    status: &'static str,
    database: &'static str,
    timestamp: String,
}

/// 健康检查路由
pub fn health_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(root_handler))
        .route("/health", get(health_handler))
        .route("/ready", get(readiness_handler))
}

/// API 路由
pub fn api_routes() -> Router<AppState> {
    Router::new().nest(
        "/api/v1",
        Router::new()
            .route("/hello", get(hello_handler))
            .merge(user::routes()),
    )
}

/// 根路径处理器
#[utoipa::path(
    get,
    path = "/",
    tag = "系统",
    responses((status = 200, description = "服务基本信息", body = ApiResponse<ServerInfoResponse>))
)]
pub(crate) async fn root_handler() -> impl IntoResponse {
    Json(ApiResponse::success(
        "请求成功",
        ServerInfoResponse {
            name: "Rust Axum Server",
            version: "0.1.0",
            status: "running",
        },
    ))
}

/// 健康检查处理器
#[utoipa::path(
    get,
    path = "/health",
    tag = "系统",
    responses((status = 200, description = "服务进程健康", body = ApiResponse<HealthResponse>))
)]
pub(crate) async fn health_handler() -> impl IntoResponse {
    Json(ApiResponse::success(
        "请求成功",
        HealthResponse {
            status: "healthy",
            timestamp: chrono::Utc::now().to_rfc3339(),
        },
    ))
}

/// 就绪检查处理器（包含数据库检查）
#[utoipa::path(
    get,
    path = "/ready",
    tag = "系统",
    responses(
        (status = 200, description = "服务与数据库均已就绪", body = ApiResponse<ReadinessResponse>),
        (status = 503, description = "数据库不可用", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn readiness_handler(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => Ok(Json(ApiResponse::success(
            "请求成功",
            ReadinessResponse {
                status: "ready",
                database: "connected",
                timestamp: chrono::Utc::now().to_rfc3339(),
            },
        ))),
        Err(error) => {
            tracing::warn!(%error, "数据库就绪检查失败");
            Err(AppError::ServiceUnavailable("数据库不可用"))
        }
    }
}

/// 示例 API 处理器
#[utoipa::path(
    get,
    path = "/api/v1/hello",
    tag = "系统",
    responses((status = 200, description = "示例接口", body = ApiResponse<HelloResponse>))
)]
pub(crate) async fn hello_handler() -> impl IntoResponse {
    Json(ApiResponse::success(
        "请求成功",
        HelloResponse {
            message: "Hello from Rust Axum Server!",
            timestamp: chrono::Utc::now().to_rfc3339(),
        },
    ))
}

/// 将未匹配路由转换为统一的 JSON 错误响应。
pub(crate) async fn not_found_handler() -> AppError {
    AppError::NotFound("接口不存在")
}

/// 将不支持的 HTTP 方法转换为统一的 JSON 错误响应。
pub(crate) async fn method_not_allowed_handler() -> AppError {
    AppError::MethodNotAllowed
}
