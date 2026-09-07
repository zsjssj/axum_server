# 用户模块 API 文档

## 基础路径
所有用户相关 API 的基础路径为 `/api/v1/users`

## API 端点

### 1. 创建用户
**POST** `/api/v1/users`

**请求体：**
```json
{
  "username": "testuser",
  "email": "test@example.com",
  "password": "password123",
  "nickname": "测试用户"
}
```

**响应：** (201 Created)
```json
{
  "id": 1,
  "username": "testuser",
  "email": "test@example.com",
  "nickname": "测试用户",
  "created_at": "2026-09-07 12:00:00",
  "updated_at": "2026-09-07 12:00:00"
}
```

### 2. 获取用户列表
**GET** `/api/v1/users?page=1&page_size=10`

**查询参数：**
- `page`: 页码（默认 1，小于 1 时按 1 处理）
- `page_size`: 每页数量（默认 10，范围 1–100）

**响应：**
```json
{
  "data": [
    {
      "id": 1,
      "username": "testuser",
      "email": "test@example.com",
      "nickname": "测试用户",
      "created_at": "2026-09-07 12:00:00",
      "updated_at": "2026-09-07 12:00:00"
    }
  ],
  "total": 50,
  "page": 1,
  "page_size": 10,
  "total_pages": 5
}
```

### 3. 获取单个用户
**GET** `/api/v1/users/:id`

**响应：**
```json
{
  "id": 1,
  "username": "testuser",
  "email": "test@example.com",
  "nickname": "测试用户",
  "created_at": "2026-09-07 12:00:00",
  "updated_at": "2026-09-07 12:00:00"
}
```

### 4. 更新用户
**PUT** `/api/v1/users/:id`

**请求体：**
```json
{
  "email": "newemail@example.com",
  "nickname": "新昵称"
}
```

**响应：**
```json
{
  "id": 1,
  "username": "testuser",
  "email": "newemail@example.com",
  "nickname": "新昵称",
  "created_at": "2026-09-07 12:00:00",
  "updated_at": "2026-09-07 12:30:00"
}
```

### 5. 删除用户
**DELETE** `/api/v1/users/:id`

**响应：**
```json
{
  "message": "用户删除成功"
}
```

## 错误响应

**404 Not Found**
```json
{
  "error": "用户不存在"
}
```

**409 Conflict**
```json
{
  "error": "用户名已存在"
}
```

**500 Internal Server Error**
```json
{
  "error": "服务器内部错误"
}
```

## 测试命令

```bash
# 创建用户
curl -X POST http://127.0.0.1:3000/api/v1/users \
  -H "Content-Type: application/json" \
  -d '{"username":"testuser","email":"test@example.com","password":"password123","nickname":"测试用户"}'

# 获取用户列表
curl http://127.0.0.1:3000/api/v1/users?page=1&page_size=10

# 获取单个用户
curl http://127.0.0.1:3000/api/v1/users/1

# 更新用户
curl -X PUT http://127.0.0.1:3000/api/v1/users/1 \
  -H "Content-Type: application/json" \
  -d '{"email":"newemail@example.com","nickname":"新昵称"}'

# 删除用户
curl -X DELETE http://127.0.0.1:3000/api/v1/users/1
```
