use axum::{
    Router,
    extract::State,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Json},
    routing::{get, post},
};
use std::net::SocketAddr;

use crate::app::state::AppState;
use crate::common::{
    auth::{LOGIN_PATH, LOGOUT_PATH},
    auth_extractor::CurrentUser,
    cookie::{self, build_clear_cookie, build_token_cookie},
    error::{AppError, ErrorData},
    extractor::{ClientAddr, ValidatedJson},
    response::{ApiResponse, EmptyData},
};
use crate::config::CookieConfig;
use crate::modules::auth::{
    model::{AuthSessionResponse, CurrentUserResponse, IssuedToken, LoginRequest},
    service::{AuthService, LoginContext},
};

/// 认证路由。
///
/// 统一挂载在 `app::routes::api_routes` 的 `/api/v1` 前缀之下；这里使用相对路径，
/// 其中两个认证入口直接引用 `common::auth` 中的白名单常量，使「注册的路由」与
/// 「匿名放行的白名单」共用同一个定义——两处分叉会直接导致客户端无法登录，
/// 或某个受保护路径被意外放行。
///
/// 单令牌方案只提供登录、登出与当前用户三个入口：没有 `/auth/refresh`，
/// 令牌到期后服务端返回 `401`，客户端据 `expires_in` 或该响应引导用户重新登录。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(LOGIN_PATH, post(login))
        .route(LOGOUT_PATH, post(logout))
        .route("/auth/me", get(current_user))
}

/// 从请求中提取客户端信息，用于安全审计记录。
///
/// 当前只使用 TCP 直连地址，不读取 `X-Forwarded-For`：该头可被客户端任意伪造，
/// 在未确认反向代理受信之前使用它会让 IP 维度的审计与限流形同虚设。
/// 部署在受控代理之后时，应改为读取代理注入的头并同步调整限流维度。
///
/// 连接地址由 [`ClientAddr`] 提供：axum 0.8 的 `Option<T>` 不再接受 `ConnectInfo`，
/// 该提取器把「可能取不到对端地址」的语义显式化，取不到时审计字段为 `None`。
fn client_info(headers: &HeaderMap, connect_info: Option<SocketAddr>) -> ClientInfo {
    // 截断 UA 长度：该字段完全由客户端控制，不设上限会成为存储放大手段。
    const MAX_USER_AGENT_LENGTH: usize = 512;
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(MAX_USER_AGENT_LENGTH).collect::<String>());
    ClientInfo {
        ip: connect_info.map(|address| address.ip().to_string()),
        user_agent,
    }
}

/// 请求来源信息，仅用于审计。
struct ClientInfo {
    ip: Option<String>,
    user_agent: Option<String>,
}

/// 构建携带令牌 Cookie 的成功响应。
///
/// 令牌只出现在 `Set-Cookie` 响应头中，响应体不含令牌本身。
/// `Max-Age` 直接取自令牌剩余有效期：单令牌方案下 Cookie 的生命周期与令牌
/// 完全一致，不存在「Cookie 还在、令牌已失效」的中间状态。
fn session_response(cookie_config: &CookieConfig, issued: IssuedToken, message: &'static str) -> Result<axum::response::Response, AppError> {
    let mut headers = HeaderMap::new();
    append_set_cookie(&mut headers, build_token_cookie(cookie_config, &issued.token, issued.expires_in))?;
    let body = ApiResponse::success(message, AuthSessionResponse::new(issued.expires_in));
    Ok((headers, Json(body)).into_response())
}

/// 构建清除令牌 Cookie 的成功响应。
fn cleared_session_response(cookie_config: &CookieConfig, message: &'static str) -> Result<axum::response::Response, AppError> {
    let mut headers = HeaderMap::new();
    append_set_cookie(&mut headers, build_clear_cookie(cookie_config))?;
    let body = ApiResponse::<()>::success_without_data(message);
    Ok((headers, Json(body)).into_response())
}

