# 10 Redis 与降级设计

**本章目标**：搞懂 Redis 在这个项目里承担什么、为什么这么用，以及最重要的——**每个功能在 Redis 挂掉时会怎样**。本章的核心不是 Redis 命令，而是"依赖分级"这个架构能力。

> 想系统学 Redis（五种数据类型、内存淘汰、Lua 细节、五个动手实验、排错手册），读 [reference/REDIS_GUIDE.md](./reference/REDIS_GUIDE.md)。本章只讲**这个项目为什么这么设计**。

---

## 10.1 定位：Redis 是加速层，不是权威数据

```rust
// src/config/mod.rs:31
/// Redis 在本项目中承担三类「读写频繁、可以重建」的状态：
/// 登录失败计数、access token 吊销列表、用户资料缓存。
/// 它们都有一个共同点：丢失后可以接受
```

这个"共同点"是整章的地基。判断"某个数据能不能放 Redis"，问一句：**丢了会怎样？**

| 数据 | 存在 Redis | 丢了会怎样 | 能否接受 |
| --- | --- | --- | --- |
| 登录失败计数 | ZSET + TTL | 短时限流失效（还有数据库兜底） | ✅ 可接受 |
| access token 吊销列表 | STRING + TTL | 已登出的令牌活到自然过期（≤15 分钟） | ✅ 可接受 |
| 用户资料缓存 | STRING(JSON) + TTL | 多查一次数据库 | ✅ 可接受 |
| 用户密码哈希 | ❌ 绝不 | 无法重建 | ❌ 只能在 PostgreSQL |
| refresh token 会话 | ❌ 数据库 | 无法重建、无法吊销 | ❌ 只能在 PostgreSQL |

由此得到本项目的**依赖分级**：

```text
PostgreSQL  = 权威依赖：不可用 → 启动失败 / 就绪检查失败
Redis       = 加速依赖：不可用 → 各功能按明确策略降级，就绪检查失败
```

两条都让 `/ready` 返回 503（见[第 11 章](./11-可观测性与健康检查.md)），因为"降级运行"的实例不该继续接流量；但**启动**阶段的处理不同：数据库连不上直接退出，Redis 连不上只告警。

## 10.2 适配层：把 Redis 关进一个文件

```rust
// src/infrastructure/redis.rs:117
#[derive(Clone)]
pub struct RedisStore {
    pool: RedisPool,           // bb8 连接池
    key_prefix: String,        // 已规范化的命名空间
    user_cache_ttl: Duration,  // 缓存 TTL
}
```

模块级文档（`src/infrastructure/redis.rs:1-20`）写了三条"为什么集中在一个文件"：连接获取/超时/命令拼装只实现一次；所有键统一加前缀；**只如实返回错误，不决定降级策略**。

最后一条最重要。看一个反例：

```rust
// ❌ 适配层自己吞错（千万不要这么写）
pub async fn get_json<T>(&self, key: &str) -> Option<T> {
    // Redis 挂了就返回 None，调用方永远不知道缓存其实是坏的
}
```

这样调用方无法区分"缓存未命中"和"Redis 故障"，而这两者的正确应对完全不同：未命中该查库回填，故障该告警并按降级策略处理。**错误必须如实上报，降级决策留给知道业务语义的那一层。**

### 错误分类：为了排障，不为好看

```rust
// src/infrastructure/redis.rs:50
#[derive(Debug, Error)]
pub enum RedisStoreError {
    #[error("Redis 配置不合法：{0}")]
    Config(String),                     // 地址无法解析
    #[error("Redis 连接不可用：{0}")]
    Connection(String),                 // 取连接失败：Redis 不可达/超时/被拒绝
    #[error("Redis 命令执行失败：{0}")]
    Command(#[from] RedisError),        // 命令执行失败：键类型不匹配、脚本报错
    #[error("缓存值序列化失败：{0}")]
    Serde(#[from] serde_json::Error),   // 序列化失败：程序缺陷，不是 Redis 故障
}
```

