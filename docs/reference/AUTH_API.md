# 认证模块 API 文档

> **文档定位**：本文是**参考手册**，用于查阅既定事实与操作步骤。
> 想按学习顺序理解设计意图，请从 [`../README.md`](../README.md) 开始读教程章节。

本模块提供登录、登出和当前用户查询接口。本项目**只签发一个令牌**，不提供刷新接口：令牌到期后必须重新登录。接口定义以 Utoipa 注解为唯一来源，本文档说明使用方式、令牌语义和调用方必须遵守的约束。

## 文档入口

服务启动后可访问：

- Swagger UI：`http://127.0.0.1:3000/swagger-ui/`
- OpenAPI JSON：`http://127.0.0.1:3000/api-docs/openapi.json`

## 认证方式：单个 HttpOnly Cookie

**令牌通过 `Set-Cookie` 响应头下发，响应体不包含令牌，也不存在 `Authorization` 头认证方式。**

登录成功后，服务端在响应头中只下发一个 Cookie：

| Cookie | 内容 | 有效期 |
| --- | --- | --- |
| `access_token`（名称由 `auth.cookie.name` 配置） | JWT（HS256） | `jwt.ttl_minutes`，默认 10080 分钟（7 天） |

该 Cookie 带有以下安全属性：

| 属性 | 值 | 作用 |
| --- | --- | --- |
| `HttpOnly` | 固定启用 | 页面脚本无法读取令牌，XSS 无法直接窃取凭据 |
| `Secure` | 由 `auth.cookie.secure` 控制 | 仅通过 HTTPS 传输 |
| `SameSite` | `Strict`（默认）或 `Lax` | 禁止跨站请求携带令牌，防 CSRF 的主要手段 |
| `Path` | 由 `auth.cookie.path` 控制，默认 `/` | 限制发送范围 |
| `Max-Age` | 等于令牌剩余有效期 | Cookie 与令牌同时失效，不存在「Cookie 还在、令牌已过期」的游离状态 |

刻意**不设置 `Domain`**：省略时浏览器按"仅当前主机"处理，避免令牌被发送到同一父域下的其他子域。

### 客户端如何配合

浏览器会自动携带 Cookie，因此前端**无需也不再需要**手动管理令牌，具体表现是：

- 请求时设置 `credentials: 'include'`（fetch）或 `withCredentials = true`（axios），跨源场景下必须开启；
- 不应尝试读取 `access_token`：`HttpOnly` 使其对 JavaScript 不可见，这是有意设计；
- 判断登录状态应调用 `GET /api/v1/auth/me`，而不是检查本地是否存在令牌；
- **不做自动续期**：登录响应体里的 `expires_in` 是唯一的到期提示，收到 `401` 或 `expires_in` 用尽后，前端应清理本地登录态并跳转登录页。项目不提供 `/auth/refresh`，也不存在能换取新令牌的隐藏入口。

### 为什么不再使用 `Authorization` 头

原先的 Bearer 方案要求前端把令牌存入 `localStorage` 或内存，前者会被任意 XSS 直接读取，后者在页面刷新后丢失。改为 HttpOnly Cookie 后，XSS 的影响被限制为"攻击者可以代为发请求"，无法长期窃取凭据。

相应地，服务端**只接受 Cookie**：`Authorization` 头不再被解析（已有测试锁定该行为）。保留单一认证来源可以避免出现绕过 Cookie 安全属性的旁路，也避免"某条路径漏校验 Cookie"的差异化处理。

## 接口列表

基础路径为 `/api/v1/auth`：

| 方法 | 路径 | 用途 | 需要令牌 |
| --- | --- | --- | --- |
| `POST` | `/api/v1/auth/login` | 用户登录，令牌通过响应头下发 | 否 |
| `POST` | `/api/v1/auth/logout` | 标记会话审计记录、吊销当前令牌并清除令牌 Cookie | 否 |
| `GET` | `/api/v1/auth/me` | 获取当前登录用户资料 | 令牌 Cookie |

登出接口**没有请求体**：令牌直接取自请求 Cookie。请求体中的多余字段会被拒绝。

