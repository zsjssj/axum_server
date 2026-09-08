use axum::Router;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::app::routes::{HealthResponse, HelloResponse, ReadinessResponse, ServerInfoResponse};
use crate::app::state::AppState;
use crate::common::error::ErrorResponse;
use crate::common::pagination::PaginatedResponse;
use crate::modules::user::model::{CreateUserRequest, DeleteUserResponse, UpdateUserRequest, User};

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
        DeleteUserResponse,
        PaginatedResponse<User>,
        ErrorResponse,
    )),
    tags(
        (name = "系统", description = "服务状态与示例接口"),
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
}
