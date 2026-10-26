# 12 API 文档与测试

**本章目标**：让"接口契约"和"代码行为"都有可执行的守护。文档用代码注解生成（不会过期），测试用行为断言锁定（不会被悄悄改坏）。

---

## 12.1 文档从代码生成，而不是手写

手写 Markdown 描述接口的最大问题是**会过期**。改了字段忘了改文档，比没文档更糟——调用方会照着错的文档写代码。

本项目的做法：**注解即文档**。

```rust
// src/modules/user/handler.rs:38
#[utoipa::path(
    post,
    path = "/api/v1/users",
    tag = "用户",
    request_body = CreateUserRequest,
    responses(
        (status = 201, description = "用户创建成功", body = ApiResponse<User>),
        (status = 400, description = "JSON 请求体格式错误", body = ApiResponse<ErrorData>),
        (status = 401, description = "未登录或登录已过期", body = ApiResponse<ErrorData>),
        (status = 415, description = "请求体媒体类型错误", body = ApiResponse<ErrorData>),
        (status = 422, description = "请求参数校验失败", body = ApiResponse<ErrorData>),
        (status = 409, description = "用户名已存在", body = ApiResponse<ErrorData>),
        (status = 500, description = "服务器内部错误", body = ApiResponse<ErrorData>)
    )
)]
```

注解各部分的作用：

| 部分 | 来源 | 作用 |
| --- | --- | --- |
| `path`、`tag` | 手写字符串 | 路径与分组。**路径要和路由注册一致**，两者不会自动关联 |
| `request_body` | 类型名 | 引用 `CreateUserRequest` 的 Schema |
| `params(...)` | 类型名或字面量 | 查询参数用实现了 `IntoParams` 的类型（如 `Pagination`），路径参数手写 |
| `responses(...)` | 状态码 + `body =` 类型名 | 每个状态码对应的响应体 |

**注意 `path` 和实际路由是两份独立的字符串**。路由改了注解没改，OpenAPI 里就是错的路径。这是一个可以自动化校验的点（见 12.5 的练习）。

### 请求/响应模型：`ToSchema`

```rust
// src/modules/user/model.rs:41
#[derive(Debug, Deserialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct CreateUserRequest {
    #[validate(length(min = 3, max = 50, message = "用户名长度必须在 3 到 50 个字符之间"))]
    #[schema(min_length = 3, max_length = 50)]      // ← 让文档里也显示约束
    pub username: String,

    #[validate(length(max = 100, message = "邮箱长度不能超过 100 个字符"))]
    #[validate(email(message = "邮箱格式不正确"))]
    #[schema(format = Email, max_length = 100)]
    pub email: String,

    #[validate(length(min = 8, max = 72, message = "密码长度必须在 8 到 72 个字符之间"))]
    #[schema(min_length = 8, max_length = 72, write_only)]   // ← 密码不进响应示例
    pub password: String,
    // ...
}
```

`#[schema(...)]` 和 `#[validate(...)]` **必须同步维护**：前者描述文档，后者实际校验。两者不一致时，文档会骗人（比如文档写 `max_length = 50` 但校验是 `max = 20`）。

这是本项目里少数无法靠编译器消除的重复。缓解办法是**紧挨着写**（如上），让改一处时另一处就在眼前。

### 查询参数：`IntoParams`

```rust
// src/common/pagination.rs:8
#[derive(Debug, Deserialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct Pagination {
    #[serde(default = "default_page")]
    #[validate(range(min = 1, message = "页码必须大于等于 1"))]
    #[param(minimum = 1, default = 1)]
    page: i64,
    // ...
}
```

于是 Handler 只需要 `params(Pagination)`，文档就有了 `page`、`page_size` 的定义和默认值。

## 12.2 注册：`openapi.rs` 是唯一的汇总点

```rust
// src/app/openapi.rs:16
#[derive(OpenApi)]
#[openapi(
    info(title = "Rust Axum Server API", version = "0.1.0", description = "..."),
    paths(
        crate::app::routes::root_handler,
        // ... 共 13 个 Handler
        crate::modules::user::handler::delete_user,
    ),
    components(schemas(
        User, CreateUserRequest, UpdateUserRequest,
        // ... 所有请求/响应/通用模型
        ApiResponse<ErrorData>, ErrorData,
    )),
    tags(
        (name = "系统", description = "服务状态与示例接口"),
        (name = "认证", description = "登录、令牌刷新与登出"),
        (name = "用户", description = "用户管理接口")
    )
)]
pub struct ApiDoc;
```