`/api/v1/auth/refresh` **已下线**：路由不再注册，请求会落到统一 `404`（`40400`），且它不在匿名白名单中（白名单只有登录与登出）。

### `/api/v1` 下的接口默认需要登录

登录校验由认证中间件在 `/api/v1` 路由组上统一强制，**只有登录与登出两个入口是匿名可访问的**，其余接口（包括用户接口与示例接口 `/api/v1/hello`）未登录时一律返回：

```json
{
  "code": 40100,
  "message": "未登录或登录已过期",
  "data": null
}
```

判断依据与失败原因：

- 请求没有令牌 Cookie → `401`、`未登录或登录已过期`。
- Cookie 存在但 JWT 过期 → `401`、`认证令牌已过期，请重新登录`。**这就是登录态的终点**：没有刷新接口，客户端唯一的正确动作是引导用户重新登录。
- 签名、签发者或 `typ` 声明不合法 → `401`、`认证令牌无效` / `认证令牌类型错误`。
- 令牌已被登出吊销 → `401`、`认证令牌已失效，请重新登录`。

未注册的 `/api/v1` 路径不受该中间件影响，直接返回 `404`（`40400`）：认证中间件只作用于已匹配到路由的请求，不存在的接口不必先要求登录。

### 登出会清除 Cookie 并让令牌立即失效

登出响应会下发一个 `Max-Age=0` 的清除 Cookie，属性（`Path`、`HttpOnly`、`Secure`、`SameSite`）与下发时完全一致。浏览器按名称、域和路径匹配 Cookie，属性不一致会导致清除失败、令牌残留在客户端。

服务端同时做两件事：

- 数据库里这条登录会话的审计记录被标记为已吊销（`revoked_at`），用于回答「这次会话何时结束」；
- 当前令牌的 `jti` 被写入 Redis 吊销列表，认证中间件随后拒绝它——**登出后令牌立即失效**，不必等它自然过期。

登出是**幂等**操作：重复调用、Cookie 已缺失、甚至服务端吊销失败，都会返回成功并清除 Cookie。让客户端卡在"无法退出"的状态比一次吊销失败更糟。

## CSRF 防护说明

Cookie 会自动随请求发送，因此必须防 CSRF。本方案采用三层措施：

1. **`SameSite=Strict`（默认）**：浏览器在跨站请求中完全不发送令牌 Cookie，这是主要防线。若业务需要从外部站点跳转后保持登录态，可改为 `Lax`，但不应使用 `None`——配置为未知取值时服务端会回退到最严格的 `Strict`。
2. **请求体强制 `application/json`**：登录等接口只接受 JSON 请求体，浏览器对 `application/json` 的跨源请求会先发预检请求，而本服务的 CORS 配置不会放行未授权来源的预检。
3. **不提供 `GET` 状态的写操作**：所有改变认证状态的操作都是 `POST`。

需要注意的残余风险：若将 `same_site` 放宽为 `Lax`，则从外部站点发起的顶层 `GET` 导航会携带 Cookie。当前所有写操作均为 `POST`，因此该风险不影响认证状态。**生产环境仍应配合 CORS 白名单收紧来源。**

### 跨源部署必须同时改 CORS

`/api/v1` 下的接口默认需要登录后，跨源部署的失败模式会集中暴露在 CORS 配置上：浏览器在凭据模式下**不接受通配来源**，服务端也不能同时使用 `Any` 与 `allow_credentials(true)`。

当前的 `cors_layer()` 放行任意来源且未开启凭据支持，只适用于同源部署或非浏览器调用（如 curl）。若前端与后端不同源，必须把它改成：

- `allow_origin` 指定**具体的前端来源**，不能用 `Any`；
- 启用 `allow_credentials(true)`，否则浏览器不会携带令牌 Cookie。

否则现象是「登录接口本身返回成功，但后续接口全部 401」——因为浏览器根本没有把 `Set-Cookie` 下发的令牌带回来。前端侧还需要设置 `credentials: 'include'`（fetch）或 `withCredentials = true`（axios）。

## 单令牌设计

