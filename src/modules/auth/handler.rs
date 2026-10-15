use axum::{
    Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Json},
    routing::{get, post},
};
use std::net::SocketAddr;

use crate::app::state::AppState;
use crate::common::{
    auth_extractor::CurrentUser,
    cookie::{self, build_clear_cookie, build_token_cookie},
    error::{AppError, ErrorData},
    extractor::ValidatedJson,
    response::{ApiResponse, EmptyData},
};
use crate::config::CookieConfig;
use crate::modules::auth::{
    model::{AuthSessionResponse, CurrentUserResponse, LoginRequest, TokenPair},
    service::AuthService,
};

/// 认证路由。
///
/// 统一挂载在 `/auth` 下，由 `app::routes` 组合到 `/api/v1` 前缀中。
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/refresh", post(refresh_token))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(current_user))
}

/// 从请求中提取客户端信息，用于安全审计记录。
///
/// 当前只使用 TCP 直连地址，不读取 `X-Forwarded-For`：该头可被客户端任意伪造，
/// 在未确认反向代理受信之前使用它会让 IP 维度的审计与限流形同虚设。
/// 部署在受控代理之后时，应改为读取代理注入的头并同步调整限流维度。
fn client_info(headers: &HeaderMap, connect_info: Option<ConnectInfo<SocketAddr>>) -> ClientInfo {
    // 截断 UA 长度：该字段完全由客户端控制，不设上限会成为存储放大手段。
    const MAX_USER_AGENT_LENGTH: usize = 512;

    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .chars()
                .take(MAX_USER_AGENT_LENGTH)
                .collect::<String>()
        });

    ClientInfo {
        ip: connect_info.map(|ConnectInfo(address)| address.ip().to_string()),
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
fn session_response(
    cookie_config: &CookieConfig,
    tokens: TokenPair,
    refresh_ttl_seconds: i64,
    message: &'static str,
) -> Result<axum::response::Response, AppError> {
    let mut headers = HeaderMap::new();

    // 两个 Cookie 的 Max-Age 必须各自匹配其令牌有效期，不能共用：
    // 若 access token 的 Cookie 沿用刷新令牌的有效期，它会在令牌实际失效后
    // 仍被浏览器持续发送，让服务端反复处理注定失败的请求。
    append_set_cookie(
        &mut headers,
        build_token_cookie(
            cookie_config,
            &cookie_config.access_name,
            &tokens.access_token,
            tokens.expires_in,
        ),
    )?;
    append_set_cookie(
        &mut headers,
        build_token_cookie(
            cookie_config,
            &cookie_config.refresh_name,
            &tokens.refresh_token,
            refresh_ttl_seconds,
        ),
    )?;

    let body = ApiResponse::success(
        message,
        AuthSessionResponse::new(
            tokens.expires_in,
            &cookie_config.access_name,
            &cookie_config.refresh_name,
        ),
    );

    Ok((headers, Json(body)).into_response())
}

/// 构建清除令牌 Cookie 的成功响应。
fn cleared_session_response(
    cookie_config: &CookieConfig,
    message: &'static str,
) -> Result<axum::response::Response, AppError> {
    let mut headers = HeaderMap::new();

    // 无论当前请求带了哪些 Cookie，两个都要清除：
    // 否则未清除的那个令牌会残留在浏览器中继续可用，登出形同未完成。
    append_set_cookie(
        &mut headers,
        build_clear_cookie(cookie_config, &cookie_config.access_name),
    )?;
    append_set_cookie(
        &mut headers,
        build_clear_cookie(cookie_config, &cookie_config.refresh_name),
    )?;

    let body = ApiResponse::<()>::success_without_data(message);

    Ok((headers, Json(body)).into_response())
}

/// 向响应头追加一个 `Set-Cookie` 值。
fn append_set_cookie(headers: &mut HeaderMap, value: String) -> Result<(), AppError> {
    let header_value = HeaderValue::from_str(&value)
        .map_err(|_| AppError::Internal("Set-Cookie 响应头构造失败"))?;
    headers.append(header::SET_COOKIE, header_value);

    Ok(())
}

/// 将刷新令牌的有效期转换为秒。
fn refresh_ttl_seconds(state: &AppState) -> i64 {
    state.jwt.refresh_ttl_days * 24 * 60 * 60
}

/// 用户登录
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
    connect_info: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    ValidatedJson(req): ValidatedJson<LoginRequest>,
) -> Result<impl IntoResponse, AppError> {
    let client = client_info(&headers, connect_info);

    let tokens = AuthService::login(
        &state.db,
        &state.jwt,
        &state.auth,
        &req.username,
        &req.password,
        client.ip.as_deref(),
        client.user_agent.as_deref(),
    )
    .await?;

    session_response(
        &state.auth.cookie,
        tokens,
        refresh_ttl_seconds(&state),
        "登录成功",
    )
}

