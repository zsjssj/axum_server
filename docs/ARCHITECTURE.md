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
- Utoipa + Swagger UI：从代码注解生成 OpenAPI 文档并提供交互式浏览入口。

## 2. 目录与职责

```text
rust-axum-server/
├── config/                 # 默认配置和分环境配置
├── docs/                   # 架构与接口文档
├── migrations/             # 数据库迁移脚本
└── src/
    ├── app/                # 应用装配、全局状态、路由和中间件
    ├── common/             # 跨业务模块复用的错误、分页等能力
    ├── config/             # 配置结构定义与加载逻辑
    ├── infrastructure/     # 数据库等外部基础设施适配
    ├── modules/            # 业务模块
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
3. 根据日志级别初始化 `tracing` 订阅器。
4. 根据数据库配置创建 PostgreSQL 连接池。
5. SQLx 执行编译进程序的 `migrations` 迁移；迁移失败时终止启动，避免服务在数据库结构不完整时接收流量。
6. `create_app` 创建 `AppState`，组装健康检查路由、API 路由和中间件。
7. 根据主机与端口配置绑定 TCP 监听器。
8. Axum 接管监听器并开始处理请求。
8. 任一启动步骤失败时，错误返回到 `main`，程序输出错误并以非零状态退出。

```text
main
  └─ run
      ├─ 加载配置
      ├─ 初始化日志
      ├─ 创建数据库连接池
      ├─ 执行数据库迁移
      ├─ 组装 Axum Router 与 AppState
      ├─ 绑定 TCP 地址
      └─ 启动 HTTP 服务
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

## 6. 全局状态与中间件

`AppState` 保存可以安全共享的应用级依赖，当前仅包含 `PgPool`。Axum 会克隆 State，但 `PgPool` 内部使用共享句柄，因此不会为每次请求创建新的连接池。

当前中间件包括：

- 日志中间件：记录方法、URI、响应状态和处理耗时。
- CORS 中间件：当前允许任意来源、方法和请求头，便于开发联调；生产环境应根据实际前端域名收紧规则。

新增全局依赖时，应优先加入 `AppState`，由 Handler 通过 `State` 提取，避免使用全局可变变量。

## 7. 错误处理设计

`AppError` 是应用统一错误边界，负责将内部错误转换为 HTTP 状态码和 JSON 响应：

- `BadRequest` → `400 Bad Request`
- `Validation` / `UnprocessableEntity` → `422 Unprocessable Entity`
- `UnsupportedMediaType` → `415 Unsupported Media Type`
- `NotFound` → `404 Not Found`
- `MethodNotAllowed` → `405 Method Not Allowed`
- `Conflict` → `409 Conflict`
- `ServiceUnavailable` → `503 Service Unavailable`
- `Database` → `500 Internal Server Error`
- `Internal` → `500 Internal Server Error`

所有成功和失败响应均使用 `ApiResponse<T> { code, message, data }` 信封。数字 `code` 与 HTTP 状态码职责分离：HTTP 状态码供网关、监控和通用客户端判断协议结果，业务码供前端稳定处理，其中成功固定为 `0`。成功时 `data` 保存业务模型；失败时通常为 `null`，参数校验失败时保存 `{ details: [{ field, message }] }`，以保留具体字段原因。自定义 JSON、Query、Path 提取器以及 Router 的 404、405 fallback 可防止 Axum 默认纯文本拒绝响应绕过该结构。

业务码按错误类别分段：请求格式 `40000`、参数校验 `40001`、媒体类型 `40002`、资源不存在 `40400`、方法不允许 `40500`、业务冲突 `40900`、内部错误 `50000`、服务不可用 `50300`。新增业务模块时应在对应号段中定义更具体的业务码，避免直接复用无语义的字符串。

数据库错误的详细内容只写入服务端日志，对客户端统一返回“服务器内部错误”，避免泄露数据库结构、SQL 或连接信息。业务模块应尽量把可预期的底层错误转换为明确的业务错误，其他数据库错误通过 `From<sqlx::Error>` 进入统一处理。

## 8. 数据与安全设计

