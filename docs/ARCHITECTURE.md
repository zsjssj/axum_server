# 项目架构与设计说明

## 1. 项目定位

本项目是基于 Rust、Axum 与 PostgreSQL 构建的 HTTP API 服务。当前以用户模块为示例，采用按业务模块组织代码、模块内部按职责分层的结构，目标是让路由、业务逻辑和数据访问保持清晰边界，并为后续增加其他业务模块预留统一的扩展方式。

主要技术组件：

- Axum：HTTP 路由、请求提取和响应转换。
- Tokio：异步运行时及阻塞任务调度。
- SQLx：PostgreSQL 连接池与数据访问。
- Serde：请求、响应和配置的序列化与反序列化。
- Validator：请求模型和查询参数的声明式校验。
- config + dotenv：分环境配置加载。
- tracing：结构化日志。
- Argon2：密码哈希。
- jsonwebtoken：签发与校验 access token（HS256）。
- Utoipa + Swagger UI：从代码注解生成 OpenAPI 文档并提供交互式浏览入口。

## 2. 目录与职责

```text
rust-axum-server/
├── config/                 # 默认配置和分环境配置
├── docs/                   # 架构与接口文档
├── migrations/             # 数据库迁移脚本
└── src/
    ├── app/                # 应用装配、全局状态、路由和中间件
    ├── common/             # 跨业务模块复用的能力：错误边界、分页、Cookie、登录校验等
    ├── config/             # 配置结构定义与加载逻辑
    ├── infrastructure/     # 数据库等外部基础设施适配
    ├── modules/            # 业务模块
    │   ├── auth/           # 认证模块：登录、令牌刷新、登出、当前用户
    │   └── user/           # 用户模块
    └── main.rs             # 程序入口与启动流程
```

各层职责如下：

| 层级 | 主要职责 | 不应承担的职责 |
| --- | --- | --- |
| `main` | 串联配置、日志、数据库、应用和监听器的初始化 | 具体业务处理 |
| `app` | 组装路由、中间件和共享状态 | 业务规则和 SQL |
| `handler` | 提取 HTTP 参数、调用服务、组装 HTTP 响应 | 直接编写 SQL 或承载复杂业务规则 |
| `service` | 执行业务规则、协调数据访问、转换业务错误 | 处理路由细节 |
| `repository` | 执行 SQL、完成数据库实体读写 | 决定 HTTP 状态码或业务流程 |
| `model` | 定义数据库实体、请求模型及序列化规则 | 执行业务操作 |
| `common` | 提供可跨模块复用的通用能力 | 放置仅属于单个模块的逻辑 |
| `infrastructure` | 初始化和封装数据库等外部依赖 | 承载领域业务规则 |

依赖方向应保持为：

```text
HTTP 请求
   ↓
Router / Middleware
   ↓
Handler
   ↓
Service
   ↓
Repository
   ↓
PostgreSQL
```

上层可以调用下层，下层不应反向依赖上层。例如，Repository 不应依赖 Handler，Service 不应返回 Axum 的 HTTP 响应类型。

## 3. 应用启动流程

程序从 `src/main.rs` 启动，完整流程如下：

1. `main` 进入 Tokio 异步运行时并调用 `run`。
2. `AppConfig::new` 加载环境变量和配置文件。
3. `AppConfig::validate` 校验配置。弱 JWT 密钥、非正数有效期、重复的 Cookie 名称等问题在此被拒绝，避免服务带着可被伪造的令牌签发能力对外提供服务。
4. 根据日志级别初始化 `tracing` 订阅器。
5. 根据数据库配置创建 PostgreSQL 连接池。
6. SQLx 执行编译进程序的 `migrations` 迁移；迁移失败时终止启动，避免服务在数据库结构不完整时接收流量。
7. `create_app` 创建 `AppState`（连接池与认证配置），组装健康检查路由、API 路由和中间件。
8. 根据主机与端口配置绑定 TCP 监听器。
9. Axum 通过 `into_make_service_with_connect_info` 接管监听器并开始处理请求。使用带连接信息的服务是因为登录与刷新接口需要记录客户端 IP 用于安全审计。
10. 任一启动步骤失败时，错误返回到 `main`，程序输出错误并以非零状态退出。