文案、`source` 与 `From` 转换都由 `thiserror` 在变体上声明。这一点在「错误分类」这种类型上尤其重要：以前手写 `impl Error` 的 `source()` 时，只有 `Command` 和 `Serde` 有底层原因，`Config` 与 `Connection` 没有——这个差异藏在另一个 `match` 里，新增变体时很容易忘记维护。现在 `#[from]` 直接写在变体上，读枚举就知道来源链，`From<RedisError>` / `From<serde_json::Error>` 也顺带生成，调用处的 `.map_err(RedisStoreError::Command)` 写法不变。

分这么细的价值在排障：

| 错误 | 该做什么 |
| --- | --- |
| `Connection` | Redis 挂了 → 降级、查运维 |
| `Command` | 命令或键类型有问题 → **改代码**，重试没用 |
| `Serde` | 缓存里的 JSON 和当前结构体不匹配 → 清缓存或改结构 |
| `Config` | 配置写错了 → 改配置，且应该在启动期就拦住 |

如果全部压成一个 `String`，运维半夜看到"Redis error"根本不知道该重启 Redis 还是叫开发。

### 键命名空间：多环境共用一个实例时的唯一隔离手段

```rust
// src/infrastructure/redis.rs:300
fn normalize_prefix(prefix: &str) -> String {
    prefix.trim().trim_matches(':').to_owned()
}
```

允许写成 `axum:` 或 ` axum `，但最终拼接只出现一个冒号（否则会出现 `axum::user:detail:...` 这种难以辨认的键）。

三个键构造器：

```rust
// src/infrastructure/redis.rs:267
pub fn login_failure_key(&self, username: &str) -> String  { self.namespaced(&format!("auth:login_failures:{username}")) }
pub fn revoked_access_key(&self, jti: &str) -> String      { self.namespaced(&format!("auth:revoked_access:{jti}")) }
pub fn user_detail_key(&self, public_id: Uuid) -> String   { self.namespaced(&format!("user:detail:{public_id}")) }
```

最终形态：`axum:auth:login_failures:alice`、`axum:auth:revoked_access:<jti>`、`axum:user:detail:<uuid>`。

**为什么要分三类键空间？** 因为它们的数据类型、TTL、清理策略完全不同。混在一起时，一次"清缓存"就可能把限流计数一起删掉。测试锁定了这一点：

```rust
// src/infrastructure/redis.rs:360
fn key_categories_do_not_collide() { assert_ne!(store.login_failure_key("x"), store.revoked_access_key("x")); }
```

### 连接池：为什么不是"一请求一连接"

```rust
// src/infrastructure/redis.rs:33
/// 池中保存的是 `MultiplexedConnection`：redis-rs 的一条多路复用连接本身就能
/// 承载并发请求，因此这里的池**不是**「一个请求一条连接」，而是限制突发并发
/// 与连接重建次数。
```

Redis 协议支持在一条连接上并发多个请求（请求带序号，响应回来匹配）。所以 `pool_size = 8` 已经能支撑相当大的吞吐，不要按"并发数"来配。

```rust
// src/infrastructure/redis.rs:143
pub fn from_config(config: &RedisConfig) -> Result<Self, RedisStoreError> {
    let client = Client::open(config.url.clone()).map_err(|e| RedisStoreError::Config(e.to_string()))?;

    let pool = Pool::builder()
        .max_size(config.pool_size)
        .connection_timeout(Duration::from_secs(5))
        .build_unchecked(client);        // ← 不建连！只建骨架

    Ok(Self { pool, key_prefix: normalize_prefix(&config.key_prefix), user_cache_ttl: ... })
}
```

`build_unchecked` 是个关键选择，带来两个好处：

1. **启动流程可以自己决定"连不上要不要退出"**——构造函数只管建骨架，探测交给 `ping()`。
2. **单元测试可以构造完整的 `AppState`**，不需要真的跑一个 Redis（`RedisStore::without_connection()`，`:318`）。

代价是"构造成功"不代表"能连上"，所以启动流程里必须显式 `ping()` 一次——这正是[第 02 章](./02-项目全景与启动流程.md)里第 8 步的由来。

**为什么保留检出前 `PING`**

```rust
// src/infrastructure/redis.rs:135
/// 保留 `test_on_check_out` 的默认值（首次使用前 `PING`）：redis-rs 的
/// `has_broken` 恒返回 `false`，池无法自行发现失效连接，检出前的 `PING`
/// 是唯一能让坏连接被剔除的机制。
```