- 密码仅以 Argon2 哈希形式写入数据库，API 响应和查询字段均不包含 `password_hash`。
- 用户采用内部 `BIGINT IDENTITY` 主键与公开 UUIDv7 双 ID。内部主键用于数据库关联和排序，不通过 API 暴露；UUIDv7 由 Service 在创建用户时生成，作为接口路径、响应和跨系统传递使用的稳定标识。
- 所有 SQL 使用 SQLx 参数绑定，不拼接用户输入。
- 数据库连接通过连接池复用，并设置连接获取超时。
- 健康检查 `/health` 只表示进程可响应；就绪检查 `/ready` 会执行数据库探测，用于判断服务是否具备接收业务流量的条件。
- 数据库结构通过 `migrations` 目录中的脚本维护；迁移脚本会在编译时嵌入程序，并在服务启动、开始监听端口前自动执行。
- 已发布或可能已执行的结构变更应新增迁移文件，不直接修改历史迁移；任何迁移失败都会中止服务启动。当前用户表尚未投入使用，因此初始迁移可以随首次建表方案调整。

## 9. 模块扩展方式

新增业务模块时，建议沿用用户模块的组织方式：

```text
src/modules/{module}/
├── mod.rs
├── handler.rs
├── model.rs
├── repository.rs
└── service.rs
```

推荐步骤：

1. 在 `model.rs` 定义实体、请求和响应数据结构。
2. 在 `repository.rs` 实现最小粒度的数据访问操作。
3. 在 `service.rs` 组合业务规则并处理业务错误。
4. 在 `handler.rs` 定义路由和 HTTP 适配逻辑。
5. 在模块的 `mod.rs` 暴露路由入口。
6. 在 `src/modules/mod.rs` 注册模块。
7. 在 `src/app/routes.rs` 合并模块路由。
8. 补充迁移、测试和对应 API 文档。

如果业务逐渐复杂，可以在模块内部继续拆分领域对象或子模块，但仍需保持 HTTP、业务和持久化职责之间的边界。

## 10. 设计原则

- 按业务模块聚合：同一业务的 Handler、Service、Repository 和 Model 放在同一模块中，减少跨目录跳转。
- 分层但不过度抽象：当前直接使用具体类型和静态方法；只有在出现真实的替换、复用或测试隔离需求时，再引入 Trait 等抽象。
- 统一横切能力：错误、分页、配置、日志和共享状态集中管理，避免每个模块重复实现。
- 异步路径避免阻塞：数据库和网络操作使用异步接口；CPU 密集或阻塞操作使用专用阻塞线程池。
- 默认保护敏感信息：内部错误记录到日志，对外只返回稳定且必要的信息。
- 文档随实现更新：路由、模块边界、配置优先级或关键设计发生变化时，应同步更新本文件和对应 API 文档。

## 11. 注释语言约束

本项目的代码注释和文档注释统一使用中文，包括 `//`、`/* ... */`、`///` 和 `//!` 注释。

注释中涉及以下内容时可以保留原文：

- 类型名、函数名、变量名和配置键等代码标识符。
- HTTP、JSON、SQL、Axum、Tokio、PostgreSQL 等通用技术术语。
- 必须逐字引用的协议字段、错误文本或外部规范内容。

新增或修改代码时，应优先说明设计意图、边界条件和非显而易见的原因，避免使用只重复代码行为的无效注释。该约束的仓库级执行规则见根目录 `AGENTS.md`。

## 12. API 文档生成

接口文档以代码定义为唯一来源：

- Handler 使用 `utoipa::path` 描述路径、参数、请求体、响应模型和状态码。
- 请求、响应及通用模型通过 `ToSchema` 注册为 OpenAPI Schema。
- 查询参数模型通过 `IntoParams` 生成参数定义。
- `src/app/openapi.rs` 汇总所有接口和模型，并挂载 Swagger UI。

服务启动后可通过 `/swagger-ui/` 浏览交互式文档，通过 `/api-docs/openapi.json` 获取 OpenAPI JSON。新增或调整接口时，必须同步调整代码注解；手写 Markdown 只保留使用说明，不再重复维护完整的接口结构。
