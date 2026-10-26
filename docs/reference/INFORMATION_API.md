# 聊天消息模块 API 文档

> **文档定位**：本文是**参考手册**，用于查阅既定事实与操作步骤。
> 想理解设计取舍与并发细节，见 [`ARCHITECTURE.md`](./ARCHITECTURE.md) 第 14 节。

模块代码位于 `src/modules/information`，提供「类似微信/QQ 的聊天」所需的服务端能力：会话（单聊/群聊）、成员管理、消息收发、未读与已读回执、消息撤回。

接口路径、参数、请求体、响应模型和状态码均以 Utoipa 生成的 OpenAPI 文档为准，本文不重复维护数据结构。

## 文档入口

服务启动后可访问：

- Swagger UI：`http://127.0.0.1:3000/swagger-ui/`
- OpenAPI JSON：`http://127.0.0.1:3000/api-docs/openapi.json`

**调试前先调用登录接口**：全部聊天接口都要求登录，浏览器会把 Cookie 自动带上。

## 核心概念

| 概念 | 说明 |
| --- | --- |
| 会话（conversation） | 一次对话。`kind` 为 `single`（单聊）或 `group`（群聊）。单聊与群聊共用同一套成员、消息与已读结构 |
| 成员（member） | 用户与会话的关系，`role` 为 `owner`（群主）或 `member`。**全部权限判断只看成员表** |
| 消息序号 `seq` | **会话内**从 1 开始递增的序号，是历史消息的翻页游标。它不是数据库主键，因此不会泄露全站消息总量 |
| 已读位置 `last_read_seq` | 我在该会话读到的最新序号，只能前进不能后退 |
| 未读数 `unread_count` | 别人发的、序号大于我的已读位置的消息条数 |
| 幂等键 `client_message_id` | 客户端为一条消息生成的唯一标识（≤64 字符）。同一个键重复发送只会落一条消息 |

对外标识统一使用 `public_id`（UUIDv7）；数据库内部自增主键不通过 API 暴露。

## 接口清单

基础路径为 `/api/v1`，**全部接口都要求已登录**：

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| `POST` | `/conversations` | 创建会话（单聊/群聊） |
| `GET` | `/conversations` | 我的会话列表（含未读数与最后一条消息） |
| `GET` | `/conversations/{public_id}` | 会话详情（含成员列表） |
| `POST` | `/conversations/{public_id}/messages` | 发送消息 |
| `GET` | `/conversations/{public_id}/messages` | 拉取历史消息（游标分页） |
| `POST` | `/conversations/{public_id}/read` | 标记会话已读 |
| `POST` | `/conversations/{public_id}/members` | 添加群成员（仅群主） |
| `DELETE` | `/conversations/{public_id}/members/me` | 退出群聊 |
| `POST` | `/messages/{public_id}/recall` | 撤回消息 |

未携带有效 `access_token` Cookie 时一律返回 `401`（业务码 `40100`），不会进入业务逻辑。认证由 `/api/v1` 路由组上的中间件统一强制。

## 创建会话

请求体：

```json
{
  "kind": "single",
  "member_public_ids": ["01991f3a-7c40-7d62-8f6d-4fb743e0f731"]
}
```

```json
{
  "kind": "group",
  "title": "项目讨论组",
  "member_public_ids": [
    "01991f3a-7c40-7d62-8f6d-4fb743e0f731",
    "01991f3a-7c40-7d62-8f6d-4fb743e0f732"
  ]
}
```

`member_public_ids` 只列**对方**，创建者自动加入：单聊由服务端补成两个成员，群聊则把创建者设为群主。

| 规则 | 说明 |
| --- | --- |
| 单聊成员数 | 必须且只能是 1 个，且不能是自己 |
| 单聊标题 | 不接受，传了返回 `422`（单聊标题恒为对方昵称） |
| 群聊成员数 | 至少 1 个其他成员，含创建者在内不超过 200 人 |
| 群聊标题 | 可选，最长 100 字符，全空白也算非法 |
| 未知用户 | 任何一个 ID 不存在即返回 `404`「指定的用户不存在」，不会静默忽略 |

### 单聊是幂等的

同一对用户永远只有一个单聊会话，请求方向无关（A 找 B 与 B 找 A 命中同一个会话）：

- 真正新建 → `201`，`message` 为「会话创建成功」；
- 会话已存在 → `200`，`message` 为「会话已存在」，返回的是同一个 `public_id`。

客户端因此可以放心重复调用，不必先查询是否已存在。

## 我的会话列表

`GET /conversations?page=1&page_size=10`，按「最后一条消息时间」倒序（从未发过消息的会话按创建时间排序）。

每一项除会话基本信息外还包含：