/// 刷新令牌
#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    tag = "认证",
    responses(
        (status = 200, description = "刷新成功，新令牌通过 Set-Cookie 响应头下发", body = ApiResponse<AuthSessionResponse>),
        (status = 401, description = "刷新令牌无效、已过期或已被使用", body = ApiResponse<ErrorData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn refresh_token(
    State(state): State<AppState>,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let client = client_info(&headers, connect_info);

    let refresh_token = cookie::refresh_token_from_headers(&headers, &state.auth.cookie)
        .ok_or(AppError::Unauthorized("缺少刷新令牌"))?;

    let tokens = AuthService::refresh(
        &state.db,
        &state.jwt,
        &refresh_token,
        client.ip.as_deref(),
        client.user_agent.as_deref(),
    )
    .await?;

    session_response(
        &state.auth.cookie,
        tokens,
        refresh_ttl_seconds(&state),
        "刷新成功",
    )
}

/// 用户登出
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    tag = "认证",
    responses(
        (status = 200, description = "登出成功，令牌 Cookie 已清除", body = ApiResponse<EmptyData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
pub(crate) async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    if let Some(refresh_token) = cookie::refresh_token_from_headers(&headers, &state.auth.cookie) {
        // 先吊销服务端会话；吊销失败只记录日志而不中断，随后仍会清除 Cookie。
        // 让客户端卡在"无法退出"的状态比一次吊销失败更糟，且 Cookie 清除本身
        // 已能阻止该浏览器继续使用令牌。
        if let Err(error) = AuthService::logout(&state.db, &refresh_token).await {
            tracing::warn!(?error, "登出时吊销会话失败，仍继续清除客户端 Cookie");
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
pub(crate) async fn current_user(
    State(state): State<AppState>,
    current_user: CurrentUser,
) -> Result<impl IntoResponse, AppError> {
    let user = AuthService::current_user(&state.db, current_user.user_id).await?;

    Ok(Json(ApiResponse::success("查询成功", user)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cookie_config() -> CookieConfig {
        CookieConfig {
            access_name: "access_token".to_owned(),
            refresh_name: "refresh_token".to_owned(),
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

    /// 登录响应必须同时下发两个 Cookie。
    #[test]
    fn login_sets_both_token_cookies() {
        let config = test_cookie_config();
        let tokens = TokenPair::new("access-value".to_owned(), "refresh-value".to_owned(), 900);

        let response =
            session_response(&config, tokens, 604_800, "登录成功").expect("响应应构造成功");
        let cookies = collect_set_cookies(response.headers());

        assert_eq!(cookies.len(), 2, "应下发两个 Set-Cookie：{cookies:?}");
        assert!(
            cookies
                .iter()
                .any(|c| c.starts_with("access_token=access-value"))
        );
        assert!(
            cookies
                .iter()
                .any(|c| c.starts_with("refresh_token=refresh-value"))
        );
    }

    /// 令牌不得出现在响应体中，否则 HttpOnly 的防护意义会被完全绕过。
    #[tokio::test]
    async fn login_response_body_contains_no_token() {
        use http_body_util::BodyExt;

        let config = test_cookie_config();
        let tokens = TokenPair::new("access-value".to_owned(), "refresh-value".to_owned(), 900);

        let response =
            session_response(&config, tokens, 604_800, "登录成功").expect("响应应构造成功");
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("响应体应可读取")
            .to_bytes();
        let body = String::from_utf8(bytes.to_vec()).expect("响应体应为 UTF-8");

        assert!(
            !body.contains("access-value"),
            "响应体不应包含 access token：{body}"
        );
        assert!(
            !body.contains("refresh-value"),
            "响应体不应包含 refresh token：{body}"
        );
        assert!(
            body.contains("\"token_type\":\"cookie\""),
            "应声明 Cookie 认证：{body}"
        );
    }

    /// 两个 Cookie 的有效期必须各自匹配其令牌。
    #[test]
    fn cookies_use_matching_max_age() {
        let config = test_cookie_config();
        let tokens = TokenPair::new("access-value".to_owned(), "refresh-value".to_owned(), 900);

        let response =
            session_response(&config, tokens, 604_800, "登录成功").expect("响应应构造成功");
        let cookies = collect_set_cookies(response.headers());

        let access = cookies
            .iter()
            .find(|c| c.starts_with("access_token="))
            .expect("应存在 access Cookie");
        let refresh = cookies
            .iter()
            .find(|c| c.starts_with("refresh_token="))
            .expect("应存在 refresh Cookie");

        assert!(
            access.contains("Max-Age=900"),
            "access 有效期应为其自身：{access}"
        );
        assert!(
            refresh.contains("Max-Age=604800"),
            "refresh 有效期应为 7 天：{refresh}"
        );
    }

    /// 登出必须清除两个 Cookie，避免其中一个残留导致会话未真正结束。
    #[test]
    fn logout_clears_both_token_cookies() {
        let config = test_cookie_config();

        let response = cleared_session_response(&config, "登出成功").expect("响应应构造成功");
        let cookies = collect_set_cookies(response.headers());

        assert_eq!(cookies.len(), 2, "应清除两个 Cookie：{cookies:?}");
        for cookie in &cookies {
            assert!(cookie.contains("Max-Age=0"), "清除必须立即生效：{cookie}");
            assert!(
                cookie.contains("HttpOnly"),
                "清除属性需与下发一致：{cookie}"
            );
        }
    }

    /// secure 配置必须真正传递到响应头，否则生产环境会静默地以明文发送令牌。
    #[test]
    fn secure_config_is_applied_to_issued_cookies() {
        let mut config = test_cookie_config();
        config.secure = true;

        let tokens = TokenPair::new("access-value".to_owned(), "refresh-value".to_owned(), 900);
        let response =
            session_response(&config, tokens, 604_800, "登录成功").expect("响应应构造成功");

        for cookie in collect_set_cookies(response.headers()) {
            assert!(
                cookie.contains("Secure"),
                "启用 secure 后必须设置：{cookie}"
            );
        }
    }
}