| 项目 | 取值 |
| --- | --- |
| 令牌格式 | JWT（HS256），负载含 `sub`/`iss`/`exp`/`iat`/`jti`/`typ` |
| 有效期 | `jwt.ttl_minutes`，默认 7 天 |
| 服务端状态 | 无状态；登出后按 `jti` 写入 Redis 吊销列表 |
| 过期后 | 重新登录（无刷新接口） |

**为什么不再用「短期 access + 长期 refresh」？** 双令牌的价值在于「短期令牌承担每次请求校验，长期令牌负责续期，且续期凭据有服务端状态、可随时吊销」。代价是一整套配套机制：两个 Cookie、两个有效期、一个匿名刷新接口、轮换与重放检测、以及只在刷新路径上才走得到的会话状态机。本项目的场景不需要这么长的免登录时间，也不希望多一个匿名入口，因此选择**一个有效期较长的令牌 + 到期重新登录**：

- 认证路径只有一条（校验签名 → 查吊销列表），没有第二套续期分支需要同步维护；
- 匿名白名单从三项减到两项，少一个可被滥用的入口；
- 令牌泄露后的最长可用时间等于 `ttl_minutes`，这是可以明确写进配置、按部署环境收紧的单一数字。需要更严格时把它调小，而不是增加刷新逻辑。

### 登录会话表只承担审计

`user_sessions` 表在单令牌方案下**不参与每次请求的认证**：中间件不会为了认证去查询它。登录时写入一条记录（`token_hash` 是令牌的 SHA-256 摘要，不是令牌本身），登出时标记 `revoked_at`，用于回答「谁在什么时候、从哪个 IP 和客户端登录过、何时结束」。表的注释在新的迁移 `migrations/004_document_single_token_sessions.sql` 中补充说明。

这样划分的后果需要明确：**认证的正确性只取决于 JWT 签名与 Redis 吊销列表**，即使会话表写入失败，登录也不会被放行成半成品（写入失败会整体返回错误）；反过来说，手动改库把某条记录标为已吊销，并不会让对应令牌立刻下线——下线必须走 Redis 吊销。

### 登出后的失效范围

**登出会让当前令牌立即不可用。**

令牌的 `jti` 被写入 Redis 吊销列表（TTL 等于令牌剩余有效期），认证中间件每次校验令牌时会用 `EXISTS` 查询该列表，命中即返回：

```json
{
  "code": 40100,
  "message": "认证令牌已失效，请重新登录",
  "data": null
}
```

这里有一个需要调用方理解的边界：吊销检查依赖 Redis。若 Redis 不可用，服务端会记录告警并按「未吊销」放行，此时行为退化为「令牌到期才失效」——由于单令牌默认有效期是 7 天，这个退化窗口比双令牌方案（15 分钟）大得多。**生产环境因此应把 Redis 视为认证链路的一部分**，`/ready` 也会在 Redis 不可用时返回 `503`。

吊销记录只保存 `jti` 与 TTL，不保存令牌内容：Redis 泄露也无法据此伪造令牌，令牌自然过期后记录自动消失。

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
    "expires_in": 604800
  }
}
```

`expires_in` 是令牌剩余秒数（默认 7 天 = 604800 秒），客户端据此决定何时提示重新登录；它是提示性的，真正的权威判定始终是服务端的 `401`。

同时响应头中带有：

```http
Set-Cookie: access_token=eyJ0eXAiOiJKV1Qi...; HttpOnly; SameSite=Strict; Path=/; Max-Age=604800
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