```text
main
  └─ run
      ├─ 加载配置
      ├─ 校验配置
      ├─ 初始化日志
      ├─ 创建数据库连接池
      ├─ 执行数据库迁移
      ├─ 组装 Axum Router 与 AppState
      ├─ 绑定 TCP 地址
      └─ 启动 HTTP 服务（携带连接信息）
```

## 4. 配置加载流程

配置按从低到高的优先级进行覆盖：

1. `config/default.toml`：所有环境共享的默认值。
2. `config/{RUN_MODE}.toml`：运行环境配置，`RUN_MODE` 默认为 `development`。
3. `config/local.toml`：本地覆盖配置，可选且不应提交敏感信息。
4. `APP__*` 环境变量：部署时的最终覆盖项。

环境变量使用双下划线表示层级，例如：

```text
APP__SERVER__PORT=8080
APP__DATABASE__URL=postgresql://user:password@host/database
```

设计思路是将配置结构集中定义在 `src/config`，业务代码只依赖已经完成反序列化的强类型配置，避免在各模块中分散读取环境变量。

### 数据库需要 UTF-8 兼容的错误消息

PostgreSQL 必须配置为返回 UTF-8 兼容的错误文本，否则 SQLx 无法解析任何服务端错误：

```conf
# postgresql.conf
lc_messages = 'C'
```

原因不是编码偏好问题。当 `lc_messages` 设为中文（如 `zh-CN`）时，Windows 版 PostgreSQL 会用系统 ANSI 代码页（GBK）编码错误消息，而 SQLx 要求 UTF-8。结果是**任何**数据库错误——包括连接认证失败和迁移失败——都会被替换为一条含义模糊的协议错误：

```text
Postgres protocol error (reading ErrorResponse): Postgres returned a
non-UTF-8 string for its error message.
```

该提示把真正的原因（例如「密码认证失败」）完全掩盖，显著增加排查成本。设为 `C` 后消息为纯 ASCII，任何客户端都能正确解码。

注意客户端侧的 `PGCLIENTENCODING` **不能**覆盖该行为，因为它只影响客户端编码协商，不改变服务端错误消息的语言与编码。

## 5. HTTP 请求处理流程

以 `POST /api/v1/users` 为例：

1. 请求先经过 CORS 与日志中间件。
2. Router 将请求匹配到用户模块的 `create_user` Handler。
3. 自定义提取器将 JSON 请求体反序列化为 `CreateUserRequest`，执行模型校验，并把解析或校验错误转换成统一 JSON 响应；校验通过后 Handler 再从 `AppState` 提取数据库连接池。
4. `UserService` 执行业务流程，通过 `spawn_blocking` 在线程池中计算 Argon2 密码哈希，避免 CPU 密集型计算阻塞异步执行器。
5. `UserRepository` 使用参数绑定执行插入 SQL，防止 SQL 注入，并返回不包含密码哈希的用户实体。
6. Service 将唯一键冲突转换为统一的 `AppError::Conflict`。
7. Handler 将成功数据包装为 `ApiResponse<T>` 并转换为 `201 Created`；失败结果由 `AppError::into_response` 转换为相同响应信封。
8. 日志中间件记录请求方法、路径、状态码和耗时。

查询、更新和删除请求遵循同样的调用方向。JSON、Query 和 Path 参数均通过 `common::extractor` 中的提取器进入统一错误边界。分页参数要求页码大于等于 1、每页数量处于 1 到 100 之间，超出范围时返回校验错误，不再静默纠正客户端输入。

### 受保护接口的认证流程

