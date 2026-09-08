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
