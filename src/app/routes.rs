use crate::app::state::AppState;
use crate::common::{
    error::{AppError, ErrorData},
    response::ApiResponse,
};
use crate::modules::{auth, user};
use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Json},
    routing::get,
};
use serde::Serialize;
use utoipa::ToSchema;

/// 健康检查路由
pub fn health_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(root_handler))
        .route("/health", get(health_handler))
        .route("/ready", get(readiness_handler))
}

/// API 路由
///
/// 认证中间件加在 `nest` 之后的整个 `/api/v1` 路由组上，而不是逐个接口声明：
/// 这样默认行为就是「需要登录」，新增接口天然受保护，漏写认证不会造成匿名暴露。
/// 例外清单（登录、刷新、登出）集中在 `common::auth::require_login` 中维护。
///
/// 这里显式接收 `AppState`：`nest` 之后内部 Router 的状态类型已经解析为具体类型，
/// 中间件无法再依赖外层的 `with_state` 注入（`from_fn` 要求自行解析 `State`）。
/// 使用 `from_fn_with_state` 把状态绑定到中间件上，`State` 提取才能正常工作。
pub fn api_routes(state: AppState) -> Router<AppState> {
    let api = Router::new()
        .route("/hello", get(hello_handler))
        .merge(auth::routes())
        .merge(user::routes())
        // 用 `route_layer` 而不是 `layer`：它明确只作用于已匹配到路由的请求，
        // 中间件不会因为被降到外层 fallback 而误判不存在的路径。
        .route_layer(axum::middleware::from_fn_with_state(state, crate::common::auth::require_login));

    Router::new().nest("/api/v1", api)
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ServerInfoResponse {
    name: &'static str,
    version: &'static str,
    status: &'static str,
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

#[derive(Debug, Serialize, ToSchema)]
pub struct HealthResponse {
    status: &'static str,
    timestamp: String,
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

#[derive(Debug, Serialize, ToSchema)]
pub struct ReadinessResponse {
    status: &'static str,
    database: &'static str,
    timestamp: String,
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
pub(crate) async fn readiness_handler(State(state): State<AppState>) -> Result<impl IntoResponse, AppError> {
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

#[derive(Debug, Serialize, ToSchema)]
pub struct HelloResponse {
    message: &'static str,
    timestamp: String,
}
/// 示例 API 处理器
///
/// 虽然只是一个示例接口，但它同样位于受保护的 `/api/v1` 之下，
/// 未登录时会被认证中间件拦截并返回 `401`。
#[utoipa::path(
    get,
    path = "/api/v1/hello",
    tag = "系统",
    responses(
        (status = 200, description = "示例接口", body = ApiResponse<HelloResponse>),
        (status = 401, description = "未登录或登录已过期", body = ApiResponse<ErrorData>)
    )
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