代价是每次取连接多一次往返。作者明确写了权衡：当前"一次请求 1~3 条 Redis 命令"的负载可以接受；**如果将来单请求命令数变多，应该合并命令，而不是关掉这项检查**。这类"留下判断依据"的注释，比单纯写"开启 PING"有用得多。

## 10.3 场景一：滑动窗口登录限流（ZSET + Lua）

### 为什么不用 `INCR` + `EXPIRE`

```text
固定窗口（INCR + EXPIRE）：
  [-------- 15 分钟窗口 --------][-------- 下一个 15 分钟窗口 --------]
                           ↑ 边界处计数整体清零

攻击者策略：在每个窗口的最后 1 分钟猜 5 次，紧接着在新窗口再猜 5 次
→ 实际在 2 分钟内容忍了 10 次失败，限额翻倍
```

```text
滑动窗口（ZSET）：
  每次失败记录 (member=唯一ID, score=毫秒时间戳)
  判定时先删掉 (now - window) 之前的成员，再数剩下多少
→ 永远是"最近 15 分钟内失败了几次"，没有边界可利用
```

### 两条 Lua 脚本

**读取 + 修剪**（判定用）：

```lua
-- src/infrastructure/redis.rs:79
local key = KEYS[1]
local now = tonumber(ARGV[1])
local window = tonumber(ARGV[2])
redis.call('ZREMRANGEBYSCORE', key, '-inf', now - window)
return redis.call('ZCARD', key)
```

**修剪 + 写入 + 续期 + 统计**（记录用）：

```lua
-- src/infrastructure/redis.rs:98
local key = KEYS[1]
local now = tonumber(ARGV[1])
local window = tonumber(ARGV[2])
local member = ARGV[3]
redis.call('ZREMRANGEBYSCORE', key, '-inf', now - window)
redis.call('ZADD', key, now, member)
redis.call('PEXPIRE', key, window)
return redis.call('ZCARD', key)
```

为什么必须用脚本？

| 拆成多条命令的问题 | 后果 |
| --- | --- |
| 修剪与统计之间插入并发写入 | 计数不自洽（删了 3 个、数到 5 个） |
| `ZADD` 成功但 `PEXPIRE` 失败 | 键永不过期 → 内存泄漏 + 该用户名被永久限流 |

Lua 脚本在 Redis 里是**原子执行**的（单线程模型下整段连续跑完），所以"修剪 + 统计"和"修剪 + 写入 + 续期 + 统计"各自是一个不可分割的操作。这是本项目唯一必须用 Lua 的地方。

### `member` 为什么用 UUIDv7

```rust
// src/infrastructure/redis.rs:196
// 成员用 UUIDv7：同一毫秒内的两次失败也不会互相覆盖。
// 计数的精度依赖成员唯一，这里不能退化成「时间戳作为成员」。
let member = Uuid::now_v7().to_string();
```

ZSET 的成员是唯一的。如果用毫秒时间戳当成员，同一毫秒内的两次失败会变成 `ZADD` 更新已有成员的分数——**少记一次**。而限流的正确性完全依赖计数准确。

### 毫秒与最小值的兜底

```rust
// src/infrastructure/redis.rs:311
fn window_millis(window: Duration) -> i64 {
    window.as_millis().max(1) as i64      // 窗口为 0 会让所有成员立即过期 = 关闭限流
}
```

这和 `rate_limit_window` 的 `max(60)` 是同一个思路：**在一个错误取值会"静默削弱防护"的地方多加一道兜底。**

### 谁在用这两个方法

- 判定：`AuthService::count_login_failures`（`src/modules/auth/service.rs:151`）
- 记录：`AuthService::record_login_failure`（`:173`）

**失败时的降级**：判定回退数据库统计；记录只告警。详见[第 09 章 9.4](./09-认证与授权.md)。

## 10.4 场景二：access token 吊销列表（STRING + TTL）

```rust
// src/infrastructure/redis.rs:215
pub async fn revoke_access_token(&self, jti: &str, ttl: Duration) -> Result<(), RedisStoreError> {
    let key = self.revoked_access_key(jti);
    let mut connection = self.connection().await?;

    connection.set_ex::<_, _, ()>(key, "1", ttl.as_secs().max(1)).await
        .map_err(RedisStoreError::Command)
}
```

