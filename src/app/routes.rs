use crate::app::state::AppState;
use crate::modules::user;
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::get,
};
use serde_json::json;

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
async fn root_handler() -> impl IntoResponse {
    Json(json!({
        "name": "Rust Axum Server",
        "version": "0.1.0",
        "status": "running"
    }))
}

/// 健康检查处理器
async fn health_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "status": "healthy",
            "timestamp": chrono::Utc::now().to_rfc3339()
        })),
    )
}

/// 就绪检查处理器（包含数据库检查）
async fn readiness_handler(State(state): State<AppState>) -> impl IntoResponse {
    match sqlx::query("SELECT 1").execute(&state.db).await {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({
                "status": "ready",
                "database": "connected",
                "timestamp": chrono::Utc::now().to_rfc3339()
            })),
        ),
        Err(error) => {
            tracing::warn!(%error, "database readiness check failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "status": "not ready",
                    "database": "disconnected",
                    "timestamp": chrono::Utc::now().to_rfc3339()
                })),
            )
        }
    }
}

/// 示例 API 处理器
async fn hello_handler() -> impl IntoResponse {
    Json(json!({
        "message": "Hello from Rust Axum Server!",
        "timestamp": chrono::Utc::now().to_rfc3339()
    }))
}