/// 向响应头追加一个 `Set-Cookie` 值。
fn append_set_cookie(headers: &mut HeaderMap, value: String) -> Result<(), AppError> {
    let header_value = HeaderValue::from_str(&value).map_err(|_| AppError::Internal("Set-Cookie 响应头构造失败"))?;
    headers.append(header::SET_COOKIE, header_value);
    Ok(())
}

/// 用户登录
///
/// 成功时签发唯一的登录令牌，并通过 `Set-Cookie` 下发；响应体只包含
/// 「令牌还有多少秒到期」，客户端据此决定何时提示用户重新登录。
#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
    tag = "认证",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "登录成功，令牌通过 Set-Cookie 响应头下发", body = ApiResponse<AuthSessionResponse>),
        (status = 400, description = "JSON 请求体格式错误", body = ApiResponse<ErrorData>),
        (status = 401, description = "用户名或密码错误", body = ApiResponse<ErrorData>),
        (status = 415, description = "请求体媒体类型错误", body = ApiResponse<ErrorData>),
        (status = 422, description = "请求参数校验失败", body = ApiResponse<ErrorData>),
        (status = 429, description = "登录失败次数过多，暂时拒绝", body = ApiResponse<ErrorData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn login(
    State(state): State<AppState>,
    client_addr: ClientAddr,
    headers: HeaderMap,
    ValidatedJson(req): ValidatedJson<LoginRequest>,
) -> Result<impl IntoResponse, AppError> {
    let client = client_info(&headers, client_addr.0);
    let issued = AuthService::login(
        &state.db,
        &state.redis,
        &state.jwt,
        &state.auth,
        &req.username,
        &req.password,
        LoginContext {
            client_ip: client.ip.as_deref(),
            user_agent: client.user_agent.as_deref(),
        },
    )
    .await?;
    session_response(&state.auth.cookie, issued, "登录成功")
}

/// 用户登出
///
/// 单令牌方案下需要撤销两类状态，顺序是先标记会话审计记录，再把令牌写入
/// Redis 吊销列表：
///
/// - 会话记录落库，标记吊销后可以回答「这次会话何时结束」；
/// - 令牌本身无状态，只能借助 Redis 吊销列表在自然过期前失效。
///
/// 令牌是唯一的凭据，因此这两步都以同一个 Cookie 为输入：Cookie 缺失时
/// 没有任何东西需要吊销，直接清除即可返回成功。
///
/// 两次吊销都是「尽力而为」：任何一步失败都只记录告警并继续清除 Cookie。
/// 让客户端卡在"无法退出"的状态比一次吊销失败更糟，而且 Cookie 清除本身
/// 已经能阻止该浏览器继续使用令牌。
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "认证",
    responses(
        (status = 200, description = "登出成功，令牌 Cookie 已清除", body = ApiResponse<EmptyData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<impl IntoResponse, AppError> {
    if let Some(token) = cookie::token_from_headers(&headers, &state.auth.cookie) {
        if let Err(error) = AuthService::logout(&state.db, &token).await {
            tracing::warn!(?error, "登出时吊销会话审计记录失败，仍继续清除客户端 Cookie");
        }
        if let Err(error) = AuthService::revoke_access_token(&state.redis, &state.jwt, &token).await {
            tracing::warn!(?error, "登出时写入令牌吊销列表失败，令牌将在自然过期前继续有效");
        }
    }
    // 即使没有令牌也返回成功：登出是幂等操作，
    // 重复调用或 Cookie 已缺失都不应让客户端收到错误。
    cleared_session_response(&state.auth.cookie, "登出成功")
}

/// 获取当前登录用户
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
    tag = "认证",
    responses(
        (status = 200, description = "当前登录用户资料", body = ApiResponse<CurrentUserResponse>),
        (status = 401, description = "未提供令牌或令牌无效", body = ApiResponse<ErrorData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn current_user(State(state): State<AppState>, current_user: CurrentUser) -> Result<impl IntoResponse, AppError> {
    let user = AuthService::current_user(&state.db, current_user.user_id).await?;
    Ok(Json(ApiResponse::success("查询成功", user)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cookie_config() -> CookieConfig {
        CookieConfig {
            name: "access_token".to_owned(),
            secure: false,
            same_site: "strict".to_owned(),
            path: "/".to_owned(),
        }
    }

    fn collect_set_cookies(headers: &HeaderMap) -> Vec<String> {
        headers
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().expect("响应头应为合法 ASCII").to_owned())
            .collect()
    }

    /// 登录响应只下发一个 Cookie：单令牌方案下不存在第二个凭据。
    #[test]
    fn login_sets_a_single_token_cookie() {
        let config = test_cookie_config();
        let issued = IssuedToken::new("token-value".to_owned(), 604_800);

        let response = session_response(&config, issued, "登录成功").expect("响应应构造成功");
        let cookies = collect_set_cookies(response.headers());

        assert_eq!(cookies.len(), 1, "应只下发一个 Set-Cookie：{cookies:?}");
        assert!(cookies[0].starts_with("access_token=token-value"));
    }

    /// 令牌不得出现在响应体中，否则 HttpOnly 的防护意义会被完全绕过。
    #[tokio::test]
    async fn login_response_body_contains_no_token() {
        use http_body_util::BodyExt;

        let config = test_cookie_config();
        let issued = IssuedToken::new("token-value".to_owned(), 604_800);

        let response = session_response(&config, issued, "登录成功").expect("响应应构造成功");
        let bytes = response.into_body().collect().await.expect("响应体应可读取").to_bytes();
        let body = String::from_utf8(bytes.to_vec()).expect("响应体应为 UTF-8");

        assert!(!body.contains("token-value"), "响应体不应包含令牌：{body}");
        assert!(body.contains("\"token_type\":\"cookie\""), "应声明 Cookie 认证：{body}");
        assert!(body.contains("\"expires_in\":604800"), "应回传令牌剩余有效期：{body}");
    }

    /// Cookie 的 `Max-Age` 必须与令牌有效期一致：
    /// 否则浏览器会在令牌失效后继续发送它，或提前丢弃仍然有效的令牌。
    #[test]
    fn cookie_uses_token_expiry_as_max_age() {
        let config = test_cookie_config();
        let issued = IssuedToken::new("token-value".to_owned(), 604_800);

        let response = session_response(&config, issued, "登录成功").expect("响应应构造成功");
        let cookies = collect_set_cookies(response.headers());

        assert!(cookies[0].contains("Max-Age=604800"), "有效期应等于令牌剩余时间：{}", cookies[0]);
    }

    /// 登出必须清除令牌 Cookie，且清除属性与下发时一致。
    #[test]
    fn logout_clears_token_cookie() {
        let config = test_cookie_config();

        let response = cleared_session_response(&config, "登出成功").expect("响应应构造成功");
        let cookies = collect_set_cookies(response.headers());

        assert_eq!(cookies.len(), 1, "应只清除一个 Cookie：{cookies:?}");
        assert!(cookies[0].contains("Max-Age=0"), "清除必须立即生效：{}", cookies[0]);
        assert!(cookies[0].contains("HttpOnly"), "清除属性需与下发一致：{}", cookies[0]);
    }

    /// secure 配置必须真正传递到响应头，否则生产环境会静默地以明文发送令牌。
    #[test]
    fn secure_config_is_applied_to_issued_cookies() {
        let mut config = test_cookie_config();
        config.secure = true;

        let issued = IssuedToken::new("token-value".to_owned(), 604_800);
        let response = session_response(&config, issued, "登录成功").expect("响应应构造成功");

        for cookie in collect_set_cookies(response.headers()) {
            assert!(cookie.contains("Secure"), "启用 secure 后必须设置：{cookie}");
        }
    }
}