```rust
// src/infrastructure/redis.rs:226
pub async fn is_access_token_revoked(&self, jti: &str) -> Result<bool, RedisStoreError> {
    let key = self.revoked_access_key(jti);
    let mut connection = self.connection().await?;

    let exists: i64 = connection.exists(&key).await.map_err(RedisStoreError::Command)?;
    Ok(exists > 0)
}
```

四个设计点：

| 决定 | 原因 |
| --- | --- |
| 只存 `jti`，不存令牌 | Redis 泄露也无法据此伪造令牌 |
| 值是 `"1"` | 只关心"在不在"，不关心内容 |
| TTL = 令牌剩余有效期 | 令牌过期后记录无意义，让它一起消失，避免无限累积 |
| `SET ... EX` 单条命令 | 避免 `SET` 成功、`EXPIRE` 失败留下永不过期的键 |
| 查询用 `EXISTS` 而不是 `GET` | 不受值类型影响，语义更直接 |

**降级**：查询失败按"未吊销"放行（`src/common/auth.rs:110`），退化为"令牌到期才失效"。这是"立即生效"的增强，不是认证正确性的前提。

## 10.5 场景三：用户详情缓存（Cache-Aside）

```rust
// src/infrastructure/redis.rs:238
pub async fn get_json<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, RedisStoreError> {
    let raw: Option<String> = connection.get(key).await.map_err(RedisStoreError::Command)?;
    match raw {
        Some(raw) => serde_json::from_str(&raw).map(Some).map_err(RedisStoreError::Serde),
        None => Ok(None),          // 未命中 → None（不是错误）
    }
}
```

`Option<T>` 把"未命中"和"出错"分成两个维度，调用方才能分别处理：

- `Ok(Some(v))` → 命中
- `Ok(None)` → 未命中，查库
- `Err(e)` → 缓存故障，降级

### 缓存结构必须能反序列化回来

这里有个非常隐蔽的坑。`User` 的时间字段用了自定义序列化格式：

```rust
// src/modules/user/model.rs:19
#[serde(serialize_with = "serialize_datetime", deserialize_with = "deserialize_datetime")]
pub created_at: chrono::NaiveDateTime,
```

```rust
// src/modules/user/model.rs:83
fn serialize_datetime<T>(value: &chrono::NaiveDateTime, serializer: T) -> ... {
    serializer.serialize_str(&value.format("%Y-%m-%d %H:%M:%S").to_string())
}

fn deserialize_datetime<'de, D>(deserializer: D) -> Result<chrono::NaiveDateTime, D::Error> {
    let value = String::deserialize(deserializer)?;
    chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S").map_err(serde::de::Error::custom)
}
```

如果只自定义了序列化、反序列化用默认（chrono 默认期待 RFC3339），会发生什么？

```text
写入缓存：{"created_at": "2024-05-06 07:08:09"}     ← 自定义格式
读取缓存：尝试按 RFC3339 解析 → 失败 → Serde 错误
```

**结果是"缓存命中反而 500，走数据库却完全正常"**。这种 bug 只在有缓存、且缓存命中时暴露，本地开发甚至可能测不到。所以本项目专门写了一个往返测试：

```rust
// src/modules/user/model.rs:138
/// 缓存与响应共用同一个 `User` 结构，序列化结果必须能被反序列化回来。
fn user_round_trips_through_cache_json() { /* 序列化 → 反序列化 → 逐字段比对 */ }
```

> **通用教训**：任何"序列化到外部（缓存、消息队列、跨服务）"的类型，都必须有往返测试。这类错误的代价极高——数据格式不匹配往往在灰度后期才暴露。

### 一致性与穿透/击穿/雪崩

本项目采用的策略：

| 问题 | 本项目做法 |
| --- | --- |
| **穿透**（查不存在的 key，每次都打库） | 未做特殊处理；当前以 `public_id`（UUIDv7）为键，天然不可枚举，穿透风险低。若将来按用户名查询，应缓存空值或加布隆过滤器 |
| **击穿**（热点 key 失效瞬间大量请求打库） | 未做互斥锁；TTL 60 秒 + 单个 key 的并发量在当前规模下可接受 |
| **雪崩**（大量 key 同时失效） | TTL 固定，但用户详情 key 是分散写入的，天然错峰；规模变大后应加随机抖动（TTL ± 10%） |
| **一致性** | 写操作**删除**缓存而不是更新（[第 06 章](./06-Service层与业务规则.md)），TTL 兜底 |