```rust
// src/app/openapi.rs:73
pub fn routes() -> Router<AppState> {
    Router::new().merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
}
```

**新增接口时最容易漏的一步就是在这里注册 `paths`。** 注解写好了、路由也通了，但 OpenAPI 里没有它——因为 utoipa 不会自动发现注解。

### 为什么不注册 `bearer_auth`

接口用 Cookie 认证，但 OpenAPI 里**故意不声明任何安全方案**：

| 为什么不声明 | 说明 |
| --- | --- |
| 不声明 `bearer_auth` | 残留的 Bearer 方案会让调用方以为存在 `Authorization` 头这条路（实际不支持） |
| 不声明 `apiKey` 形式的 Cookie | OpenAPI 的 `apiKey` 只能表达"有个凭证"，无法表达 `HttpOnly`、`SameSite` 这些**实际生效的约束** |

不如在接口描述里说清楚，也不给出误导性的机器可读声明。测试锁定：

```rust
// src/app/openapi.rs:109
fn openapi_does_not_register_bearer_scheme() {
    assert!(!security_schemes.contains_key("bearer_auth"));
}
```

### 受保护接口必须声明 401

这是一个"注解容易漏、后果很实际"的点：

```rust
// src/app/openapi.rs:127
fn openapi_declares_unauthorized_for_protected_routes() {
    for path in ["/api/v1/users", "/api/v1/users/{public_id}", "/api/v1/auth/me", "/api/v1/hello"] {
        // 断言每个方法都声明了 401
    }
}
```

为什么重要：认证是**中间件统一强制**的，从 Handler 签名上看不出来（`CurrentUser` 只是可读性提示）。所以注解是调用方判断"这个接口要不要登录"的唯一直观依据。漏写会让文档把它显示成匿名接口。

## 12.3 测试策略：把测试放在最便宜的层次

本项目的测试只有两层，没有独立的 `tests/` 目录：

```text
src/**/mod.rs 里的 #[cfg(test)] mod tests   ← 单元测试（绝大多数）
src/app/mod.rs 里的集成式测试                ← 走完整 Router，但不连真实数据库/Redis
```

当前共 **129 个测试**，分布大致是：

| 文件 | 数量 | 测什么 |
| --- | ---: | --- |
| `modules/information/model.rs` | 18 | 会话/消息校验、撤回必须清空正文、游标参数边界 |
| `modules/auth/service.rs` | 16 | JWT 签发/校验/过期/类型、重放、哈希 |
| `config/mod.rs` | 14 | 各类非法配置必须被拒 |
| `app/mod.rs` | 17 | 路由、认证中间件、信封、404/405 |
| `common/cookie.rs` | 12 | Cookie 解析与安全属性 |
| `common/auth.rs` | 8 | 白名单、路径匹配、身份传递 |
| `infrastructure/redis.rs` | 8 | 键命名空间、前缀规范化、错误分类 |
| `modules/auth/model.rs` / `handler.rs` | 5 + 5 | 登录模型、Cookie 下发/清除 |
| `modules/information/service.rs` | 3 | 单聊唯一键的方向无关性与无碰撞 |
| 其余 | 23 | 分页、密码、提取器、引导账号、OpenAPI 注册、聊天路由保护等 |

这个分布很说明问题：**测试集中在"安全约束"和"输入边界"上**，而不是追求覆盖率数字。这是正确的优先级——认证、配置校验、Cookie 属性的 bug，代价远高于一个 CRUD 方法的分支未覆盖。

### 单元测试：不需要外部依赖

```rust
// src/modules/user/model.rs:108
#[test]
fn rejects_invalid_create_request() {
    let request = CreateUserRequest {
        username: "ab".to_owned(),
        email: "invalid-email".to_owned(),
        password: "short".to_owned(),
        nickname: None,
    };
    let errors = request.validate().expect_err("非法创建参数应校验失败");

    assert!(errors.field_errors().contains_key("username"));
    assert!(errors.field_errors().contains_key("email"));
    assert!(errors.field_errors().contains_key("password"));
}
```

