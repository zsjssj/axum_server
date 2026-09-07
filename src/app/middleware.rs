use axum::{extract::Request, middleware::Next, response::Response};
use std::time::Instant;
use tower_http::cors::{Any, CorsLayer};

/// 日志中间件
pub async fn logging_middleware(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let uri = request.uri().clone();
    let start = Instant::now();

    let response = next.run(request).await;

    let duration = start.elapsed();
    let status = response.status();

    tracing::info!(
        %method,
        %uri,
        status = status.as_u16(),
        duration_ms = duration.as_secs_f64() * 1000.0,
        "request completed"
    );

    response
}

/// CORS 配置
pub fn cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
}