**"知道但当前不做"是一个合格的工程判断**，前提是你写清楚为什么。反过来，"不知道有这些风险"才是问题。项目在 `reference/REDIS_GUIDE.md` 6.3 节里有完整讨论。

## 10.6 降级矩阵：这一节最值得背下来

| 功能 | Redis 正常 | Redis 故障 | 代码位置 | 安全影响 |
| --- | --- | --- | --- | --- |
| 登录失败限流 | Redis ZSET 判定 | **回退数据库统计** | `auth/service.rs:151` | 无（只是慢一点） |
| 失败计数写入 | Redis + 数据库 | 只写数据库，告警 | `auth/service.rs:173` | 无 |
| access token 吊销检查 | `EXISTS` 判定 | **按未吊销放行** | `common/auth.rs:104` | 登出的令牌活到自然过期（≤15 分钟） |
| 登出写吊销 | `SET EX` | 告警，继续清 Cookie | `auth/handler.rs:268` | 同上 |
| 用户详情缓存读 | 命中直接返回 | 告警，查数据库 | `user/service.rs:79` | 无 |
| 用户详情缓存回填 | `SET EX` | 告警 | `user/service.rs:91` | 无 |
| 缓存删除（更新/删除后） | `DEL` | 告警，靠 TTL | `user/service.rs:141` | 陈旧数据最多存活一个 TTL |
| `/ready` | 200 | **503** | `app/routes.rs:121` | 编排系统移除流量 |

**读这张表的正确方式**：每一行的"安全影响"都是**有界的、写明的**。这就是"降级策略必须写进文档和注释"的意义——不能在夜里三点靠猜。

## 10.7 动手练习

1. **观察限流键**：故意用错误密码登录 3 次，然后 `redis-cli` 里 `ZRANGE axum:auth:login_failures:<username> 0 -1 WITHSCORES`，确认每个成员是一次失败、分数是毫秒时间戳；再 `TTL` 看键的过期时间。
2. **验证滑动窗口**：把 `auth.login_window_minutes` 改成 1，登录失败 5 次触发 429，等 70 秒后重试，应该恢复。
3. **验证吊销 TTL**：登出后 `SCAN 0 MATCH axum:auth:revoked_access:*` 找到键，`TTL` 应接近 900 秒。
4. **制造缓存往返错误**：删掉 `User` 的 `deserialize_with` 只留 `serialize_with`，跑 `cargo test user_round_trips_through_cache_json`，看它如何拦住这个改动。**改完还原。**
5. **观察降级**：停掉 Redis，依次验证：登录仍可用（限流回退库）、`/auth/me` 仍可用（吊销检查放行）、用户详情仍可用（查库）、`/ready` 返回 503。日志里应该能看到对应的 warn。
6. **加一个"缓存空值"防穿透**：把 `get_user_by_public_id` 改成"查不到时也写一个短 TTL（如 10 秒）的哨兵值"，思考：这样做会不会掩盖"用户刚被创建"的情况？

## 10.8 自测题

1. 为什么 Redis 在本项目里可以"连不上也不退出"，而 PostgreSQL 不行？判断标准是什么？
2. 适配层为什么不能自己吞掉错误、返回默认值？
3. 为什么限流必须用 ZSET + Lua，而不能用 `INCR` + `EXPIRE`？除了边界问题，还有什么问题？
4. `member` 用时间戳会导致什么具体错误？
5. 吊销记录为什么"只存 jti、值是 1、TTL 等于剩余有效期"？
6. 缓存读取时 `Ok(None)` 与 `Err(_)` 的处理为什么必须不同？
7. 自定义序列化但忘了自定义反序列化，会在什么条件下暴露问题？为什么最难发现？
8. 请背出降级矩阵里"安全影响"这一列——如果记不住，说明你还不能独立判断"这个依赖能不能降级"。

---

下一步 → [11 可观测性与健康检查](./11-可观测性与健康检查.md)
