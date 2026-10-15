use axum::http::HeaderMap;
use cookie::Cookie;

use crate::config::CookieConfig;

/// 从请求 Cookie 中读取 access token。
pub fn access_token_from_headers(headers: &HeaderMap, config: &CookieConfig) -> Option<String> {
    cookie_value(headers, &config.access_name)
}

/// 从请求 Cookie 中读取 refresh token。
pub fn refresh_token_from_headers(headers: &HeaderMap, config: &CookieConfig) -> Option<String> {
    cookie_value(headers, &config.refresh_name)
}

/// 按名称读取单个 Cookie 的值。
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    // 同名 Cookie 可能因路径不同而重复出现，取第一个匹配项，
    // 与大多数服务端实现的既有行为保持一致。
    for header_value in headers.get_all(axum::http::header::COOKIE) {
        let Ok(raw) = header_value.to_str() else {
            // 单个请求头不是合法 ASCII 时跳过它，而不是让整个认证失败：
            // 其他 Cookie 中仍可能包含有效令牌。
            continue;
        };

        for cookie in cookie::Cookie::split_parse(raw.to_owned()).flatten() {
            if cookie.name() == name {
                return Some(cookie.value().to_owned());
            }
        }
    }

    None
}

/// 构建下发令牌的 `Set-Cookie` 头值。
///
/// 安全属性逐项说明：
///
/// - `HttpOnly`：禁止页面脚本读取令牌，使 XSS 无法直接窃取凭据长期冒用。
/// - `Secure`：仅在 HTTPS 下发送，由配置控制以便本地 HTTP 调试。
/// - `SameSite`：禁止浏览器在跨站请求中携带令牌；这是本方案防 CSRF 的主要手段。
/// - `Path`：限制 Cookie 的发送范围。
///
/// 刻意不设置 `Domain`：省略时浏览器按"仅当前主机"处理，可避免令牌被发送到
/// 同一父域下的其他子域。
pub fn build_token_cookie(
    config: &CookieConfig,
    name: &str,
    value: &str,
    max_age_seconds: i64,
) -> String {
    Cookie::build((name.to_owned(), value.to_owned()))
        .path(config.path.clone())
        .http_only(true)
        .secure(config.secure)
        .same_site(same_site(config))
        .max_age(cookie::time::Duration::seconds(max_age_seconds))
        .build()
        .to_string()
}

/// 构建清除令牌的 `Set-Cookie` 头值。
///
/// 除空值与 `Max-Age=0` 外，其余属性必须与下发时完全一致：浏览器按
/// 名称、域和路径匹配 Cookie，属性不一致会导致清除失败、令牌残留在客户端。
pub fn build_clear_cookie(config: &CookieConfig, name: &str) -> String {
    Cookie::build((name.to_owned(), String::new()))
        .path(config.path.clone())
        .http_only(true)
        .secure(config.secure)
        .same_site(same_site(config))
        // 置零立即过期，并保持与下发时一致的属性以便正确定位到同一个 Cookie。
        .max_age(cookie::time::Duration::seconds(0))
        .build()
        .to_string()
}