`/api/v1` 下的**全部接口默认需要登录**，认证由 `common::auth::require_login` 中间件在路由组上统一强制。以 `GET /api/v1/auth/me` 为例：

1. 请求先经过 CORS 与日志中间件。
2. 请求进入 `/api/v1` 路由组后先经过认证中间件。中间件从 `access_token` Cookie 读取 JWT，校验签名、签发者、有效期与 `typ` 声明，并把任何失败转换为统一的 `AppError::Unauthorized`（`401`，业务码 `40100`）。
3. 校验通过后，中间件把用户身份写入请求扩展；Handler 再提取 `State`、`CurrentUser` 与业务参数，后续流程与匿名接口一致。
4. `CurrentUser` 提取器只从请求扩展读取身份，不再次解析 Cookie。Handler 签名中出现 `CurrentUser` 表示该接口需要登录，身份来源由中间件保证。

选择「中间件默认拒绝 + 显式白名单」而不是逐个 Handler 声明认证，原因是失效方向：漏写一处认证只会被评审发现，而默认拒绝会让任何新增接口天然受保护。白名单只有三项，且与路由注册共用同一组常量（`common::auth` 的 `LOGIN_PATH`、`REFRESH_PATH`、`LOGOUT_PATH`）：

| 匿名可访问 | 不可移除的理由 |
| --- | --- |
| `POST /api/v1/auth/login` | 登录本身必须匿名，否则无法获得凭证 |
| `POST /api/v1/auth/refresh` | access token 过期后客户端只剩刷新 Cookie，否则刷新流程永远无法启动 |
| `POST /api/v1/auth/logout` | 登出必须幂等，令牌已失效时仍要能清理会话 |

两点实现约束：

- 中间件挂在 `nest` 之后，而 `nest` 会剥离 `/api/v1` 前缀，因此中间件看到的路径是 `/auth/login` 这样的相对路径。`is_public_path` 同时接受完整路径与相对路径，避免有人按完整路径写白名单后造成静默错误。
- 使用的是 `route_layer` 而不是 `layer`：中间件只作用于已匹配到路由的请求，未注册的路径直接落到 `404` fallback，不会因为认证失败先返回 `401`。

令牌**只从 Cookie 读取**，不接受 `Authorization` 头，避免出现绕过 Cookie 安全属性的旁路认证方式。

## 6. 认证与令牌设计

认证模块位于 `src/modules/auth`，遵循与其他业务模块相同的分层结构。

### 双令牌与 Cookie 下发

登录成功后签发两个令牌，均通过 `Set-Cookie` 响应头下发，**响应体不包含令牌**：

| 令牌 | 格式 | 有效期 | 服务端状态 |
| --- | --- | --- | --- |
| access token | JWT（HS256） | 15 分钟 | 无状态，不落库 |
| refresh token | 256 位随机值 | 7 天 | 只存 SHA-256 摘要 |

Cookie 固定启用 `HttpOnly`，`Secure` 与 `SameSite` 由 `auth.cookie` 配置控制。`HttpOnly` 使页面脚本无法读取令牌，把 XSS 的影响限制为"代为发请求"，而无法长期窃取凭据。两个 Cookie 的 `Max-Age` 分别匹配各自令牌的有效期。

`AuthService::TokenPair` 刻意不派生 `Serialize`：令牌在类型上不可序列化，从代码层面杜绝被误放进 JSON 响应。

### 令牌轮换与重放检测

`/auth/refresh` 采用轮换策略：每次刷新签发新的 refresh token，并把旧会话标记为已吊销、在 `replaced_by` 中记录轮换关系。

已吊销的 refresh token 再次被使用会被判定为泄露重放，服务端随即吊销该用户的全部会话。此时无法分辨攻击者与合法用户，只能让双方都重新登录——这是"宁可多一次登录，也不允许泄露令牌继续可用"的取舍。

### 登出语义的边界

