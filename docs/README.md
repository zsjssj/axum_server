# Rust 后端学习手册

这是一套**按学习顺序**编排的教程，目标不是"介绍这个项目用到了什么"，而是带你从零理解 **一个 Rust 后端是怎样被想出来、拆开、写出来、再守住的**。所有讲解都对照本仓库的真实代码，你可以边读边打开对应文件。

- 想直接查某个接口怎么调 → 看 [`reference/AUTH_API.md`](./reference/AUTH_API.md)、[`reference/USER_API.md`](./reference/USER_API.md)、[`reference/INFORMATION_API.md`](./reference/INFORMATION_API.md)
- 想查架构总览与设计清单 → 看 [`reference/ARCHITECTURE.md`](./reference/ARCHITECTURE.md)
- 想深入 Redis → 看 [`reference/REDIS_GUIDE.md`](./reference/REDIS_GUIDE.md)
- **想学会写 Rust 后端 → 从下面第 01 章顺着读**

---

## 1. 这套文档给谁看

| 你的背景 | 建议读法 |
| --- | --- |
| 会 Rust 语法，但没写过后端 | 01 可以快速跳过，重点读 02 → 05 → 07 → 08 |
| 写过 Java/Go/Node 后端，刚学 Rust | **按顺序通读**，01 是最省时间的一章 |
| 能用 Axum 写 CRUD，想搞清楚工程化 | 跳过 01、02，重点读 03、07、08、09、10、14 |
| 只想照着加一个模块 | 直接读 [13 动手扩展新模块](./13-动手扩展新模块.md)，遇阻再回查 |

阅读约定：

- `src/xxx.rs:12` 表示对应文件的第 12 行；代码变动后行号可能漂移，用搜索函数名定位更稳。
- 文中 `重点` 标记的内容，通常是**顺序错误就会产生安全漏洞或数据错误**的地方。
- 每章末尾有「动手练习」和「自测题」。**练习比阅读重要**：后端能力来自"改坏了再修好"。

## 2. 学习路线图

```text
阶段一：建立心智模型         阶段二：看懂一条请求         阶段三：看懂状态与安全
┌──────────────────┐      ┌──────────────────┐      ┌──────────────────┐
│ 01 Rust 后端心智模型 │ ──→  │ 05 请求生命周期     │ ──→  │ 08 状态与中间件     │
│ 02 项目全景与启动     │      │ 06 Service 业务规则 │      │ 09 认证与授权       │
│ 03 配置系统         │      │ 07 统一响应与错误   │      │ 10 Redis 与降级     │
│ 04 数据库与 SQLx    │      └──────────────────┘      └──────────────────┘
└──────────────────┘                  │                        │
                                      ▼                        ▼
                        阶段四：工程化          阶段五：自己造
                        ┌──────────────────┐    ┌──────────────────┐
                        │ 11 可观测性        │    │ 13 动手扩展新模块   │
                        │ 12 API 文档与测试  │ ──→ │ 14 生产化与进阶     │
                        └──────────────────┘    └──────────────────┘
```

一句话主线：**一个 HTTP 请求从 Router 进入，被提取器解析校验，经 Service 组织业务规则，由 Repository 落到 PostgreSQL；成功与失败都收口到同一套响应信封；而认证、限流、缓存这些横切能力挂在中间件和适配层上，并且每一处都明确写下"依赖挂了会怎样"。**

## 3. 章节索引

| 章节 | 你会学到 | 主要对应代码 |
| --- | --- | --- |
| [01 从 Rust 到后端](./01-从Rust到后端.md) | 所有权、`Result`、async 在后端语境下的真实含义 | `src/main.rs` |
| [02 项目全景与启动流程](./02-项目全景与启动流程.md) | 分层、模块树、启动顺序，以及每一步失败该不该退出 | `src/main.rs`、`src/app/mod.rs` |
| [03 配置系统](./03-配置系统.md) | 强类型配置、分层覆盖、启动期校验 | `src/config/mod.rs`、`config/*.toml` |
| [04 数据库与 SQLx](./04-数据库与SQLx.md) | 连接池、迁移、Repository 写法、双 ID 设计 | `src/infrastructure/database.rs`、`src/modules/user/repository.rs`、`migrations/` |
| [05 请求生命周期](./05-请求生命周期.md) | Router、提取器、Handler 签名规则 | `src/app/routes.rs`、`src/common/extractor.rs` |
| [06 Service 层](./06-Service层与业务规则.md) | 业务规则、阻塞任务、错误归类、顺序即正确性 | `src/modules/user/service.rs` |
| [07 统一响应与错误处理](./07-统一响应与错误处理.md) | 响应信封、业务码、`AppError` 与 `IntoResponse` | `src/common/response.rs`、`src/common/error.rs` |
| [08 全局状态与中间件](./08-全局状态与中间件.md) | `AppState`、`tower` 中间件、中间件顺序 | `src/app/state.rs`、`src/common/auth.rs`、`src/app/middleware.rs` |
| [09 认证与授权](./09-认证与授权.md) | JWT、Cookie、双令牌轮换、重放检测、限流 | `src/modules/auth/*`、`src/common/cookie.rs` |
| [10 Redis 与降级设计](./10-Redis与降级设计.md) | 缓存、滑动窗口、吊销列表、降级矩阵 | `src/infrastructure/redis.rs` |
| [11 可观测性](./11-可观测性与健康检查.md) | 结构化日志、`/health` 与 `/ready` | `src/main.rs`、`src/app/middleware.rs`、`src/app/routes.rs` |
| [12 API 文档与测试](./12-API文档与测试.md) | utoipa、Swagger、单元/集成测试策略 | `src/app/openapi.rs`、各文件 `mod tests` |
| [13 动手扩展新模块](./13-动手扩展新模块.md) | 从零加一个完整业务模块的 10 步清单 | 全仓库 |
| [14 生产化与进阶](./14-生产化与进阶.md) | 上线清单、性能、已知缺口、学习路线 | — |

