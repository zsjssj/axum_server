pub mod middleware;
pub mod routes;
pub mod state;

use crate::app::middleware::{cors_layer, logging_middleware};
use crate::app::state::AppState;
use axum::Router;
use sqlx::PgPool;

/// 创建 Axum 应用
pub fn create_app(db_pool: PgPool) -> Router {
    let state = AppState::new(db_pool);

    Router::new()
        .merge(routes::health_routes())
        .merge(routes::api_routes())
        .layer(cors_layer())
        .layer(axum::middleware::from_fn(logging_middleware))
        .with_state(state)
}
