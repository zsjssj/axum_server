# 认证模块 API 文档

> **文档定位**：本文是**参考手册**，用于查阅既定事实与操作步骤。
> 想按学习顺序理解设计意图，请从 [`../README.md`](../README.md) 开始读教程章节。

本模块提供登录、令牌刷新、登出和当前用户查询接口。接口定义以 Utoipa 注解为唯一来源，本文档说明使用方式、令牌语义和调用方必须遵守的约束。

## 文档入口

服务启动后可访问：

- Swagger UI：`http://127.0.0.1:3000/swagger-ui/`
- OpenAPI JSON：`http://127.0.0.1:3000/api-docs/openapi.json`

## 认证方式：HttpOnly Cookie

**令牌通过 `Set-Cookie` 响应头下发，响应体不包含令牌，也不存在 `Authorization` 头认证方式。**

登录成功后，服务端在响应头中下发两个 Cookie：

| Cookie | 内容 | 有效期 |
| --- | --- | --- |
| `access_token` | JWT（HS256） | 15 分钟（`jwt.access_ttl_minutes`） |
| `refresh_token` | 256 位随机值 | 7 天（`jwt.refresh_ttl_days`） |

两个 Cookie 均带有以下安全属性：

| 属性 | 值 | 作用 |
| --- | --- | --- |
| `HttpOnly` | 固定启用 | 页面脚本无法读取令牌，XSS 无法直接窃取凭据 |
| `Secure` | 由 `auth.cookie.secure` 控制 | 仅通过 HTTPS 传输 |
| `SameSite` | `Strict`（默认）或 `Lax` | 禁止跨站请求携带令牌，防 CSRF 的主要手段 |
| `Path` | 由 `auth.cookie.path` 控制，默认 `/` | 限制发送范围 |

刻意**不设置 `Domain`**：省略时浏览器按"仅当前主机"处理，避免令牌被发送到同一父域下的其他子域。

### 客户端如何配合

浏览器会自动携带 Cookie，因此前端**无需也不再需要**手动管理令牌，具体表现是：

- 请求时设置 `credentials: 'include'`（fetch）或 `withCredentials = true`（axios），跨源场景下必须开启；
- 不应尝试读取 `access_token`：`HttpOnly` 使其对 JavaScript 不可见，这是有意设计；
- 判断登录状态应调用 `GET /api/v1/auth/me`，而不是检查本地是否存在令牌。

### 为什么不再使用 `Authorization` 头

原先的 Bearer 方案要求前端把令牌存入 `localStorage` 或内存，前者会被任意 XSS 直接读取，后者在页面刷新后丢失。改为 HttpOnly Cookie 后，XSS 的影响被限制为"攻击者可以代为发请求"，无法长期窃取凭据。

相应地，服务端**只接受 Cookie**：`Authorization` 头不再被解析（已有测试锁定该行为）。保留单一认证来源可以避免出现绕过 Cookie 安全属性的旁路，也避免"某条路径漏校验 Cookie"的差异化处理。

## 接口列表

基础路径为 `/api/v1/auth`：

| 方法 | 路径 | 用途 | 需要令牌 |
| --- | --- | --- | --- |
| `POST` | `/api/v1/auth/login` | 用户登录，令牌通过响应头下发 | 否 |
| `POST` | `/api/v1/auth/refresh` | 用刷新令牌 Cookie 换取新令牌对 | 刷新 Cookie |
| `POST` | `/api/v1/auth/logout` | 吊销会话、吊销当前 access token 并清除令牌 Cookie | 否 |
| `GET` | `/api/v1/auth/me` | 获取当前登录用户资料 | access Cookie |

刷新与登出接口**没有请求体**：令牌直接取自请求 Cookie。请求体中的多余字段会被拒绝。

### `/api/v1` 下的接口默认需要登录

登录校验由认证中间件在 `/api/v1` 路由组上统一强制，**只有上表中登录、刷新、登出三个入口是匿名可访问的**，其余接口（包括用户接口与示例接口 `/api/v1/hello`）未登录时一律返回：

```json
{
  "code": 40100,
  "message": "未登录或登录已过期",
  "data": null
}
```

判断依据与失败原因：

- 请求没有 `access_token` Cookie → `401`、`未登录或登录已过期`。
- Cookie 存在但 JWT 过期 → `401`、`认证令牌已过期`，此时应调用 `/auth/refresh`。
- 签名、签发者或 `typ` 声明不合法 → `401`、`认证令牌无效` / `认证令牌类型错误`。
- 令牌已被登出吊销 → `401`、`认证令牌已失效，请重新登录`。