access token 无状态，服务端无法逐个吊销。因此**登出只能立即失效 refresh token**，已签发的 access token 最多再存活 15 分钟。这是双令牌方案的固有性质，不是实现缺陷，对安全性要求更高的场景应缩短 `access_ttl_minutes`。

### 登录保护

登录失败按用户名与滑动时间窗口统计，超过阈值后返回 `429`。两个关键顺序约束：

- 失败记录独立存放于 `login_failures` 表，不在 `users` 表上做账号锁定，避免自动锁定被用作针对特定账号的拒绝服务手段。
- **先判定限流，再校验密码**。Argon2 校验被刻意设计得很慢，顺序颠倒会让大量失败请求持续占用阻塞线程池，使限流本身成为消耗算力的攻击面。

此外，用户名不存在时同样执行一次 Argon2 计算，使用户名枚举无法通过响应耗时区分账号是否存在。

### CSRF 防护

Cookie 会自动随请求发送，因此必须防 CSRF。主要防线是 `SameSite=Strict`（默认值）；配置为未知取值时会回退到 `Strict`，不存在"写错配置导致防护静默放宽"的路径。辅助措施是写操作全部使用 `POST` 且强制 `application/json` 请求体，使跨源请求必须先通过预检。**生产环境仍应配合 CORS 白名单收紧来源。**

## 7. 全局状态与中间件

`AppState` 保存可以安全共享的应用级依赖：`PgPool`、`Arc<JwtConfig>` 与 `Arc<AuthConfig>`。Axum 会克隆 State，但 `PgPool` 内部使用共享句柄、配置项使用 `Arc`，因此不会为每次请求复制连接池或配置内容。认证配置随状态共享，使认证中间件可以在校验令牌时直接访问 JWT 密钥，而不必让每个受保护的 Handler 多一个依赖参数。

当前中间件包括：

- 日志中间件：记录方法、URI、响应状态和处理耗时。
- CORS 中间件：当前允许任意来源、方法和请求头，便于开发联调；生产环境应根据实际前端域名收紧规则。若前端与后端跨源部署，**必须启用凭据支持并指定具体来源**，因为浏览器不会在通配来源下携带 Cookie。
- 认证中间件（`common::auth::require_login`）：只作用于 `/api/v1` 路由组中已匹配到路由的请求，按白名单放行认证入口，其余请求一律要求有效的 access token Cookie。

中间件的挂载顺序会影响失败响应：认证中间件在内层，因此 `/api/v1` 下未登录的请求先得到 `401`，不会落到外层的 404、405 fallback。

新增全局依赖时，应优先加入 `AppState`，由 Handler 通过 `State` 提取，避免使用全局可变变量。

## 8. 错误处理设计

`AppError` 是应用统一错误边界，负责将内部错误转换为 HTTP 状态码和 JSON 响应：

- `BadRequest` → `400 Bad Request`
- `Validation` / `UnprocessableEntity` → `422 Unprocessable Entity`
- `UnsupportedMediaType` → `415 Unsupported Media Type`
- `Unauthorized` → `401 Unauthorized`
- `NotFound` → `404 Not Found`
- `MethodNotAllowed` → `405 Method Not Allowed`
- `Conflict` → `409 Conflict`
- `TooManyRequests` → `429 Too Many Requests`
- `ServiceUnavailable` → `503 Service Unavailable`
- `Database` → `500 Internal Server Error`
- `Internal` → `500 Internal Server Error`

所有成功和失败响应均使用 `ApiResponse<T> { code, message, data }` 信封。数字 `code` 与 HTTP 状态码职责分离：HTTP 状态码供网关、监控和通用客户端判断协议结果，业务码供前端稳定处理，其中成功固定为 `0`。由于 `ApiCode` 使用 `#[repr(i32)]`，`Success` 必须显式声明为 `0` 而不是复用 HTTP 的 `200`，否则会出现"HTTP 成功但业务码表示失败"的矛盾。成功时 `data` 保存业务模型；失败时通常为 `null`，参数校验失败时保存 `{ details: [{ field, message }] }`，以保留具体字段原因。自定义 JSON、Query、Path 提取器以及 Router 的 404、405 fallback 可防止 Axum 默认纯文本拒绝响应绕过该结构。