这类测试**零依赖、毫秒级**，能在 `cargo test` 里跑上千次也不慢。所以能写成这样就不要写成集成测试。

### 集成式测试：走完整 Router，但不连库

```rust
// src/app/mod.rs:60
fn test_app() -> Router {
    let pool = PgPoolOptions::new()
        .connect_lazy("postgres://user:password@localhost/test")   // ← 不建连
        .expect("测试数据库连接字符串应合法");

    create_app(pool, crate::infrastructure::redis::RedisStore::without_connection(), &test_config())
}
```

两个技巧是整个测试方案的关键：

| 技巧 | 作用 |
| --- | --- |
| `connect_lazy()` | 创建连接池但不建立连接。只要测试不真的查库，就完全不需要数据库 |
| `RedisStore::without_connection()` | 同理。而且 Redis 不可达时，吊销检查会走"按未吊销放行"的降级分支——**顺带覆盖了降级路径** |

驱动方式用 `tower::ServiceExt::oneshot`：

```rust
// src/app/mod.rs:133
async fn dispatch(method: Method, uri: &str, body: Body, content_type: bool, access_token: Option<&str>) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if content_type { builder = builder.header("content-type", "application/json"); }
    if let Some(token) = access_token {
        builder = builder.header("cookie", format!("{}={}", ACCESS_COOKIE_NAME, token));
    }

    test_app()
        .oneshot(builder.body(body).expect("测试请求应构造成功"))
        .await
        .expect("测试请求应成功进入 Router")
}
```

`oneshot` 的意思是"发一个请求然后消费掉这个 Router"。它不需要监听端口，所以测试之间不会有端口冲突，也不用担心并发。

### 已登录请求怎么造

认证中间件挂在 `/api/v1` 整组上，所以**任何业务接口的测试都必须带合法令牌**，否则测到的是 401 而不是目标行为。项目提供了一个专门的方法：

```rust
// src/modules/auth/service.rs:319
#[doc(hidden)]
#[allow(dead_code)]
pub fn issue_test_access_token(jwt_config: &JwtConfig, user_id: i64, expires_at: i64) -> String
```

它为什么值得单独讲？注释里解释了三层考虑（`:305-318`）：

1. **复用生产的 `AccessClaims` 结构**。如果测试自己拼 JSON 造 JWT，生产改了字段测试还会通过——覆盖价值归零。
2. **`#[doc(hidden)]` 而不是 `#[cfg(test)]`**。`cfg(test)` 只在编译本 crate 的测试目标时生效，而 `tests/` 下的集成测试是**独立 crate**，看不到这个函数。
3. **`#[allow(dead_code)]`**，因为生产构建里没人调用它。

配套用一个远期过期时间，避免用例随系统时间推移突然失效：

```rust
// src/app/mod.rs:58
const LONG_LIVED_TOKEN_EXPIRY: i64 = 4_102_444_800;   // 2100-01-01
```

> 这些细节说明作者踩过坑：**"测试突然因为时间流逝而失败"是非常经典的一类 flaky test。**

### 断言策略：断言业务码，而不只是状态码

```rust
// src/app/mod.rs:224
#[tokio::test]
async fn rejects_anonymous_requests_under_api_prefix() {
    for (method, uri) in [
        (Method::GET, "/api/v1/hello"),
        (Method::GET, "/api/v1/users"),
        (Method::POST, "/api/v1/users"),
        (Method::GET, "/api/v1/auth/me"),
    ] {
        let response = request(method.clone(), uri, Body::empty(), false).await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{method} {uri} 未登录时必须返回 401");
        let body = json_body(response).await;
        assert_eq!(body["code"], 40100, "{method} {uri} 应返回未认证业务码");
        assert_eq!(body["message"], "未登录或登录已过期");
    }
}
```

三点值得学：

- **状态码 + 业务码 + 文案三重断言**。只断言状态码的话，"401 但业务码写成 40000"这种错就漏了。
- **表驱动**。一个用例覆盖四个路径，加新接口时往数组里加一行即可。
- **`{method} {uri}` 写进断言消息**。失败时能立刻知道是哪一行挂了，不用去数循环。

### 一类特别的测试："断言响应不是某个错误"