# 3. 登出（令牌立即失效，Cookie 被清除）
curl -b cookies.txt -c cookies.txt -X POST http://127.0.0.1:3000/api/v1/auth/logout
```

令牌到期后不存在续期命令：重新执行第 1 步登录即可。

若 `auth.cookie.path` 被改为 `/api/v1` 等子路径，Cookie 不会发送到该路径之外，`cookies.txt` 中的记录也会相应带路径属性，命令行调试时需注意路径匹配。

## 配置项

| 配置键 | 环境变量 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `jwt.secret` | `APP__JWT__SECRET` | 开发占位值 | HS256 签名密钥 |
| `jwt.issuer` | `APP__JWT__ISSUER` | `rust-axum-server` | 令牌签发者 |
| `jwt.ttl_minutes` | `APP__JWT__TTL_MINUTES` | `10080` | 登录令牌有效期（分钟，默认 7 天，上限一年），到期后需重新登录 |
| `auth.max_login_failures` | `APP__AUTH__MAX_LOGIN_FAILURES` | `5` | 窗口内允许的失败次数 |
| `auth.login_window_minutes` | `APP__AUTH__LOGIN_WINDOW_MINUTES` | `15` | 失败统计窗口（分钟） |
| `auth.cookie.name` | `APP__AUTH__COOKIE__NAME` | `access_token` | 令牌 Cookie 名 |
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

单令牌方案下还应重新评估 `APP__JWT__TTL_MINUTES`：默认 7 天意味着令牌泄露后最长可用 7 天，面向高敏感业务时应显著调小，并接受更频繁的重新登录。

另外，**引导账号创建完成后应关闭 `bootstrap.enabled` 并从部署环境移除其凭据**。该账号拥有与其他用户完全相同的权限，且口令是人工设定的，长期留存会让它成为比正常账号更容易被猜中的入口。

## 安全设计要点

- **令牌通过 `HttpOnly` Cookie 下发**，响应体不包含令牌，且类型上 `IssuedToken` 不派生 `Serialize`，从代码层面杜绝令牌被误放进 JSON。
- **Cookie 属性不可配置化削减**：`HttpOnly` 恒为启用，`SameSite` 非法取值回退到最严格的 `Strict`，不存在"配置写错导致防护静默放宽"的路径。
- **Cookie 名只有一处定义**：下发与清除都直接读 `auth.cookie.name`，不接受调用方另传名字，避免出现「名字不一致导致清除失败、令牌残留」。
- **密码只以 Argon2 哈希存储**，认证查询通过独立的 `UserCredentials` 类型返回，该类型不派生 `Serialize`，因此 `password_hash` 没有进入任何响应或 OpenAPI Schema 的路径。
- **JWT 校验显式指定 HS256**，绝不从令牌头部读取算法。信任头部 `alg` 会让攻击者改用 `alg=none` 或把非对称公钥当作 HMAC 密钥，从而伪造任意令牌。
- **令牌携带 `typ` 字段并在校验时断言为 `access`**：目前只有一种令牌，但把用途写进契约后，将来引入其他用途令牌时不会因为漏判而被当作登录凭据。
- **会话表只存令牌摘要**（SHA-256）。数据库泄露时摘要无法反推出可用的原始令牌，且该表不参与认证，泄露也不会直接扩大攻击面。
- **Cookie 的 `Max-Age` 与令牌有效期一致**，避免令牌失效后浏览器仍持续发送它。
- **JWT 错误信息区分「过期」与「无效」**：过期提示直接指向重新登录，不暗示存在刷新接口，也不泄露签名或密钥细节。
- **登出立即吊销令牌**：只把 `jti` 写入 Redis，TTL 等于令牌剩余有效期，不保存令牌内容；Redis 泄露也无法据此伪造令牌，令牌自然过期后记录自动消失。
- **限流状态放在 Redis，审计留在数据库**：既获得自动过期与多实例共享，又保留可追溯的失败记录；Redis 故障时判定自动回退数据库，不会出现「限流静默失效」。

## 维护方式

接口发生变化时，在对应代码位置维护定义：

- `modules/auth/handler.rs`：路由、Cookie 下发与 `utoipa::path` 注解。
- `modules/auth/model.rs`：请求与响应 Schema（`IssuedToken`、`AuthSessionResponse`）。
- `common/cookie.rs`：Cookie 的构建、解析与安全属性。
- `common/auth.rs`：登录校验中间件、令牌吊销检查与匿名白名单。调整 `/api/v1` 下的路由或认证入口时必须同步检查这里。
- `modules/auth/service.rs`：限流窗口、失败记录、令牌签发与吊销逻辑。
- `modules/auth/repository.rs`：登录会话审计记录的写入与吊销。
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
