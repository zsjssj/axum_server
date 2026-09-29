use axum::{
    body::Body,
    extract::Request,
    http::{Method, Response as ResponseH, Uri, status},
    middleware::Next,
    response::Response,
};
use std::time::{Duration, Instant};
use tower_http::cors::{Any, CorsLayer};

/// 日志中间件
pub async fn logging_middleware(request: Request, next: Next) -> Response {
    let method: Method = request.method().clone(); //获取请求方式
    let uri: Uri = request.uri().clone(); //获取请求地址
    let start: Instant = Instant::now(); //获取请求时间

    let response: ResponseH<Body> = next.run(request).await; //获取响应体
    let duration: Duration = start.elapsed(); //获取响应时间
    let status: status::StatusCode = response.status(); //获取响应状态

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