| 字段 | 说明 |
| --- | --- |
| `unread_count` | 未读数 |
| `last_read_seq` | 我的已读位置 |
| `message_seq` | 会话当前最新序号，可用于判断「我是否已读到最新」 |
| `last_message` | 最后一条消息；会话还没有消息时为 `null` |
| `member_count` | 成员数 |

`last_message` 中撤回的消息 `content` 为空串、`recalled` 为 `true`，客户端应显示「某条消息已撤回」而不是空气泡。

## 发送消息

`POST /conversations/{public_id}/messages`

```json
{
  "content": "你好",
  "content_type": "text",
  "client_message_id": "c-20240506-0001",
  "reply_to_public_id": "01991f3a-7c40-7d62-8f6d-4fb743e0f799"
}
```

| 字段 | 说明 |
| --- | --- |
| `content` | `text` 为原文（≤4000 字符，按**字符数**而非字节数），`image`/`file` 为资源标识或 URL（≤2048 字符）。不能为空或全空白 |
| `content_type` | 可选，`text`（默认）/`image`/`file`；其他取值在反序列化阶段即被拒绝（`422`） |
| `client_message_id` | 可选，幂等键，≤64 字符 |
| `reply_to_public_id` | 可选，被引用消息必须**属于同一个会话**，否则 `422` |

成功返回 `201` 与创建出的消息（含服务端分配的 `seq`）。

### 幂等发送

带同一个 `client_message_id` 重复发送时，返回的是**第一次**那条消息（相同的 `public_id` 与 `seq`），不会产生第二条。弱网重试因此可以直接重发，不必先确认上一次请求是否到达服务端。

## 拉取历史消息

`GET /conversations/{public_id}/messages?limit=20&before_seq=100`

| 参数 | 说明 |
| --- | --- |
| `limit` | 每页条数，1–100，默认 20 |
| `before_seq` | 只返回序号小于该值的消息；省略时从最新一条开始 |

返回按 `seq` **倒序**（最新在前）的 `items`，外加：

- `has_more`：是否还有更早的消息；
- `next_before_seq`：下一页游标，把它作为下一次请求的 `before_seq` 即可；没有更早消息时为 `null`。

翻到底的条件是 `has_more == false`。`next_before_seq` 取自本页最后一条（最旧一条）的序号，因此不会漏消息也不会重复。

## 已读回执

`POST /conversations/{public_id}/read`（无请求体），把已读位置推进到会话**当前最新序号**，返回：

```json
{ "conversation_public_id": "…", "last_read_seq": 42, "unread_count": 0 }
```

- 接口不接受客户端指定读到哪里：位置由服务端决定，所以不存在「上报一个很大的数字把未读一次清空」的问题。
- `unread_count` 正常为 0；大于 0 只可能是推进那一刻之后又到达了新消息。
- 已读位置只能前进：重复调用或乱序调用都不会把它往回拨。

## 撤回消息

`POST /messages/{public_id}/recall`（无请求体），成功后返回该消息，`content` 为空串、`recalled` 为 `true`。

| 情况 | 结果 |
| --- | --- |
| 撤回自己 2 分钟内发出的消息 | 成功 |
| 撤回自己超过 2 分钟的消息 | `409`「超过可撤回时间」 |
| 群主撤回群内任意消息 | 成功（管理行为，不受时间限制） |
| 普通成员撤回他人消息 | `403`「只能撤回自己发送的消息」 |
| 重复撤回 | `409`「消息已撤回」 |
| 不是该会话成员 | `404`「消息不存在」 |

撤回是「标记 + 清空正文」，记录本身保留：历史序号不会出现空洞，客户端也能显示「这里原本有一条消息」。**正文一旦撤回就无法通过接口读回。**

## 添加群成员与退出群聊

### 添加群成员

`POST /conversations/{public_id}/members`，请求体 `{"member_public_ids": ["…"]}`，每次 1–50 人。

| 情况 | 结果 |
| --- | --- |
| 群主添加新成员 | 成功，返回更新后的会话详情 |
| 普通成员尝试添加 | `403`「只有群主可以添加成员」 |
| 单聊加人 | `409`「单聊会话不能添加成员」 |
| 所选用户全部已在群里 | `409`「所选用户已全部在会话中」 |
| 添加后超过 200 人 | `409`「群聊成员数量不能超过 200 人」 |

### 退出群聊

`DELETE /conversations/{public_id}/members/me`

- 普通成员退出：直接离开，之后读不到该会话（`404`）。
- **群主退出**：群主身份自动转让给「最早加入的剩余成员」。
- **最后一个成员退出**：会话连同消息一起删除。
- 单聊不支持退出，返回 `409`「单聊会话不支持退出」——退出一个两人的会话等于删除对话，语义与「清空聊天记录」混在一起会让对方看到一半的历史。

## 权限与错误码

