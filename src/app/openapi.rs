use axum::Router;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::app::routes::{HealthResponse, HelloResponse, ReadinessResponse, ServerInfoResponse};
use crate::app::state::AppState;
use crate::common::error::{ErrorData, ErrorDetail};
use crate::common::pagination::PaginatedResponse;
use crate::common::response::{ApiResponse, EmptyData};
use crate::modules::auth::model::{
    AuthSessionResponse, CookieNames, CurrentUserResponse, LoginRequest,
};
use crate::modules::user::model::{CreateUserRequest, UpdateUserRequest, User};

/// 服务 OpenAPI 文档定义
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Rust Axum Server API",
        version = "0.1.0",
        description = "Rust Axum Server 自动生成的接口文档"
    ),
    paths(
        crate::app::routes::root_handler,
        crate::app::routes::health_handler,
        crate::app::routes::readiness_handler,
        crate::app::routes::hello_handler,
        crate::modules::auth::handler::login,
        crate::modules::auth::handler::refresh_token,
        crate::modules::auth::handler::logout,
        crate::modules::auth::handler::current_user,
        crate::modules::user::handler::create_user,
        crate::modules::user::handler::list_users,
        crate::modules::user::handler::get_user,
        crate::modules::user::handler::update_user,
        crate::modules::user::handler::delete_user,
    ),
    components(schemas(
        ServerInfoResponse,
        HealthResponse,
        HelloResponse,
        ReadinessResponse,
        User,
        CreateUserRequest,
        UpdateUserRequest,
        LoginRequest,
        AuthSessionResponse,
        CookieNames,
        CurrentUserResponse,
        PaginatedResponse<User>,
        ApiResponse<User>,
        ApiResponse<PaginatedResponse<User>>,
        ApiResponse<ServerInfoResponse>,
        ApiResponse<HealthResponse>,
        ApiResponse<ReadinessResponse>,
        ApiResponse<HelloResponse>,
        ApiResponse<AuthSessionResponse>,
        ApiResponse<CurrentUserResponse>,
        ApiResponse<ErrorData>,
        ApiResponse<EmptyData>,
        ErrorDetail,
        ErrorData,
    )),
    tags(
        (name = "系统", description = "服务状态与示例接口"),
        (name = "认证", description = "登录、令牌刷新与登出"),
        (name = "用户", description = "用户管理接口")
    )
)]
pub struct ApiDoc;

/// 创建 OpenAPI JSON 与 Swagger UI 路由
pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_contains_registered_routes() {
        let document = ApiDoc::openapi();

        assert!(document.paths.paths.contains_key("/health"));
        assert!(document.paths.paths.contains_key("/api/v1/users"));
        assert!(
            document
                .paths
                .paths
                .contains_key("/api/v1/users/{public_id}")
        );
    }

    #[test]
    fn openapi_contains_auth_routes() {
        let document = ApiDoc::openapi();

        assert!(document.paths.paths.contains_key("/api/v1/auth/login"));
        assert!(document.paths.paths.contains_key("/api/v1/auth/refresh"));
        assert!(document.paths.paths.contains_key("/api/v1/auth/logout"));
        assert!(document.paths.paths.contains_key("/api/v1/auth/me"));
    }

    /// 认证改为 Cookie 方案后不应再注册 Bearer 安全方案：
    /// 残留的 Bearer 方案会误导调用方以为存在 `Authorization` 头这条认证路径。
    #[test]
    fn openapi_does_not_register_bearer_scheme() {
        let document = ApiDoc::openapi();
        let security_schemes = document
            .components
            .map(|components| components.security_schemes)
            .unwrap_or_default();

        assert!(
            !security_schemes.contains_key("bearer_auth"),
            "Cookie 方案下不应存在 bearer_auth 安全方案"
        );
    }
}