业务码按错误类别分段：请求格式 `40000`、参数校验 `40001`、媒体类型 `40002`、未认证 `40100`、资源不存在 `40400`、方法不允许 `40500`、业务冲突 `40900`、触发限流 `42900`、内部错误 `50000`、服务不可用 `50300`。新增业务模块时应在对应号段中定义更具体的业务码，避免直接复用无语义的字符串。

数据库错误的详细内容只写入服务端日志，对客户端统一返回“服务器内部错误”，避免泄露数据库结构、SQL 或连接信息。业务模块应尽量把可预期的底层错误转换为明确的业务错误，其他数据库错误通过 `From<sqlx::Error>` 进入统一处理。

## 9. 数据与安全设计

- 密码仅以 Argon2 哈希形式写入数据库，API 响应和查询字段均不包含 `password_hash`。认证查询通过独立的 `UserCredentials` 类型返回凭证，该类型不派生 `Serialize` 与 `ToSchema`；把 `password_hash` 挂到公开的 `User` 上会使其顺着响应序列化和 OpenAPI Schema 一起对外暴露，类型分离是这里唯一可靠的防线。
- 令牌通过 `HttpOnly` Cookie 下发，响应体不携带令牌；access token 与 refresh token 使用不同有效期，Cookie 的 `Max-Age` 分别匹配。
- refresh token 只保存 SHA-256 摘要，数据库泄露时无法反推出可用令牌；因其本身是 256 位随机值，不存在字典攻击空间，无需使用慢速哈希。
- 用户采用内部 `BIGINT IDENTITY` 主键与公开 UUIDv7 双 ID。内部主键用于数据库关联和排序，不通过 API 暴露；UUIDv7 由 Service 在创建用户时生成，作为接口路径、响应和跨系统传递使用的稳定标识。认证令牌的 `sub` 使用内部主键，使受保护接口可直接命中主键索引。
- 所有 SQL 使用 SQLx 参数绑定，不拼接用户输入。IP 地址以文本绑定并由 `$n::inet` 交给 PostgreSQL 校验格式。
- 数据库连接通过连接池复用，并设置连接获取超时。
- 健康检查 `/health` 只表示进程可响应；就绪检查 `/ready` 会执行数据库探测，用于判断服务是否具备接收业务流量的条件。
- 数据库结构通过 `migrations` 目录中的脚本维护；迁移脚本会在编译时嵌入程序，并在服务启动、开始监听端口前自动执行。
- 已发布或可能已执行的结构变更应新增迁移文件，不直接修改历史迁移；任何迁移失败都会中止服务启动。认证相关表通过 `002_create_auth_tables.sql` 新增，未改动 `001`。

## 10. 模块扩展方式

新增业务模块时，建议沿用用户模块与认证模块的组织方式：

```text
src/modules/{module}/
├── mod.rs
├── handler.rs
├── model.rs
├── repository.rs
└── service.rs
```

推荐步骤：

1. 在 `model.rs` 定义实体、请求和响应数据结构。只有需要序列化给调用方的模型才派生 `Serialize` 与 `ToSchema`；密码哈希这类内部字段应放在单独的类型中，避免顺着响应序列化泄露出去。
2. 在 `repository.rs` 实现最小粒度的数据访问操作。
3. 在 `service.rs` 组合业务规则并处理业务错误。
4. 在 `handler.rs` 定义路由和 HTTP 适配逻辑。
5. 在模块的 `mod.rs` 暴露路由入口。
6. 在 `src/modules/mod.rs` 注册模块。
7. 在 `src/app/routes.rs` 合并模块路由到 `/api/v1` 路由组。挂在该组下的接口**自动需要登录**，无需额外声明；只有当接口确实应当匿名可访问时，才需要把它加入 `common::auth` 的白名单并说明理由。
8. 需要当前用户身份时，在 Handler 签名中声明 `common::auth_extractor::CurrentUser`。
9. 在 `src/app/openapi.rs` 注册新增的接口与 Schema，并为受保护接口补上 `401` 响应。
10. 补充迁移、测试和对应 API 文档。