未注册的 `/api/v1` 路径不受该中间件影响，直接返回 `404`（`40400`）：认证中间件只作用于已匹配到路由的请求，不存在的接口不必先要求登录。

### 登出必须清除两个 Cookie

登出响应会同时下发两个 `Max-Age=0` 的清除 Cookie，且属性（`Path`、`HttpOnly`、`Secure`、`SameSite`）与下发时完全一致。浏览器按名称、域和路径匹配 Cookie，属性不一致会导致清除失败、令牌残留在客户端。

服务端同时吊销两类凭据：

- `refresh_token` 对应的会话立即失效，无法再续期；
- 当前 `access_token` 的 `jti` 被写入 Redis 吊销列表，认证中间件随后拒绝它——**登出后 access token 立即失效**。

登出是**幂等**操作：重复调用、Cookie 已缺失、甚至服务端吊销失败，都会返回成功并清除 Cookie。让客户端卡在"无法退出"的状态比一次吊销失败更糟。

## CSRF 防护说明

Cookie 会自动随请求发送，因此必须防 CSRF。本方案采用三层措施：

1. **`SameSite=Strict`（默认）**：浏览器在跨站请求中完全不发送令牌 Cookie，这是主要防线。若业务需要从外部站点跳转后保持登录态，可改为 `Lax`，但不应使用 `None`——配置为未知取值时服务端会回退到最严格的 `Strict`。
2. **请求体强制 `application/json`**：登录、刷新等接口只接受 JSON 请求体，浏览器对 `application/json` 的跨源请求会先发预检请求，而本服务的 CORS 配置不会放行未授权来源的预检。
3. **不提供 `GET` 状态的写操作**：所有改变认证状态的操作都是 `POST`。

需要注意的残余风险：若将 `same_site` 放宽为 `Lax`，则从外部站点发起的顶层 `GET` 导航会携带 Cookie。当前所有写操作均为 `POST`，因此该风险不影响认证状态。**生产环境仍应配合 CORS 白名单收紧来源。**

### 跨源部署必须同时改 CORS

`/api/v1` 下的接口默认需要登录后，跨源部署的失败模式会集中暴露在 CORS 配置上：浏览器在凭据模式下**不接受通配来源**，服务端也不能同时使用 `Any` 与 `allow_credentials(true)`。

当前的 `cors_layer()` 放行任意来源且未开启凭据支持，只适用于同源部署或非浏览器调用（如 curl）。若前端与后端不同源，必须把它改成：

- `allow_origin` 指定**具体的前端来源**，不能用 `Any`；
- 启用 `allow_credentials(true)`，否则浏览器不会携带令牌 Cookie。

否则现象是「登录接口本身返回成功，但后续接口全部 401」——因为浏览器根本没有把 `Set-Cookie` 下发的令牌带回来。前端侧还需要设置 `credentials: 'include'`（fetch）或 `withCredentials = true`（axios）。

## 双令牌设计

| 令牌 | 格式 | 服务端状态 | 用途 |
| --- | --- | --- | --- |
| access token | JWT（HS256） | 无状态；登出后按 `jti` 写入 Redis 吊销列表 | 访问受保护接口 |
| refresh token | 256 位随机值 | 只存 SHA-256 摘要 | 换取新的令牌对 |

### 登出后的失效范围

**登出会立即使 `access_token` 与 `refresh_token` 都不可用。**

- `refresh_token` 对应的会话在数据库中被标记为已吊销，无法再续期；
- `access_token` 的 `jti` 被写入 Redis 吊销列表（TTL 等于令牌剩余有效期），认证中间件每次校验令牌时会用 `EXISTS` 查询该列表，命中即返回：

```json
{
  "code": 40100,
  "message": "认证令牌已失效，请重新登录",
  "data": null
}
```

这里有一个需要调用方理解的边界：吊销检查依赖 Redis。若 Redis 不可用，服务端会记录告警并按「未吊销」放行，此时行为退化为改动前——已签发的 access token 在自然过期前（默认 15 分钟）仍然有效。Redis 是「立即生效」的增强，不是认证正确性的前提。

## 令牌轮换与重放检测

`/refresh` 采用**轮换**策略：每次刷新都签发全新的 `refresh_token`，同时把旧的标记为已吊销，并在 `replaced_by` 中记录轮换关系。

如果**已吊销的 `refresh_token` 被再次使用**，服务端判定为令牌泄露重放，会立即吊销该用户的**全部**会话，强制攻击者与合法用户同时重新登录。这是有意的取舍：此时无法分辨谁是攻击者，宁可让用户重新登录，也不允许泄露的令牌继续可用。