参考手册（不对应学习顺序，按需查阅）：

- [reference/ARCHITECTURE.md](./reference/ARCHITECTURE.md)：架构、启动流程、认证设计、错误码、扩展方式、聊天模块设计
- [reference/AUTH_API.md](./reference/AUTH_API.md)：认证接口、令牌语义、客户端约束、配置表
- [reference/USER_API.md](./reference/USER_API.md)：用户接口、校验规则、业务码
- [reference/INFORMATION_API.md](./reference/INFORMATION_API.md)：聊天接口（会话/消息/已读/撤回）、幂等发送、权限与错误码
- [reference/REDIS_GUIDE.md](./reference/REDIS_GUIDE.md)：Redis 概念、五种数据类型、Lua、五个实验、排错

## 4. 先把环境跑起来

学习后端最忌讳"只读不跑"。下面四步能让你在 10 分钟内得到一个可登录的服务。

### 4.1 依赖

| 组件 | 版本要求 | 用途 |
| --- | --- | --- |
| Rust | 1.85+（项目使用 edition 2024） | 编译 |
| PostgreSQL | 13+ | 权威数据 |
| Redis | 6+ | 限流、令牌吊销、缓存（可缺省，功能降级） |
| Docker（可选） | — | 快速起 Postgres/Redis |

```bash
# Postgres（示例）
docker run --rm -p 5432:5432 -e POSTGRES_PASSWORD=postgres postgres:16

# Redis（示例）
docker run --rm -p 6379:6379 redis:7
```

> **PostgreSQL 必须返回 UTF-8 兼容的错误消息**，否则 SQLx 无法解析任何服务端错误，所有失败都会变成含义模糊的协议错误。在 `postgresql.conf` 里设置 `lc_messages = 'C'`。原因见 [reference/ARCHITECTURE.md 第 4 节](./reference/ARCHITECTURE.md)。

### 4.2 配置

配置有四级来源，后者覆盖前者：`config/default.toml` → `config/{RUN_MODE}.toml` → `config/local.toml`（不提交）→ `APP__*` 环境变量。细节见 [03 配置系统](./03-配置系统.md)。

```bash
cp .env.example .env            # 可选：本地环境变量
# 敏感项写入 config/local.toml（已被 .gitignore 排除）
```

### 4.3 第一个账号

`/api/v1` 下的接口默认全部需要登录，而"创建用户"接口自己也在里面——空库时没人能登录。项目为此提供了启动期引导账号（默认关闭）：

```toml
# config/local.toml
[bootstrap]
enabled = true
username = "admin"
email = "admin@example.com"
password = "换成你的强口令"
```

启动一次、创建成功后再把 `enabled` 改回 `false`。

### 4.4 启动并验证

```bash
cargo run
```

```bash
# 1) 健康检查（进程活着）
curl http://127.0.0.1:3000/health

# 2) 就绪检查（数据库 + Redis 都可用才返回 200）
curl http://127.0.0.1:3000/ready

# 3) 登录，把令牌 Cookie 存到文件
curl -c cookies.txt -X POST http://127.0.0.1:3000/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"换成你的强口令"}'

# 4) 带着 Cookie 访问受保护接口
curl -b cookies.txt http://127.0.0.1:3000/api/v1/auth/me
curl -b cookies.txt http://127.0.0.1:3000/api/v1/users

# 5) 不带 Cookie 访问同一个接口 → 必须 401，业务码 40100
curl http://127.0.0.1:3000/api/v1/users
```

浏览器里打开 `http://127.0.0.1:3000/swagger-ui/` 可以看到自动生成的接口文档；调试受保护接口前先调登录接口，浏览器会自动带上 Cookie。