/// 将配置中的 SameSite 取值映射为 `cookie` crate 的枚举。
fn same_site(config: &CookieConfig) -> cookie::SameSite {
    if config.same_site_attribute() == "Lax" {
        cookie::SameSite::Lax
    } else {
        cookie::SameSite::Strict
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn test_config() -> CookieConfig {
        CookieConfig {
            access_name: "access_token".to_owned(),
            refresh_name: "refresh_token".to_owned(),
            secure: false,
            same_site: "strict".to_owned(),
            path: "/".to_owned(),
        }
    }

    fn headers_with_cookie(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_str(value).expect("测试 Cookie 头应合法"),
        );
        headers
    }

    #[test]
    fn parses_access_token_cookie() {
        let headers = headers_with_cookie("access_token=abc.def.ghi; other=1");

        assert_eq!(
            access_token_from_headers(&headers, &test_config()).as_deref(),
            Some("abc.def.ghi")
        );
    }

    #[test]
    fn parses_refresh_token_cookie() {
        let headers = headers_with_cookie("refresh_token=deadbeef");

        assert_eq!(
            refresh_token_from_headers(&headers, &test_config()).as_deref(),
            Some("deadbeef")
        );
    }

    #[test]
    fn returns_none_when_cookie_absent() {
        let headers = headers_with_cookie("other=1");

        assert!(access_token_from_headers(&headers, &test_config()).is_none());
    }

    #[test]
    fn returns_none_without_cookie_header() {
        let headers = HeaderMap::new();

        assert!(access_token_from_headers(&headers, &test_config()).is_none());
    }

    /// 多个 Cookie 头并存时也要能找到令牌，避免中间件追加 Cookie 后认证失效。
    #[test]
    fn finds_token_across_multiple_cookie_headers() {
        let mut headers = HeaderMap::new();
        headers.append(
            axum::http::header::COOKIE,
            HeaderValue::from_static("first=1"),
        );
        headers.append(
            axum::http::header::COOKIE,
            HeaderValue::from_static("access_token=found-me"),
        );

        assert_eq!(
            access_token_from_headers(&headers, &test_config()).as_deref(),
            Some("found-me")
        );
    }

    /// 名称前缀相同的 Cookie 不能被误匹配。
    #[test]
    fn does_not_match_similar_cookie_name() {
        let headers = headers_with_cookie("access_token_extra=wrong");

        assert!(access_token_from_headers(&headers, &test_config()).is_none());
    }

    #[test]
    fn sets_expected_security_attributes() {
        let cookie = build_token_cookie(&test_config(), "access_token", "token-value", 900);

        assert!(cookie.starts_with("access_token=token-value"));
        assert!(cookie.contains("HttpOnly"), "必须设置 HttpOnly：{cookie}");
        assert!(
            cookie.contains("SameSite=Strict"),
            "必须设置 SameSite：{cookie}"
        );
        assert!(cookie.contains("Max-Age=900"), "必须设置有效期：{cookie}");
        assert!(cookie.contains("Path=/"), "必须设置 Path：{cookie}");
        // 未启用 secure 时不应出现该属性，否则本地 HTTP 调试会失败。
        assert!(
            !cookie.contains("Secure"),
            "未启用时不应设置 Secure：{cookie}"
        );
    }

    /// secure 配置必须真正传递到 Cookie 属性上，
    /// 否则生产环境会静默地以明文发送令牌。
    #[test]
    fn honors_secure_flag() {
        let mut config = test_config();
        config.secure = true;

        let cookie = build_token_cookie(&config, "access_token", "token-value", 900);

        assert!(cookie.contains("Secure"), "启用后必须设置 Secure：{cookie}");
    }

    #[test]
    fn honors_lax_same_site() {
        let mut config = test_config();
        config.same_site = "lax".to_owned();

        let cookie = build_token_cookie(&config, "access_token", "token-value", 900);

        assert!(cookie.contains("SameSite=Lax"), "应使用 Lax：{cookie}");
    }

    /// 清除 Cookie 的属性必须与下发时一致，否则浏览器匹配不到目标 Cookie，
    /// 令牌会残留在客户端。
    #[test]
    fn clear_cookie_matches_issued_attributes() {
        let config = test_config();
        let cookie = build_clear_cookie(&config, "access_token");

        assert!(cookie.starts_with("access_token="));
        assert!(cookie.contains("Max-Age=0"), "必须立即过期：{cookie}");
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Path=/"));
    }

    #[test]
    fn clear_cookie_honors_secure_flag() {
        let mut config = test_config();
        config.secure = true;

        let cookie = build_clear_cookie(&config, "refresh_token");

        assert!(
            cookie.contains("Secure"),
            "清除时必须与下发属性一致：{cookie}"
        );
    }

    /// 令牌值不应被引号包裹，避免客户端拼接请求头时出现不一致。
    #[test]
    fn does_not_quote_token_value() {
        let cookie = build_token_cookie(&test_config(), "access_token", "plain-token", 60);

        assert!(
            !cookie.contains("\"plain-token\""),
            "令牌值不应被引号包裹：{cookie}"
        );
    }
}
