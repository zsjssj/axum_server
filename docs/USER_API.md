# 用户模块 API 文档

本项目使用 Utoipa 从代码注解自动生成 OpenAPI 文档，接口路径、参数、请求体、响应模型和状态码均以生成结果为准，不再在 Markdown 中重复维护数据结构。

## 文档入口

服务启动后可访问：

- Swagger UI：`http://127.0.0.1:3000/swagger-ui/`
- OpenAPI JSON：`http://127.0.0.1:3000/api-docs/openapi.json`

Swagger UI 支持查看 Schema、展开接口详情并直接发起调试请求。实际地址中的主机和端口由当前环境配置决定。

## 用户接口

用户接口基础路径为 `/api/v1/users`：

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| `POST` | `/api/v1/users` | 创建用户 |
| `GET` | `/api/v1/users` | 分页获取用户列表 |
| `GET` | `/api/v1/users/{public_id}` | 获取用户详情 |
| `PUT` | `/api/v1/users/{public_id}` | 更新用户 |
| `DELETE` | `/api/v1/users/{public_id}` | 删除用户 |

用户响应中的 `public_id` 是服务端生成的 UUIDv7，也是用户接口唯一对外使用的标识。数据库内部的自增主键不会通过 API 暴露。

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
| `40400` | 资源不存在 |
| `40500` | HTTP 方法不被允许 |
| `40900` | 业务冲突 |
| `50000` | 服务器内部错误 |
| `50300` | 服务暂时不可用 |

HTTP 状态码仍保持标准语义，不统一强制返回 `200`；客户端应先处理 HTTP 状态，再使用业务码进行细分。

## 维护方式

接口发生变化时，在对应代码位置维护文档定义：

- Handler 的 `utoipa::path`：路径、参数、请求体、响应和状态码。
- Model 的 `ToSchema`：请求与响应 Schema。
- 查询参数模型的 `IntoParams`：查询参数定义。
- `src/app/openapi.rs`：统一注册新增的接口和 Schema。

完成修改后运行：

```bash
cargo fmt --check
cargo test
```

## 命令行示例

```bash
# 创建用户
curl -X POST http://127.0.0.1:3000/api/v1/users \
  -H "Content-Type: application/json" \
  -d '{"username":"testuser","email":"test@example.com","password":"password123","nickname":"测试用户"}'

# 获取用户列表
curl "http://127.0.0.1:3000/api/v1/users?page=1&page_size=10"

# 获取用户详情
curl http://127.0.0.1:3000/api/v1/users/01991f3a-7c40-7d62-8f6d-4fb743e0f731

# 更新用户
curl -X PUT http://127.0.0.1:3000/api/v1/users/01991f3a-7c40-7d62-8f6d-4fb743e0f731 \
  -H "Content-Type: application/json" \
  -d '{"email":"newemail@example.com","nickname":"新昵称"}'

# 删除用户
curl -X DELETE http://127.0.0.1:3000/api/v1/users/01991f3a-7c40-7d62-8f6d-4fb743e0f731
```