### 4.5 动手前的自检

```bash
cargo fmt --check      # 格式
cargo clippy --all-targets   # 静态检查
cargo test             # 单元测试 + 集成测试（不需要真实数据库）
```

`cargo test` 应输出 **129 passed**，且不依赖数据库或 Redis。

> **关于 `cargo fmt --check`**：`rustfmt.toml` 里启用了一些 nightly-only 选项（`wrap_comments`、`trailing_comma`、`format_strings` 等），stable 工具链会忽略它们并给出警告；因此在 stable 下 `cargo fmt --check` 会报出约 120 处"应当合并成一行"的差异。这属于仓库既有的工具链差异，**不影响编译和测试**。实用做法是本地改完代码跑一次 `cargo fmt`，并以 `cargo clippy --all-targets` 和 `cargo test` 作为真正的门禁；若要严格卡格式，需要在 CI 里固定工具链并使用 nightly 的 rustfmt。

## 5. 代码地图

| 路径 | 职责 | 对应章节 |
| --- | --- | --- |
| `src/main.rs` | 程序入口，串联配置、日志、数据库、Redis、应用与监听 | 02、11 |
| `src/app/mod.rs` | `create_app`：装配 Router、中间件、State | 02、08 |
| `src/app/routes.rs` | 健康检查 + `/api/v1` 路由组与认证中间件挂载 | 05、08 |
| `src/app/state.rs` | `AppState`：数据库池、Redis、JWT/Auth 配置 | 08 |
| `src/app/middleware.rs` | CORS 与 `TraceLayer` 日志 | 08、11 |
| `src/app/openapi.rs` | OpenAPI 文档与 Swagger UI 注册 | 12 |
| `src/app/bootstrap.rs` | 启动期引导账号 | 02、09 |
| `src/config/mod.rs` | 配置结构与启动期校验 | 03 |
| `src/common/error.rs` | `AppError` 统一错误边界 | 07 |
| `src/common/response.rs` | `ApiResponse` 信封与 `ApiCode` | 07 |
| `src/common/extractor.rs` | 带校验的 JSON/Query/Path 提取器 | 05 |
| `src/common/auth.rs` | 认证中间件与匿名白名单 | 08、09 |
| `src/common/auth_extractor.rs` | `CurrentUser` 提取器 | 08、09 |
| `src/common/cookie.rs` | Cookie 解析与安全属性构建 | 09 |
| `src/common/pagination.rs` | 分页参数与分页响应 | 04、07 |
| `src/common/password.rs` | Argon2 密码哈希 | 06、09 |
| `src/infrastructure/database.rs` | 连接池与迁移 | 04 |
| `src/infrastructure/redis.rs` | Redis 适配层（键、脚本、缓存原语） | 10 |
| `src/modules/user/*` | 用户模块：model/repository/service/handler | 04、05、06 |
| `src/modules/auth/*` | 认证模块：登录、刷新、登出、当前用户 | 09 |
| `src/modules/information/*` | 聊天消息模块：会话、成员、消息、已读回执、撤回 | 04、06、07、13 |
| `migrations/*.sql` | 数据库结构演进 | 04 |
| `config/*.toml` | 分层配置 | 03 |

## 6. 文档维护约定

这套文档的分工是**单一来源**，避免同一件事写在两处后各自漂移：

| 内容 | 唯一来源 | 说明 |
| --- | --- | --- |
| 接口路径、参数、请求体、响应、状态码 | 代码里的 `#[utoipa::path]` 注解 | 由 `/api-docs/openapi.json` 与 Swagger UI 呈现；Markdown 只写使用方式与语义 |
| 架构分层、启动流程、关键设计取舍 | [`reference/ARCHITECTURE.md`](./reference/ARCHITECTURE.md) | 路由、配置、模块边界变更时必须同步 |
| 概念与学习路径 | 本目录各章节 | 讲解"为什么"，不重复维护接口细节 |
| Redis 概念与实验 | [`reference/REDIS_GUIDE.md`](./reference/REDIS_GUIDE.md) | 键空间、脚本、排错 |

改代码时请顺手确认：

1. 新增/调整接口 → 同步 `#[utoipa::path]` 注解与 `src/app/openapi.rs` 注册。
2. 路由、配置项、模块边界变化 → 同步 `reference/ARCHITECTURE.md` 与相关章节。
3. 数据库结构变化 → **新增**迁移文件，不修改历史迁移。
4. 提交前跑 `cargo fmt --check && cargo clippy --all-targets && cargo test`。
5. 代码注释统一使用中文（见根目录 `AGENTS.md`）。

---

下一步 → [01 从 Rust 到后端：先把心智模型摆正](./01-从Rust到后端.md)