跨模块复用的能力应下沉到 `common`，而不是让一个业务模块引用另一个业务模块的内部实现。例如密码哈希原本定义在用户模块中，认证模块也需要相同的 Argon2 参数，于是抽取为 `common::password`；两处独立实现很容易在参数上逐渐分叉，导致同一密码在不同入口得到强度不一致的哈希。

如果业务逐渐复杂，可以在模块内部继续拆分领域对象或子模块，但仍需保持 HTTP、业务和持久化职责之间的边界。

## 11. 设计原则

- 按业务模块聚合：同一业务的 Handler、Service、Repository 和 Model 放在同一模块中，减少跨目录跳转。
- 分层但不过度抽象：当前直接使用具体类型和静态方法；只有在出现真实的替换、复用或测试隔离需求时，再引入 Trait 等抽象。
- 统一横切能力：错误、分页、配置、日志和共享状态集中管理，避免每个模块重复实现。
- 异步路径避免阻塞：数据库和网络操作使用异步接口；CPU 密集或阻塞操作使用专用阻塞线程池。
- 默认保护敏感信息：内部错误记录到日志，对外只返回稳定且必要的信息。
- 文档随实现更新：路由、模块边界、配置优先级或关键设计发生变化时，应同步更新本文件和对应 API 文档。

## 12. 注释语言约束

本项目的代码注释和文档注释统一使用中文，包括 `//`、`/* ... */`、`///` 和 `//!` 注释。

注释中涉及以下内容时可以保留原文：

- 类型名、函数名、变量名和配置键等代码标识符。
- HTTP、JSON、SQL、Axum、Tokio、PostgreSQL 等通用技术术语。
- 必须逐字引用的协议字段、错误文本或外部规范内容。

新增或修改代码时，应优先说明设计意图、边界条件和非显而易见的原因，避免使用只重复代码行为的无效注释。该约束的仓库级执行规则见根目录 `AGENTS.md`。

## 13. API 文档生成

接口文档以代码定义为唯一来源：

- Handler 使用 `utoipa::path` 描述路径、参数、请求体、响应模型和状态码。
- 请求、响应及通用模型通过 `ToSchema` 注册为 OpenAPI Schema。
- 查询参数模型通过 `IntoParams` 生成参数定义。
- `src/app/openapi.rs` 汇总所有接口和模型，并挂载 Swagger UI。

认证采用 Cookie 方案，因此**不注册 `bearer_auth` 安全方案**：残留的 Bearer 方案会让调用方误以为存在 `Authorization` 头这条认证路径。同理也不注册 `apiKey` 形式的 Cookie 安全方案——OpenAPI 的 `apiKey` 只能声明「安全方案」，无法表达 HttpOnly、SameSite 等实际生效的约束，不如在接口描述与文档中说明。

服务启动后可通过 `/swagger-ui/` 浏览交互式文档，通过 `/api-docs/openapi.json` 获取 OpenAPI JSON。**经 Swagger UI 调试受保护接口前，需要先调用登录接口**，浏览器会把返回的 Cookie 自动带上（Swagger UI 的请求与页面同源，因此能携带 Cookie）。新增或调整接口时，必须同步调整代码注解；手写 Markdown 只保留使用说明，不再重复维护完整的接口结构。各模块文档见 `docs/USER_API.md` 与 `docs/AUTH_API.md`。