```rust
// src/app/mod.rs:266
async fn lets_authenticated_request_reach_business_layer() {
    let response = request_as_logged_in_user(Method::GET, "/api/v1/users", Body::empty(), false).await;
    let body = json_body(response).await;

    // 没有真实数据库，所以断言的不是"业务成功"，而是"不再是未认证错误"
    assert_ne!(body["code"], 40100, "合法令牌不应被认证中间件拒绝");
}
```

这是一种"环境受限下的最优断言"：没有数据库就无法断言 200，但能断言"请求已经穿过中间件"。**测试的目的是证明行为，不是凑出 200。**

同类技巧还有：

```rust
// src/app/mod.rs:314
async fn keeps_refresh_reachable_without_token() {
    // 带合法 access token 但没有 refresh_token：
    // 看到"缺少刷新令牌"而不是"未登录或登录已过期"，就证明请求穿过了中间件
    assert_eq!(body["message"], "缺少刷新令牌");
}
```

## 12.4 什么行为必须有测试

判断标准：**这个行为被改坏时，会不会是静默的？**

| 必须有 | 例子 | 为什么 |
| --- | --- | --- |
| 安全约束 | `rejects_token_with_another_secret`、`dummy_password_hash_is_parseable` | 改坏后功能照常工作，只在攻击下暴露 |
| 白名单与路由 | `keeps_logout_reachable_without_token` | 改坏后登出变成 401，客户端卡死 |
| 序列化往返 | `user_round_trips_through_cache_json` | 改坏后只在缓存命中时 500 |
| 配置校验 | `rejects_short_jwt_secret`、`falls_back_to_strict_same_site` | 改坏后服务能启动，但防护变弱 |
| 契约细节 | `cookies_use_matching_max_age` | 改坏后浏览器行为异常，服务端无感 |

| 可以不写 | 为什么 |
| --- | --- |
| `getter` / 简单转发 | 编译器已经保证了 |
| 纯 CRUD 的每个分支 | 代价高于收益；边界情况（不存在、冲突）值得测 |
| 框架行为（Axum 路由匹配） | 那是框架的测试职责 |

## 12.5 动手练习

1. **跑一遍全量测试**：`cargo test`，观察 100 个测试全部通过且**不需要数据库和 Redis**。然后 `cargo test -- --nocapture` 看日志输出。
2. **加一个接口 + 一个测试**：注册 `GET /api/v1/ping`（返回 `"pong"`），在 `openapi.rs` 的 `paths` 里注册，然后补两个测试：匿名必须 401、登录后返回 `code = 0`。
3. **修一个 flaky 隐患**：把 `LONG_LIVED_TOKEN_EXPIRY` 改成一个很近的时间戳（比如当前时间 +5 秒），跑测试，等 10 秒再跑一次，观察它开始失败。理解为什么要用远期常量。
4. **加自动化校验**：写一个测试，遍历 `ApiDoc::openapi().paths`，断言每个 `path` 都以 `/` 开头；再进一层：把 `#[utoipa::path]` 里的 `path` 与实际路由对比（提示：可以从 `Router` 里拿不到，但可以维护一份常量表对比，或用集成测试请求 OpenAPI 后逐个 `curl`）。
5. **体验"降级被覆盖"**：在 `lets_authenticated_request_reach_business_layer` 里加一行 `assert!(response.headers().len() > 0)` 之类的断言，思考为什么"用不连通的 Redis 跑测试"反而是一种优势。

## 12.6 自测题

1. 为什么不维护手写 Markdown 接口文档？"注解即文档"解决了什么具体问题？
2. `#[schema(...)]` 和 `#[validate(...)]` 为什么会不一致？怎么降低风险？
3. 新增接口时最容易漏的两步是什么？
4. 为什么不注册 `bearer_auth` 安全方案？
5. `connect_lazy` 和 `without_connection` 分别让什么变得可能？
6. `issue_test_access_token` 为什么必须是 `pub` + `#[doc(hidden)]`，而不能是 `#[cfg(test)]`？
7. 为什么测试要同时断言状态码和业务码？
8. "断言响应不是 401"这种测试，价值在哪？什么时候它是最优选择？

---

下一步 → [13 动手扩展新模块](./13-动手扩展新模块.md)
