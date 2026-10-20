//! 接口登录校验：认证中间件与公开路径白名单。
//!
//! 采用「默认拒绝 + 显式白名单」策略：`/api/v1` 下的全部接口都需要登录，
//! 只有白名单中列出的接口允许匿名访问。选择中间件而不是逐个 Handler 声明
//! `CurrentUser`，是因为前者把「新增接口自动受保护」变成了默认行为——
//! 漏写一处认证只会在评审时被发现，而默认拒绝的失效方向是安全的。
//!
//! 白名单与中间件放在同一处维护：两者分离时，新增公开接口容易只改路由而忘记
//! 同步例外列表，最终得到一个「本以为受保护、实际匿名可访问」的接口。

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};

use crate::app::state::AppState;
use crate::common::cookie;
use crate::common::error::AppError;
use crate::modules::auth::service::AuthService;

/// `/api/v1` 路由组的挂载前缀。
///
/// 认证中间件挂在 `nest` 之后，而 `nest` 会剥离前缀，因此中间件看到的路径是
/// `/auth/login` 这样的相对路径。把前缀抽成常量，使「白名单写的是哪一段路径」
/// 这件事在代码里有唯一定义，避免有人按完整路径写白名单后得到
/// 「所有接口都要求登录」或「所有接口都放行」这类静默错误。
pub const API_PREFIX: &str = "/api/v1";
/// 登录接口路径，匿名可访问。
pub const LOGIN_PATH: &str = "/auth/login";
/// 刷新令牌接口路径，匿名可访问：access token 过期后只能凭刷新 Cookie 换取新令牌。
pub const REFRESH_PATH: &str = "/auth/refresh";
/// 登出接口路径，匿名可访问：令牌失效时仍必须能清理会话。
pub const LOGOUT_PATH: &str = "/auth/logout";

/// 无需登录即可访问的相对路径白名单。
///
/// 清单刻意保持最小，每一项都有不可移除的理由：
///
/// - [`LOGIN_PATH`]：登录本身必须匿名可访问，否则无法获得凭证。
/// - [`REFRESH_PATH`]：access token 过期后客户端只剩刷新 Cookie，
///   若要求先有有效 access token，刷新流程将永远无法启动。
/// - [`LOGOUT_PATH`]：登出必须幂等，令牌已过期或 Cookie 已丢失时
///   仍要能清理服务端会话。要求登录才能登出会让用户陷入无法退出登录的状态。
///
/// 注册路由时使用上面这些常量拼接，就不会出现「路由改了、白名单没改」的偏差。
const PUBLIC_PATHS: [&str; 3] = [LOGIN_PATH, REFRESH_PATH, LOGOUT_PATH];

/// 认证通过后写入请求扩展的用户身份。
///
/// 只保存内部主键：`public_id`、用户名等资料由 Handler 按需查询，
/// 避免中间件为了填充扩展而多做一次数据库查询。
#[derive(Debug, Clone, Copy)]
pub struct AuthUser {
    pub user_id: i64,
}

/// 判断请求路径是否属于匿名可访问的白名单。
///
/// 同时接受完整路径与 `nest` 剥离前缀后的路径：前者便于按接口文档核对白名单，
/// 后者是中间件在真实请求中实际看到的形态。匹配是绝对相等而不是前缀比较——
/// 前缀比较会让 `/auth/login/extra` 这类路径一并放行，
/// 一旦将来出现同名子路由就会形成匿名入口。
fn is_public_path(path: &str) -> bool {
    let relative_path = path.strip_prefix(API_PREFIX).unwrap_or(path);
    PUBLIC_PATHS.contains(&relative_path)
}