客户端因此必须遵守：**不要并发发起多个刷新请求。** 并发刷新会让其中一个请求使用已轮换的旧令牌，触发全量下线。刷新成功后新 Cookie 会由响应头自动覆盖，前端无需处理令牌本身。

## 请求参数校验

- 登录：`username` 长度为 3–50；`password` 只要求非空。
- JSON 请求体不接受未声明字段。

登录接口刻意**不校验密码长度**：登录的职责是核验凭证，不是约束密码格式。若在登录时校验长度，密码策略调整会把存量用户挡在门外，且不同的提示会泄露服务端密码策略。

## 登录保护

登录失败按**用户名 + 滑动时间窗口**统计，窗口默认 15 分钟，失败达到 5 次后暂时拒绝该用户名的登录尝试，返回 `429`。

设计说明：

- 计数存放在 Redis 的有序集合中（键为 `{key_prefix}:auth:login_failures:{username}`，成员是失败的唯一标识、分数是失败时间），读取时先删除窗口外的成员再统计，因此是真正的滚动窗口，而不是「窗口边界会整体清零」的固定窗口。计数键带 TTL，窗口内没有失败就会自动消失。
- 数据库的 `login_failures` 表继续保存审计记录，**不在 `users` 表上做账号锁定**。后者会让自动锁定变成针对特定账号的拒绝服务手段——攻击者只需故意输错密码即可锁死他人账号。
- Redis 不可用时，判定自动回退为统计 `login_failures` 表，限流不会静默失效；此时延迟会略有上升，日志中会出现「回退到数据库统计」的告警。
- 判定顺序是**先检查限流、再校验密码**。Argon2 校验被刻意设计得很慢，若顺序颠倒，大量失败请求会持续占用阻塞线程池，使限流本身成为消耗服务端算力的攻击面。
- 成功登录会清理窗口外的历史审计记录，但**不清空窗口内的失败计数**：清空等于给攻击者一个「只要偶尔成功一次就能重置限额」的入口。
- 用户名不存在与密码错误返回**完全一致**的响应，且用户名不存在时同样执行一次 Argon2 计算，避免通过响应耗时区分账号是否存在，形成用户名枚举通道。

命中限流时的响应：

```json
{
  "code": 42900,
  "message": "登录失败次数过多，请稍后再试",
  "data": null
}
```

即使此时密码正确也会被拒绝，这是限流的设计语义；窗口过去后自动恢复。

## 统一响应格式

沿用全项目的 `ApiResponse<T>` 信封，成功业务码固定为 `0`。登录成功响应体只包含与客户端调度有关的信息：

```json
{
  "code": 0,
  "message": "登录成功",
  "data": {
    "token_type": "cookie",
    "expires_in": 900,
    "cookies": {
      "access_token": "access_token",
      "refresh_token": "refresh_token"
    }
  }
}
```

同时响应头中带有：

```http
Set-Cookie: access_token=eyJ0eXAiOiJKV1Qi...; HttpOnly; SameSite=Strict; Path=/; Max-Age=900
Set-Cookie: refresh_token=3f1a9c2e5b7d...; HttpOnly; SameSite=Strict; Path=/; Max-Age=604800
```

认证相关错误码：

| 业务码 | HTTP | 含义 |
| --- | ---: | --- |
| `40100` | 401 | 未认证或凭证无效 |
| `42900` | 429 | 触发登录保护 |

登录失败的响应：

```json
{
  "code": 40100,
  "message": "用户名或密码错误",
  "data": null
}
```

## 首次启动：第一个账号从哪里来

`/api/v1` 下的接口默认要求登录，而创建用户的 `POST /api/v1/users` 自身也在该路由组内，且没有匿名注册入口。因此**空数据库时不存在任何能通过登录的账号**，全部业务接口只会返回 `401`。服务为这个死锁提供了一条受配置约束的出口：启动期引导账号（`src/app/bootstrap.rs`）。

生效条件（两个都满足才执行）：

1. `bootstrap.enabled = true`；
2. `users` 表为空。

已有用户的库永远不会被写入，因此开关保持开启也不会成为反复建号的通道。执行成功时日志会打印一条 `warn`，提示尽快修改口令并关闭开关。引导账号的字段规则与 `POST /api/v1/users` 完全一致（用户名 3–50、合法邮箱、密码 8–72），密码同样以 Argon2 哈希入库。

