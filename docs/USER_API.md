# 用户模块 API 文档

本项目使用 Utoipa 从代码注解自动生成 OpenAPI 文档，接口路径、参数、请求体、响应模型和状态码均以生成结果为准，不再在 Markdown 中重复维护数据结构。

## 文档入口

服务启动后可访问：

- Swagger UI：`http://127.0.0.1:3000/swagger-ui/`
- OpenAPI JSON：`http://127.0.0.1:3000/api-docs/openapi.json`

Swagger UI 支持查看 Schema、展开接口详情并直接发起调试请求。实际地址中的主机和端口由当前环境配置决定。

## 用户接口

用户接口基础路径为 `/api/v1/users`。**本模块全部接口都要求已登录**：

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| `POST` | `/api/v1/users` | 创建用户 |
| `GET` | `/api/v1/users` | 分页获取用户列表 |
| `GET` | `/api/v1/users/{public_id}` | 获取用户详情 |
| `PUT` | `/api/v1/users/{public_id}` | 更新用户 |
| `DELETE` | `/api/v1/users/{public_id}` | 删除用户 |

未携带有效 `access_token` Cookie 时，上述接口一律返回 `401`（业务码 `40100`，`message` 为 `未登录或登录已过期`），不会进入业务逻辑，也不会返回参数校验错误。认证要求由 `/api/v1` 路由组上的认证中间件统一强制，详见[认证模块 API 文档](./AUTH_API.md)。

用户响应中的 `public_id` 是服务端生成的 UUIDv7，也是用户接口唯一对外使用的标识。数据库内部的自增主键不会通过 API 暴露。

## 缓存与一致性（Redis）

`GET /api/v1/users/{public_id}` 采用 Cache-Aside：先读 Redis（键为 `{redis.key_prefix}:user:detail:{public_id}`），未命中再查数据库并回填，存活时间由 `redis.user_cache_ttl_seconds` 控制（默认 60 秒）。

对调用方可见的两个结果：

- **更新（`PUT`）与删除（`DELETE`）成功后缓存立即被删除**，随后第一次查询必然读到最新数据。
- 缓存只是加速层，Redis 不可用时接口仍然可用（直接回源数据库）；此时若删除缓存失败，最多出现一个 TTL 长度的陈旧窗口。

列表接口刻意不缓存：分页结果与总数会被任意一次写入整体失效，命中率低而失效成本高。缓存结构、键设计与排障方法见 [Redis 学习与项目实战指南](./REDIS_GUIDE.md)。

## 请求参数校验

- 创建用户：`username` 长度为 3–50，`email` 必须是合法邮箱且不超过 100，`password` 长度为 8–72，`nickname` 不超过 100。
- 更新用户：至少提供 `email` 或 `nickname` 之一，并沿用对应字段的格式和长度限制。
- 用户列表：`page >= 1`，`page_size` 为 1–100；未传时分别默认为 1 和 10。
- 用户路径参数 `public_id` 必须是合法 UUID。
- JSON 请求体和查询参数不接受未声明字段，避免因客户端拼写错误而静默忽略输入。

参数无法解析时返回 `400`，缺少 `application/json` 媒体类型时返回 `415`，字段值校验失败时返回 `422`。

## 统一响应格式

成功和失败响应统一使用 `ApiResponse<T>` 信封：

```rust
pub struct ApiResponse<T> {
    pub code: i32,
    pub message: String,
    pub data: Option<T>,
}
```

成功响应的业务码固定为 `0`，业务数据位于 `data`：

```json
{
  "code": 0,
  "message": "查询成功",
  "data": {
    "public_id": "01991f3a-7c40-7d62-8f6d-4fb743e0f731",
    "username": "testuser"
  }
}
```

字段校验失败时，具体原因位于 `data.details`：

```json
{
  "code": 40001,
  "message": "请求参数校验失败",
  "data": {
    "details": [
      {
        "field": "email",
        "message": "邮箱格式不正确"
      }
    ]
  }
}
```

没有附加错误信息时 `data` 为 `null`。业务码定义如下：

| 业务码 | 含义 |
| ---: | --- |
| `0` | 成功 |
| `40000` | 请求参数无法解析 |
| `40001` | 参数校验失败 |
| `40002` | 请求体媒体类型错误 |
| `40100` | 未认证或凭证无效 |
| `40400` | 资源不存在 |
| `40500` | HTTP 方法不被允许 |
| `40900` | 业务冲突 |
| `42900` | 触发登录保护等频率限制 |
| `50000` | 服务器内部错误 |
| `50300` | 服务暂时不可用 |

HTTP 状态码仍保持标准语义，不统一强制返回 `200`；客户端应先处理 HTTP 状态，再使用业务码进行细分。

## 相关模块

登录、令牌刷新、登出与当前用户接口见 [认证模块 API 文档](./AUTH_API.md)。用户接口需要登录：认证由挂在 `/api/v1` 路由组上的 `common::auth::require_login` 中间件统一强制，Handler 中声明 `common::auth_extractor::CurrentUser` 只用于表达「该接口需要身份」并在需要时读取当前用户。新增接口挂在 `/api/v1` 之下即自动受保护，无需额外声明。

## 维护方式

接口发生变化时，在对应代码位置维护文档定义：

- Handler 的 `utoipa::path`：路径、参数、请求体、响应和状态码。
- Model 的 `ToSchema`：请求与响应 Schema。
- 查询参数模型的 `IntoParams`：查询参数定义。
- `src/app/openapi.rs`：统一注册新增的接口和 Schema。
- `src/modules/user/service.rs`：缓存读写与写操作后的失效逻辑（调整缓存键或 TTL 时同步更新本文档与 `docs/REDIS_GUIDE.md`）。

完成修改后运行：

```bash
cargo fmt --check
cargo clippy --all-targets
cargo test
```

## 命令行示例

用户接口需要登录，因此先用 `-c` 保存登录后的 Cookie，再用 `-b` 携带。**数据库为空时没有任何账号能登录**（创建用户接口自身也要求登录），需要先按 `docs/AUTH_API.md` 的「首次启动：第一个账号从哪里来」创建引导账号：

```bash
# 0. 登录并保存令牌 Cookie
#    用户名与口令取自引导账号配置（bootstrap.username / bootstrap.password），
#    详见 docs/AUTH_API.md 的「首次启动」一节。
curl -c cookies.txt -X POST http://127.0.0.1:3000/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"your-admin","password":"your-bootstrap-password"}'

# 创建用户
curl -b cookies.txt -X POST http://127.0.0.1:3000/api/v1/users \
  -H "Content-Type: application/json" \
  -d '{"username":"testuser","email":"test@example.com","password":"password123","nickname":"测试用户"}'

# 获取用户列表
curl -b cookies.txt "http://127.0.0.1:3000/api/v1/users?page=1&page_size=10"

# 获取用户详情
curl -b cookies.txt http://127.0.0.1:3000/api/v1/users/01991f3a-7c40-7d62-8f6d-4fb743e0f731

# 更新用户
curl -b cookies.txt -X PUT http://127.0.0.1:3000/api/v1/users/01991f3a-7c40-7d62-8f6d-4fb743e0f731 \
  -H "Content-Type: application/json" \
  -d '{"email":"newemail@example.com","nickname":"新昵称"}'

# 删除用户
curl -b cookies.txt -X DELETE http://127.0.0.1:3000/api/v1/users/01991f3a-7c40-7d62-8f6d-4fb743e0f731
```