/// 校验请求中的 access token，返回用户身份。
///
/// 令牌只从 Cookie 读取：`Authorization` 头不被解析，避免出现绕过 Cookie
/// 安全属性（`HttpOnly`、`SameSite`）的旁路认证方式。
///
/// 签名校验通过后还要查询 Redis 吊销列表：登出会把 access token 的 `jti`
/// 写入该列表，使令牌不必等到自然过期才失效。查询失败时按「未吊销」放行并
/// 记录告警——吊销是「登出立即生效」的增强，而不是令牌有效性的前提；
/// Redis 故障时退化为改动前的行为（令牌到期才失效），比把全部已登录用户
/// 一次性踢下线更合理。
async fn authenticate(state: &AppState, headers: &axum::http::HeaderMap) -> Result<AuthUser, AppError> {
    let token = cookie::access_token_from_headers(headers, &state.auth.cookie).ok_or(AppError::Unauthorized("未登录或登录已过期"))?;
    let verified = AuthService::verify_access_token(&state.jwt, &token)?;
    match state.redis.is_access_token_revoked(&verified.jti).await {
        Ok(true) => {
            tracing::debug!(jti = %verified.jti, "access token 已在吊销列表中");
            return Err(AppError::Unauthorized("认证令牌已失效，请重新登录"));
        }
        Ok(false) => {}
        Err(error) => tracing::warn!(%error, "查询令牌吊销列表失败，按未吊销处理"),
    }
    Ok(AuthUser { user_id: verified.user_id })
}