推荐做法是把凭据写进未被版本控制的 `config/local.toml`，或使用环境变量：

```toml
# config/local.toml（已被 .gitignore 排除）
[bootstrap]
enabled = true
username = "your-admin"
email = "admin@example.com"
password = "在此填入强口令"
```

```bash
# 或使用环境变量
APP__BOOTSTRAP__ENABLED=true \
APP__BOOTSTRAP__USERNAME=your-admin \
APP__BOOTSTRAP__EMAIL=admin@example.com \
APP__BOOTSTRAP__PASSWORD='在此填入强口令' \
cargo run
```

账号创建成功后即可把 `enabled` 改回 `false`。修改 `config/local.toml` 需要重启服务才会生效，配置只在启动时读取一次。

## 命令行示例

浏览器会自动管理 Cookie，命令行调试需要用 `-c` 写入、`-b` 读取 Cookie 文件。由于 `/api/v1` 下的接口默认需要登录，**先登录再调用其他接口**是必须的：

```bash
# 1. 登录并把令牌 Cookie 保存到文件
curl -c cookies.txt -X POST http://127.0.0.1:3000/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"testuser","password":"password123"}'

# 2. 携带 Cookie 访问受保护接口（不带 Cookie 时会得到 401）
curl -b cookies.txt http://127.0.0.1:3000/api/v1/auth/me
curl -b cookies.txt "http://127.0.0.1:3000/api/v1/users?page=1&page_size=10"

# 3. 刷新令牌（Cookie 文件会在响应后更新，请加上 -c 覆盖保存）
curl -b cookies.txt -c cookies.txt -X POST http://127.0.0.1:3000/api/v1/auth/refresh

# 4. 登出
curl -b cookies.txt -X POST http://127.0.0.1:3000/api/v1/auth/logout
```

若 `auth.cookie.path` 被改为 `/api/v1` 等子路径，Cookie 不会发送到该路径之外，`cookies.txt` 中的记录也会相应带路径属性，命令行调试时需注意路径匹配。

## 配置项

| 配置键 | 环境变量 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `jwt.secret` | `APP__JWT__SECRET` | 开发占位值 | HS256 签名密钥 |
| `jwt.issuer` | `APP__JWT__ISSUER` | `rust-axum-server` | 令牌签发者 |
| `jwt.access_ttl_minutes` | `APP__JWT__ACCESS_TTL_MINUTES` | `15` | access token 有效期（分钟） |
| `jwt.refresh_ttl_days` | `APP__JWT__REFRESH_TTL_DAYS` | `7` | refresh token 有效期（天） |
| `auth.max_login_failures` | `APP__AUTH__MAX_LOGIN_FAILURES` | `5` | 窗口内允许的失败次数 |
| `auth.login_window_minutes` | `APP__AUTH__LOGIN_WINDOW_MINUTES` | `15` | 失败统计窗口（分钟） |
| `auth.cookie.access_name` | `APP__AUTH__COOKIE__ACCESS_NAME` | `access_token` | access token Cookie 名 |
| `auth.cookie.refresh_name` | `APP__AUTH__COOKIE__REFRESH_NAME` | `refresh_token` | refresh token Cookie 名 |
| `auth.cookie.secure` | `APP__AUTH__COOKIE__SECURE` | `false` | 是否仅 HTTPS 传输 |
| `auth.cookie.same_site` | `APP__AUTH__COOKIE__SAME_SITE` | `strict` | `strict` 或 `lax` |
| `auth.cookie.path` | `APP__AUTH__COOKIE__PATH` | `/` | Cookie 作用路径 |
| `redis.url` | `APP__REDIS__URL` | `redis://127.0.0.1:6379` | 登录失败计数与令牌吊销使用的 Redis 地址 |
| `redis.pool_size` | `APP__REDIS__POOL_SIZE` | `8` | Redis 连接池上限 |
| `redis.key_prefix` | `APP__REDIS__KEY_PREFIX` | `axum` | 键命名空间，多环境必须不同 |
| `redis.user_cache_ttl_seconds` | `APP__REDIS__USER_CACHE_TTL_SECONDS` | `60` | 用户详情缓存 TTL（认证模块不直接使用） |
| `bootstrap.enabled` | `APP__BOOTSTRAP__ENABLED` | `false` | 是否在 `users` 表为空时创建引导账号 |
| `bootstrap.username` | `APP__BOOTSTRAP__USERNAME` | 空 | 引导账号用户名（3–50 字符） |
| `bootstrap.email` | `APP__BOOTSTRAP__EMAIL` | 空 | 引导账号邮箱 |
| `bootstrap.password` | `APP__BOOTSTRAP__PASSWORD` | 空 | 引导账号口令（8–72 字符，仅启动期使用） |
| `bootstrap.nickname` | `APP__BOOTSTRAP__NICKNAME` | 空 | 引导账号昵称，可选 |

