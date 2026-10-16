#![allow(unused_imports)]

use axum::{
    body::Body,
    extract::Request,
    http::{Method, Response as ResponseH, Uri, status},
    middleware::Next,
    response::Response,
};
use std::time::{Duration, Instant};
use tower_http::{
    classify::{ServerErrorsAsFailures, SharedClassifier},
    cors::{Any, CorsLayer},
};

use tower_http::{
    LatencyUnit,
    trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

/// 日志中间件
///
/// 早期的手写实现已由 `tower-http` 的 `TraceLayer` 取代，保留其形态仅作参考：
///
/// ```ignore
/// pub async fn logging_middleware(request: Request, next: Next) -> Response {
///     let method: Method = request.method().clone(); //获取请求方式
///     let uri: Uri = request.uri().clone(); //获取请求地址
///     let start: Instant = Instant::now(); //获取请求时间
///
///     let response: ResponseH<Body> = next.run(request).await; //获取响应体
///     let duration: Duration = start.elapsed(); //获取响应时间
///     let status: status::StatusCode = response.status(); //获取响应状态
///
///     tracing::info!(
///         %method,
///         %uri,
///         status = status.as_u16(),
///         duration_ms = duration.as_secs_f64() * 1000.0,
///         "request completed"
///     );
///     response
/// }
/// ```
pub fn logging_middleware() -> TraceLayer<SharedClassifier<ServerErrorsAsFailures>> {
    TraceLayer::new_for_http()
        .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
        .on_response(DefaultOnResponse::new().level(Level::INFO).latency_unit(LatencyUnit::Millis))
}

/// CORS 配置
///
/// 当前放行任意来源，仅适用于开发联调。接入 Cookie 认证后必须注意一个约束：
/// 浏览器的凭据模式（`credentials: 'include'`）**不接受通配来源**，服务端也不能
/// 同时使用 `Any` 与 `allow_credentials(true)`，否则响应会被浏览器拒绝。
///
/// 因此前后端跨源部署时，必须把 `allow_origin` 改为具体的前端来源并启用
/// `allow_credentials(true)`；同源部署（前端由本服务或同一域名提供）则不受影响。
pub fn cors_layer() -> CorsLayer {
    CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any)
}