/// 登录校验中间件。
///
/// 白名单以外的请求必须携带有效的 `access_token` Cookie，否则直接返回 `401`，
/// 不进入任何 Handler。校验通过后把用户身份写入请求扩展，供 `CurrentUser` 提取器读取。
pub async fn require_login(State(state): State<AppState>, request: Request, next: Next) -> Result<Response, AppError> {
    if is_public_path(request.uri().path()) {
        return Ok(next.run(request).await);
    }
    let user = authenticate(&state, request.headers()).await?;
    // 拆分请求后重新组装，唯一目的是在扩展中写入身份再交给后续处理链。
    let (mut parts, body) = request.into_parts();
    parts.extensions.insert(user);
    Ok(next.run(Request::from_parts(parts, body)).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 登录、刷新、登出必须匿名可访问，否则客户端无法进入认证流程。
    #[test]
    fn allows_auth_entrypoints_without_token() {
        assert!(is_public_path("/auth/login"));
        assert!(is_public_path("/auth/refresh"));
        assert!(is_public_path("/auth/logout"));
    }

    /// `nest` 之前的完整路径同样应被识别，便于按接口文档核对白名单。
    #[test]
    fn also_matches_full_paths_with_api_prefix() {
        assert!(is_public_path("/api/v1/auth/login"));
        assert!(is_public_path("/api/v1/auth/refresh"));
        assert!(is_public_path("/api/v1/auth/logout"));
    }

    /// 业务接口与示例接口都不在白名单内，必须登录后才能访问。
    #[test]
    fn protects_business_endpoints() {
        assert!(!is_public_path("/users"));
        assert!(!is_public_path("/users/01991f3a-7c40-7d62-8f6d-4fb743e0f731"));
        assert!(!is_public_path("/auth/me"));
        assert!(!is_public_path("/hello"));
    }

    /// 前缀匹配不能变成「只要路径里有 auth 就放行」。
    #[test]
    fn does_not_allow_prefix_lookalikes() {
        assert!(!is_public_path("/auth/login/extra"));
        assert!(!is_public_path("/auth/logins"));
        assert!(!is_public_path("/auth/loginx"));
    }

    /// 不属于 `/api/v1` 的同类路径不能误命中白名单：
    /// 该前缀之外挂载的接口（如健康检查）不在本中间件的管辖范围内。
    #[test]
    fn unknown_prefix_is_not_treated_as_public() {
        assert!(!is_public_path("/other/auth/login"));
    }

    mod middleware {
        use super::*;
        use std::sync::Arc;

        use axum::{Router, body::Body, http::StatusCode, routing::get};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        use crate::common::auth_extractor::CurrentUser;
        use crate::config::{AuthConfig, CookieConfig, JwtConfig};

        fn test_state() -> AppState {
            AppState {
                db: sqlx::postgres::PgPoolOptions::new()
                    .connect_lazy("postgres://user:password@localhost/test")
                    .expect("测试数据库连接字符串应合法"),
                // 不建立连接的实例：用例只验证令牌校验与身份传递。
                // Redis 不可达时吊销检查走「按未吊销处理」的降级分支，
                // 因此这里同时也覆盖了 Redis 故障时的认证行为。
                redis: crate::infrastructure::redis::RedisStore::without_connection(),
                jwt: Arc::new(JwtConfig {
                    secret: "unit-test-secret-with-enough-length-32".to_owned(),
                    issuer: "rust-axum-server-test".to_owned(),
                    access_ttl_minutes: 15,
                    refresh_ttl_days: 7,
                }),
                auth: Arc::new(AuthConfig {
                    max_login_failures: 5,
                    login_window_minutes: 15,
                    cookie: CookieConfig {
                        access_name: "access_token".to_owned(),
                        refresh_name: "refresh_token".to_owned(),
                        secure: false,
                        same_site: "strict".to_owned(),
                        path: "/".to_owned(),
                    },
                }),
            }
        }

        /// 受保护接口的最小复现：返回提取到的用户 ID。
        async fn identity_handler(CurrentUser { user_id }: CurrentUser) -> String {
            user_id.to_string()
        }

        /// 复现真实的挂载方式：`nest` 剥离 `/api/v1` 前缀后再套认证中间件。
        fn test_app(state: AppState) -> Router {
            let api = Router::new()
                .route("/protected", get(identity_handler))
                .route(LOGIN_PATH, get(|| async { "login" }))
                .route_layer(axum::middleware::from_fn_with_state(state.clone(), require_login));

            Router::new().nest(API_PREFIX, api).with_state(state)
        }

        async fn request(state: AppState, uri: &str, cookie: Option<&str>) -> (StatusCode, String) {
            let mut builder = axum::http::Request::builder().uri(uri);
            if let Some(cookie) = cookie {
                builder = builder.header(axum::http::header::COOKIE, cookie);
            }

            let response = test_app(state)
                .oneshot(builder.body(Body::empty()).expect("测试请求应构造成功"))
                .await
                .expect("测试请求应成功进入 Router");

            let status = response.status();
            let bytes = response.into_body().collect().await.expect("响应体应可读取").to_bytes();

            (status, String::from_utf8_lossy(&bytes).into_owned())
        }

        /// 中间件校验令牌，并把身份传递给 `CurrentUser` 提取器。
        ///
        /// 这是两个组件之间唯一的契约：中间件负责写入扩展、提取器负责读取。
        /// 任一侧改动而另一侧没跟上，都会让所有受保护接口立刻失败，
        /// 因此必须有用例把这条链路固定下来。
        #[tokio::test]
        async fn passes_authenticated_identity_to_handler() {
            let state = test_state();
            let token = AuthService::issue_test_access_token(&state.jwt, 42, 4_102_444_800);

            let (status, body) = request(state, "/api/v1/protected", Some(&format!("access_token={token}"))).await;

            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, "42", "Handler 应拿到令牌中的用户 ID");
        }

        /// 未登录时在进入 Handler 之前就被拦下，响应为统一信封。
        #[tokio::test]
        async fn blocks_request_without_token() {
            let (status, body) = request(test_state(), "/api/v1/protected", None).await;

            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert!(body.contains("40100"), "应返回未认证业务码：{body}");
        }

        /// 白名单路径不需要令牌即可到达 Handler。
        #[tokio::test]
        async fn lets_public_path_through() {
            let (status, body) = request(test_state(), "/api/v1/auth/login", None).await;

            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, "login");
        }
    }
}