### 生产环境必须做的三件事

1. **替换 `APP__JWT__SECRET`**。配置文件中的值是公开的占位值，使用它等于任何人都能伪造任意用户的令牌。启动时会拒绝长度不足 32 个字符的密钥，但长度校验无法识别「已泄露的长密钥」。可用 `openssl rand -base64 48` 生成。
2. **设置 `APP__AUTH__COOKIE__SECURE=true`**。默认值 `false` 仅为本地 HTTP 调试，若不改则令牌会在明文 HTTP 中传输。
3. **多实例部署时保证 `jwt.secret` 一致**，否则令牌会在实例间随机失效。

此外，**生产环境应保证 Redis 可用，并为每个环境设置不同的 `redis.key_prefix`**：不启动 Redis 时服务仍能运行，但限流会回退数据库、登出吊销与缓存失效，`/ready` 会返回 `503`；不同环境共用同一个前缀则会互相覆盖登录失败计数与吊销记录。

另外，**引导账号创建完成后应关闭 `bootstrap.enabled` 并从部署环境移除其凭据**。该账号拥有与其他用户完全相同的权限，且口令是人工设定的，长期留存会让它成为比正常账号更容易被猜中的入口。

## 安全设计要点

- **令牌通过 `HttpOnly` Cookie 下发**，响应体不包含令牌，且类型上 `TokenPair` 不派生 `Serialize`，从代码层面杜绝令牌被误放进 JSON。
- **Cookie 属性不可配置化削减**：`HttpOnly` 恒为启用，`SameSite` 非法取值回退到最严格的 `Strict`，不存在"配置写错导致防护静默放宽"的路径。
- **密码只以 Argon2 哈希存储**，认证查询通过独立的 `UserCredentials` 类型返回，该类型不派生 `Serialize`，因此 `password_hash` 没有进入任何响应或 OpenAPI Schema 的路径。
- **JWT 校验显式指定 HS256**，绝不从令牌头部读取算法。信任头部 `alg` 会让攻击者改用 `alg=none` 或把非对称公钥当作 HMAC 密钥，从而伪造任意令牌。
- **令牌携带 `typ` 字段并在校验时断言为 `access`**。缺少该检查时，长效 `refresh_token` 可被直接当作 access token 使用，绕过短期有效性设计。
- **刷新令牌只存 SHA-256 摘要**。数据库泄露时摘要无法反推出可用的原始令牌。
- **两个 Cookie 的 `Max-Age` 分别匹配各自令牌的有效期**，避免 access token 过期后仍被浏览器持续发送。
- **JWT 错误信息区分「过期」与「无效」**，便于客户端决定是刷新还是重新登录，但两者都不泄露签名或密钥细节。
- **登出立即吊销 access token**：只把 `jti` 写入 Redis，TTL 等于令牌剩余有效期，不保存令牌内容；Redis 泄露也无法据此伪造令牌，令牌自然过期后记录自动消失。
- **限流状态放在 Redis，审计留在数据库**：既获得自动过期与多实例共享，又保留可追溯的失败记录；Redis 故障时判定自动回退数据库，不会出现「限流静默失效」。

## 维护方式

接口发生变化时，在对应代码位置维护定义：

- `modules/auth/handler.rs`：路由、Cookie 下发与 `utoipa::path` 注解。
- `modules/auth/model.rs`：请求与响应 Schema。
- `common/cookie.rs`：Cookie 的构建、解析与安全属性。
- `common/auth.rs`：登录校验中间件、令牌吊销检查与匿名白名单。调整 `/api/v1` 下的路由或认证入口时必须同步检查这里。
- `modules/auth/service.rs`：限流窗口、失败记录与 access token 吊销逻辑。
- `infrastructure/redis.rs`：Redis 键命名、限流脚本与吊销读写。
- `app/routes.rs`：`/api/v1` 路由组与认证中间件的挂载。
- `app/openapi.rs`：统一注册接口与 Schema。

Redis 的数据结构、命令与排错方式见 [Redis 学习与项目实战指南](./REDIS_GUIDE.md)。

完成修改后运行：

```bash
cargo fmt --check
cargo clippy --all-targets
cargo test
```

数据库结构变更必须新增 `migrations/` 下的迁移文件，不修改已执行的迁移。