| 业务码 | HTTP | 含义 |
| ---: | --- | --- |
| `0` | 200/201 | 成功 |
| `40000` | 400 | 路径或查询参数无法解析 |
| `40001` | 422 | 参数校验失败 |
| `40002` | 415 | 请求体媒体类型错误 |
| `40100` | 401 | 未登录或凭证无效 |
| `40300` | 403 | 已登录但无权执行（非群主加人、普通成员撤回他人消息） |
| `40400` | 404 | 会话/消息/用户不存在，**或当前用户不是该会话成员** |
| `40900` | 409 | 业务冲突（单聊加人、重复撤回、超出撤回窗口、成员已达上限） |
| `50000` | 500 | 服务器内部错误 |

**看不出成员身份是刻意的**：不是会话成员时，所有读取与会话操作的接口一律返回 `404` 而不是 `403`。`403` 会确认「这个会话确实存在，只是你不在里面」，等于提供了一个探测接口。只有调用方本来就能看见的资源才用 `403`，例如群成员能看见群、但没有管理成员的权限。

## 实时性：当前没有推送通道

本模块是**请求-响应**接口，不包含 WebSocket 或 SSE。客户端获取新消息的方式：

1. 轮询 `GET /conversations` 拿各会话的 `unread_count` 与 `message_seq`（一次请求覆盖全部会话）；
2. 对 `message_seq` 变大的会话，取最新一页消息（不带 `before_seq`），把其中 `seq` 大于本地已缓存最大值的那些插到本地末尾；
3. 打开会话时调用 `POST /conversations/{id}/read` 清空未读。

这套方式足以支撑中小规模使用，但轮询在会话多、在线人数多时会产生大量空请求。需要真正的实时投递时，应新增推送层（WebSocket 或长连接网关），并让消息写入后向该层发布事件——届时数据库仍是唯一权威来源，推送只是加速层。

## 命令行示例

```bash
# 0. 登录并保存令牌 Cookie（引导账号见 docs/reference/AUTH_API.md）
curl -c cookies.txt -X POST http://127.0.0.1:3000/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"your-admin","password":"your-bootstrap-password"}'

# 1. 创建单聊（对方 public_id 取自 /api/v1/users）
curl -b cookies.txt -X POST http://127.0.0.1:3000/api/v1/conversations \
  -H "Content-Type: application/json" \
  -d '{"kind":"single","member_public_ids":["01991f3a-7c40-7d62-8f6d-4fb743e0f731"]}'

# 2. 创建群聊
curl -b cookies.txt -X POST http://127.0.0.1:3000/api/v1/conversations \
  -H "Content-Type: application/json" \
  -d '{"kind":"group","title":"项目讨论组","member_public_ids":["01991f3a-7c40-7d62-8f6d-4fb743e0f731"]}'

# 3. 发消息（带幂等键，可以安全重试）
curl -b cookies.txt -X POST http://127.0.0.1:3000/api/v1/conversations/CONVERSATION_ID/messages \
  -H "Content-Type: application/json" \
  -d '{"content":"你好","client_message_id":"c-0001"}'

# 4. 会话列表（看未读数与最后一条消息）
curl -b cookies.txt "http://127.0.0.1:3000/api/v1/conversations?page=1&page_size=10"

# 5. 翻历史消息（游标用上一页返回的 next_before_seq）
curl -b cookies.txt "http://127.0.0.1:3000/api/v1/conversations/CONVERSATION_ID/messages?limit=20"

# 6. 标记已读
curl -b cookies.txt -X POST http://127.0.0.1:3000/api/v1/conversations/CONVERSATION_ID/read

# 7. 撤回消息
curl -b cookies.txt -X POST http://127.0.0.1:3000/api/v1/messages/MESSAGE_ID/recall

# 8. 加人 / 退群
curl -b cookies.txt -X POST http://127.0.0.1:3000/api/v1/conversations/CONVERSATION_ID/members \
  -H "Content-Type: application/json" \
  -d '{"member_public_ids":["01991f3a-7c40-7d62-8f6d-4fb743e0f732"]}'
curl -b cookies.txt -X DELETE http://127.0.0.1:3000/api/v1/conversations/CONVERSATION_ID/members/me
```

## 维护方式

接口发生变化时，在对应代码位置维护文档定义：

- Handler 的 `utoipa::path`：路径、参数、请求体、响应和状态码（`src/modules/information/handler.rs`）。
- Model 的 `ToSchema` 与 `Validate`：请求与响应 Schema、校验规则（`model.rs`）。
- `src/app/openapi.rs`：统一注册新增的接口、Schema 与 `聊天` 标签。
- 表结构变化通过**新增**迁移文件完成，不修改已执行的 `003_create_information_tables.sql`。

完成修改后运行：

```bash
cargo clippy --all-targets
cargo test
```

`cargo fmt --check` 在 stable 工具链上会因 `rustfmt.toml` 中的 nightly-only 选项报差异，原因见 [`../README.md`](../README.md) 第 4.5 节。
