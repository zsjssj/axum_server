# Redis 学习与项目实战指南

> **文档定位**：本文是**参考手册**，用于查阅既定事实与操作步骤。
> 想按学习顺序理解设计意图，请从 [`../README.md`](../README.md) 开始读教程章节。

本文既是 Redis 的入门教程，也是本项目 Redis 用法的设计说明。读完并动手做完第 7 节的实验后，你应该能够：

- 说清 Redis 适合做什么、不适合做什么，以及它在本项目中承担哪三件事；
- 用 `redis-cli` 操作五种基本数据类型，并理解「过期」「命名空间」「原子性」三个关键概念；
- 看懂 `src/infrastructure/redis.rs` 中的每一行命令，知道为什么要用 Lua 和有序集合；
- 在 Redis 故障时准确说出每个功能会退化成什么样，并知道如何排查与恢复。

> 阅读建议：第 1~5 节是通用知识，可以边读边在 `redis-cli` 里敲命令；第 6 节开始才是本项目代码。如果你只想知道「这个项目怎么用 Redis」，直接跳到第 6 节，再回来补概念。

## 目录

1. [Redis 是什么，为什么这个项目需要它](#1-redis-是什么为什么这个项目需要它)
2. [环境准备](#2-环境准备)
3. [核心概念：键、过期与五种数据类型](#3-核心概念键过期与五种数据类型)
4. [原子性、事务与 Lua 脚本](#4-原子性事务与-lua-脚本)
5. [Rust 客户端与连接池](#5-rust-客户端与连接池)
6. [本项目中的三个 Redis 场景](#6-本项目中的三个-redis-场景)
7. [动手实验：观察每一项功能](#7-动手实验观察每一项功能)
8. [运维与排错手册](#8-运维与排错手册)
9. [生产环境实践](#9-生产环境实践)
10. [常见误区清单](#10-常见误区清单)
11. [代码与配置对照表](#11-代码与配置对照表)
12. [延伸学习路线](#12-延伸学习路线)

---

## 1. Redis 是什么，为什么这个项目需要它

Redis 是一个把数据放在内存里的键值存储（in-memory key-value store）。它有三个特征决定了它在本项目中的位置：

1. **快**：读写走内存，单次命令通常在微秒到毫秒级。数据库的 `COUNT(*)`、`SELECT` 在数据量大或索引不理想时会变成明显的延迟来源。
2. **数据结构丰富**：除了字符串，还提供列表、哈希、集合、有序集合、位图、HyperLogLog 等，可以直接表达计数器、排行榜、去重集合、延迟队列等结构。
3. **自带过期与原子操作**：`EXPIRE` / `TTL` 让「临时数据」自动消失，不需要写清理任务；单条命令天然原子，加上 Lua 脚本可以实现「多步操作不可被打断」。

### 1.1 和 PostgreSQL 的分工

本项目已经有 PostgreSQL，为什么还要 Redis？关键区别是**数据是否必须长期、可靠、可查询**：

| 关注点 | PostgreSQL | Redis |
| --- | --- | --- |
| 定位 | 权威数据存储（source of truth） | 加速层与临时状态 |
| 持久化 | 事务 WAL，保证不丢 | RDB/AOF 可持久化，但可能丢最后一段数据 |
| 数据丢失的后果 | 严重，需要恢复 | 一般可接受，可以从数据库重建 |
| 典型访问模式 | 复杂查询、关联、分页、约束 | 单键读写、计数、去重、过期 |
| 本项目用它存 | 用户、会话、登录失败审计 | 登录失败计数、令牌吊销记录、用户资料缓存 |

一个简单的判断规则：**如果一个字符串、计数或缓存丢了，你能否从数据库重新算出来？** 能，就适合放 Redis；不能，就必须留在数据库。

### 1.2 本项目用 Redis 做的三件事

| 场景 | 为什么放在 Redis |
| --- | --- |
| 登录失败计数 | 每次登录都要读一次，用窗口内的失败次数做限流；计数会随时间自动过期，天然适合 TTL |
| access token 吊销列表 | JWT 本身无状态，服务端要「登出后立即失效」就必须记一份短期黑名单，Redis 的 `SET ... EX` 正好表达「到令牌过期为止有效」 |
| 用户资料缓存 | 详情接口读多写少，缓存可以显著减少数据库压力；写操作后删缓存，用 TTL 兜底一致性 |

注意这三类数据都有同一特征：**可以重建**。这是允许「Redis 不可用时降级」的前提，也是第 6.4 节降级策略表的基础。

---

## 2. 环境准备

### 2.1 用 Docker 启动（推荐）

Windows 上没有官方 Redis 安装包，最省事的方式是 Docker：

```bash
# 启动一个带密码的 Redis 7（密码请自行替换）
docker run --name axum-redis -p 6379:6379 -d redis:7 \
  redis-server --requirepass "dev-password"

# 查看是否启动成功
docker ps --filter name=axum-redis

# 进入 redis-cli 交互界面
docker exec -it axum-redis redis-cli -a dev-password
```

如果使用项目默认配置（`redis://127.0.0.1:6379`、无密码），可以省掉 `--requirepass`：

```bash
docker run --name axum-redis -p 6379:6379 -d redis:latest
docker exec -it axum-redis redis-cli
```

> 想清空环境重新开始：`docker rm -f axum-redis` 后重新 `docker run`。**不要**在共享实例上使用 `FLUSHALL`。

### 2.2 不用 Docker 的替代方案

- **WSL2**：`wsl --install` 后 `sudo apt install redis-server`，再 `redis-server --daemonize yes`。Windows 侧访问 `127.0.0.1:6379` 即可（WSL2 默认会转发 localhost）。
- **RedisInsight / Another Redis Desktop Manager**：可视化客户端，适合观察键、TTL 和内存占用。初学阶段用 `redis-cli` 敲命令的收获更大，可视化工具作为辅助。

### 2.3 确认客户端与连通性

```bash
redis-cli -h 127.0.0.1 -p 6379 ping      # 期望 PONG
redis-cli info server | head -20          # 查看版本、运行模式
```

带密码时：

```bash
redis-cli -h 127.0.0.1 -p 6379 -a dev-password ping
# 交互式连接后也可以先执行 AUTH dev-password
```

若连接失败，按顺序检查：Redis 进程是否在跑（`docker ps` / `tasklist`）、端口是否被占用（`Get-NetTCPConnection -LocalPort 6379`）、是否设置了密码、是否只绑定了容器内部地址（`bind` 配置）。

### 2.4 让项目连上 Redis

```toml
# config/default.toml（已内置默认值）
[redis]
url = "redis://127.0.0.1:6379"
pool_size = 8
key_prefix = "axum"
user_cache_ttl_seconds = 60
```

也可以用环境变量覆盖（优先级高于配置文件）：

```bash
APP__REDIS__URL=redis://:dev-password@127.0.0.1:6379
```

启动服务后访问 `http://127.0.0.1:3000/ready`，返回体中的 `data.redis` 为 `connected` 即表示连接正常。没有启动 Redis 时服务仍能启动（第 6.4 节），但 `/ready` 会返回 `503`。

---

## 3. 核心概念：键、过期与五种数据类型

以下命令都可以直接粘贴到 `redis-cli` 中执行，`127.0.0.1:6379>` 是提示符。

### 3.1 键、值与命名空间

Redis 是扁平的键空间：所有数据都在同一个「目录」里，没有表、没有 schema。**键名本身就是你唯一的分层手段**。

约定俗成的做法是用 `:` 分段：

```text
axum:auth:login_failures:alice
axum:user:detail:01991f3a-7c40-7d62-8f6d-4fb743e0f731
```

- 第一段 `axum` 是命名空间（本项目对应 `redis.key_prefix`），用来区分「哪个应用 / 哪个环境」；
- 中间段表示业务域与用途；
- 最后一段是业务标识。

命令演示：

```text
127.0.0.1:6379> SET axum:demo:greeting "hello"
OK
127.0.0.1:6379> GET axum:demo:greeting
"hello"
127.0.0.1:6379> TYPE axum:demo:greeting
string
127.0.0.1:6379> EXISTS axum:demo:greeting
(integer) 1
127.0.0.1:6379> DEL axum:demo:greeting
(integer) 1
127.0.0.1:6379> GET axum:demo:greeting
(nil)
```

> **不要在生产环境用 `KEYS *`**：它会遍历整个键空间并阻塞其他命令。用 `SCAN` 游标迭代（见第 8 节）。键的数量很大时，`KEYS` 是事故级的操作。

### 3.2 过期：TTL 是 Redis 的灵魂

```text
127.0.0.1:6379> SET axum:demo:code "123456" EX 60     # 写入时就带 60 秒过期
OK
127.0.0.1:6379> TTL axum:demo:code
(integer) 60
127.0.0.1:6379> PERSIST axum:demo:code                 # 取消过期，变成永久键
(integer) 1
127.0.0.1:6379> TTL axum:demo:code
(integer) -1                                          # -1 表示没有设置过期
127.0.0.1:6379> DEL axum:demo:code
```

`TTL` 的三个返回值要记住：

| 返回值 | 含义 |
| ---: | --- |
| 正整数 | 剩余秒数 |
| `-1` | 键存在，但没有过期时间（永久） |
| `-2` | 键不存在 |

实践要点：

- **凡是临时数据都要设 TTL**。没有 TTL 的键是内存泄漏的常见来源。
- 过期是「惰性删除 + 定期抽样删除」结合：到了过期时间，键不会再被返回，但不一定立刻从内存释放。用 `INFO memory` 看内存不必对 `expired_keys` 过于敏感。
- `EXPIRE` 的单位是秒，`PEXPIRE` 是毫秒；`SET ... EX 60` 这种「写入即过期」比先 `SET` 再 `EXPIRE` 更安全，因为两条命令之间可能失败。

### 3.3 五种基本数据类型

#### STRING：字符串、数字、序列化对象

```text
127.0.0.1:6379> SET axum:demo:name "alice" NX        # 只在键不存在时写入
OK
127.0.0.1:6379> SET axum:demo:name "bob" NX
(nil)                                                # 已存在，未覆盖
127.0.0.1:6379> INCR axum:demo:counter               # 不存在时按 0 处理，结果为 1
(integer) 1
127.0.0.1:6379> INCRBY axum:demo:counter 10
(integer) 11
127.0.0.1:6379> MSET axum:demo:a 1 axum:demo:b 2     # 批量写
OK
127.0.0.1:6379> MGET axum:demo:a axum:demo:b
1) "1"
2) "2"
```

适用场景：计数器（`INCR`）、缓存 JSON、开关、限流标记。本项目的「access token 吊销记录」和「用户缓存」都是 STRING。

#### HASH：对象字段

```text
127.0.0.1:6379> HSET axum:demo:user name "alice" age 30
(integer) 2
127.0.0.1:6379> HGET axum:demo:user name
"alice"
127.0.0.1:6379> HGETALL axum:demo:user
1) "name"
2) "alice"
3) "age"
4) "30"
127.0.0.1:6379> HINCRBY axum:demo:user age 1
(integer) 31
```

优点是可以只读写单个字段；缺点是过期只能作用于整个键，无法给单个字段设置 TTL。本项目缓存整个 `User` 对象，用 STRING + JSON 更直接。

#### LIST：有序、可重复

```text
127.0.0.1:6379> RPUSH axum:demo:queue "job-1" "job-2"
(integer) 2
127.0.0.1:6379> LPOP axum:demo:queue
"job-1"
127.0.0.1:6379> LRANGE axum:demo:queue 0 -1
1) "job-2"
```

典型用途：简单队列、最近 N 条记录（配合 `LTRIM`）。注意它不保证「至少消费一次」，可靠队列需要 Stream 或专门的消息队列。

#### SET：无序、去重

```text
127.0.0.1:6379> SADD axum:demo:tags "rust" "redis" "rust"
(integer) 2                                          # 自动去重
127.0.0.1:6379> SMEMBERS axum:demo:tags
1) "redis"
2) "rust"
127.0.0.1:6379> SISMEMBER axum:demo:tags "rust"
(integer) 1
```

典型用途：去重、标签、共同好友（`SINTER`）。

#### ZSET：带分数的有序集合——本项目的限流基础

ZSET 的每个成员都有一个 `score`，Redis 按 score 排序，并支持按分数范围增删与计数：

```text
127.0.0.1:6379> ZADD axum:demo:scores 100 "alice" 80 "bob" 60 "carol"
(integer) 3
127.0.0.1:6379> ZRANGE axum:demo:scores 0 -1 WITHSCORES
1) "carol"
2) "60"
3) "bob"
4) "80"
5) "alice"
6) "100"
127.0.0.1:6379> ZREMRANGEBYSCORE axum:demo:scores -inf 70   # 删除 score <= 70 的成员
(integer) 1
127.0.0.1:6379> ZCARD axum:demo:scores
(integer) 2
```

**把「时间戳」当 score，「一次失败」当成员，就得到一个滚动窗口计数器**：删除 `now - window` 之前的成员，剩下的数量就是窗口内的失败次数。这正是本项目实现登录限流的方式（第 6.1 节）。

为什么不用简单的 `INCR` 计数？`INCR` + `EXPIRE` 是**固定窗口**：窗口从第一次写入开始计时，到点整体清零。攻击者只要在窗口边界前后各打一批请求，就能把允许次数翻倍。ZSET 按事件时间裁剪，是真正的滑动窗口，代价是每次失败多存一个成员。

### 3.4 内存与淘汰策略

Redis 把数据全放内存，所以必须有上限策略：

```text
127.0.0.1:6379> CONFIG GET maxmemory
1) "maxmemory"
2) "0"                        # 0 表示不限制，生产环境必须设置
127.0.0.1:6379> CONFIG GET maxmemory-policy
1) "maxmemory-policy"
2) "noeviction"               # 内存满时写入直接报错
```

常见淘汰策略：

| 策略 | 行为 |
| --- | --- |
| `noeviction` | 内存满时写入报错，读请求仍可用 |
| `allkeys-lru` / `allkeys-lfu` | 在所有键中按最近/最常使用淘汰 |
| `volatile-lru` / `volatile-lfu` | 只在设置了 TTL 的键中淘汰 |
| `volatile-ttl` | 优先淘汰剩余寿命最短的键 |
| `allkeys-random` | 随机淘汰 |

缓存型用法通常选 `allkeys-lru`；如果 Redis 同时存「不能丢的计数」和「可丢的缓存」，用 `volatile-*` 并保证两类键的 TTL 设置符合预期。本项目的三类数据都可以重建，因此 `allkeys-lru` 是安全的选择。

---

## 4. 原子性、事务与 Lua 脚本

### 4.1 单条命令是原子的，多条不是

Redis 执行命令的主线程一次只处理一条命令，因此**单条命令不会看到中间状态**。但两条命令之间可以插入其他客户端的命令：

```text
# 客户端 A 的意图：计数 +1，并在第一次计数时设置 60 秒过期
127.0.0.1:6379> INCR  axum:demo:attempts      # 返回 1
127.0.0.1:6379> EXPIRE axum:demo:attempts 60
# 如果 A 在两条命令之间崩溃，这个键就永远不过期（内存泄漏 + 永久限流）
```

### 4.2 MULTI / EXEC 不能解决「读后写」

`MULTI` 把多条命令打包执行，但**它不会在打包期间阻止其他客户端修改数据**，而且不能在事务里根据读取结果决定后续命令。`WATCH` 可以实现乐观锁（值被改动则 `EXEC` 失败），但会引入重试逻辑，代码变复杂。

### 4.3 Lua 脚本：服务端的多步原子操作

Redis 内置 Lua 解释器，`EVAL` 会在服务端一次性执行整段脚本：脚本执行期间不会被其他命令打断，天然满足「读取 → 判断 → 写入」的原子性。

```text
# KEYS[1] = 键名，ARGV[1] = 过期秒数
127.0.0.1:6379> EVAL "local n = redis.call('INCR', KEYS[1]); redis.call('EXPIRE', KEYS[1], ARGV[1]); return n" 1 axum:demo:attempts 60
(integer) 1
127.0.0.1:6379> TTL axum:demo:attempts
(integer) 60
```

调用方式：

```text
EVAL <脚本> <键的数量> <键...> <参数...>
```

- `KEYS` 必须是真正的键名，便于 Redis Cluster 做路由与检查；
- 其他数据（过期秒数、时间戳、成员标识）走 `ARGV`；
- 不要在脚本里拼接用户输入，全部通过 `KEYS` / `ARGV` 传入。

### 4.4 脚本缓存与 EVALSHA

Redis 会按脚本内容计算 SHA1，客户端可以先用 `SCRIPT LOAD` 上传，再用 `EVALSHA sha1 ...` 调用，避免每次都传输脚本文本。脚本被 `SCRIPT FLUSH` 清掉或首次执行时，`EVALSHA` 会返回 `NOSCRIPT` 错误，需要回退到 `EVAL` 重新加载。

**redis-rs 的 `Script` 类型已经帮你处理了这一切**：它默认先尝试 `EVALSHA`，遇到 `NOSCRIPT` 自动 `EVAL` 加载后重试。因此本项目的代码里看不到 `SCRIPT LOAD`。

### 4.5 写脚本的三个约束

1. **只做 O(1) 或可控规模的循环**：脚本执行期间 Redis 不处理其他命令，遍历大集合会阻塞整个实例。Lua 默认有 5 秒超时（`lua-time-limit`），超时后其他命令会收到 `BUSY`，只能用 `SCRIPT KILL` 或重启处理。
2. **Cluster 模式下所有键必须在同一个 slot**：脚本只能访问 `KEYS` 中声明的键，且这些键要通过 hash tag（`{...}`）落在同一 slot。本项目的脚本只操作一个键，因此不受影响。
3. **时间和随机性交给客户端**：本项目把「当前毫秒时间戳」和「失败标识」作为 `ARGV` 传入，而不是在脚本里调用 `TIME` / `RANDOMKEY`。这样脚本行为可复现，调试和测试都更简单（Redis 5 之后脚本不要求确定性，但把时间外置仍然是更好的习惯）。

---

## 5. Rust 客户端与连接池

### 5.1 依赖与特性

```toml
# Cargo.toml（节选）
redis = { version = "1.7.1", default-features = false, features = ["tokio-comp", "bb8", "script"] }
bb8 = "0.9.1"
```

- `tokio-comp`：Tokio 运行时下的异步连接；
- `bb8`：为 `redis::Client` 实现连接池的 `ManageConnection`；
- `script`：启用 `Script` / `EVAL` 支持（本项目限流脚本需要）。

### 5.2 为什么是「连接池 + 多路复用」

`redis` 为 Tokio 提供的 `MultiplexedConnection` 可以在一条 TCP 连接上并发发送多个命令（内部有请求队列，按顺序读取响应）。因此一条连接就能服务大量并发请求。

bb8 在这里的作用不是「一个请求一条连接」，而是：

- 限制同时存在的连接数上限（`pool_size`），避免突发流量打出成百上千条连接；
- 连接失效时重新创建（检出前会执行一次 `PING`，见 5.4）；
- 提供连接获取超时，避免数据库阻塞时把请求无限挂起。

### 5.3 错误要分类

`src/infrastructure/redis.rs` 定义了 `RedisStoreError`，把失败分成三类：

| 变体 | 含义 | 调用方通常怎么做 |
| --- | --- | --- |
| `Config` | 连接地址解析失败 | 启动失败，修配置 |
| `Connection` | 取不到连接（不可达、超时） | 降级 + 告警 |
| `Command` | 命令执行失败（如 `WRONGTYPE`） | 记日志；若是代码缺陷则修代码 |
| `Serde` | 缓存值序列化/反序列化失败 | 视为缺陷，记日志 |

把「连不上」和「命令写错」混成同一个字符串，排障时就无法判断该重试还是该改代码。

变体的 `Display`、`source` 与 `From` 转换由 `thiserror` 在枚举上派生：只有 `Command` 与 `Serde` 带底层原因，`Config` 与 `Connection` 没有来源可追。把 `#[from]` 写在变体上，读枚举就能确认这个分布，不必再去另一个 `match` 里核对。

### 5.4 一个容易被忽略的细节：检出前 PING

bb8 的 `ManageConnection::has_broken` 在 redis 的实现里**恒返回 `false`**，也就是说池子无法感知「连接已经断了」。所以本项目保留 `test_on_check_out` 的默认值：每次取连接前执行 `PING`，失败就丢弃重建。

代价是每次取连接多一次往返。对「一次请求 1~3 条 Redis 命令」的负载可以接受；如果未来单请求命令数显著变多，正确做法是**合并命令**（pipeline / 一次脚本），而不是关掉这项检查。

> bb8 创建连接池时会启动后台回收任务，因此 `RedisStore::from_config` 必须在 Tokio 运行时内调用。单元测试要用 `#[tokio::test]` 而不是 `#[test]`。

---

## 6. 本项目中的三个 Redis 场景

代码入口：`src/infrastructure/redis.rs`（适配层）、`src/modules/auth/service.rs`（限流与吊销）、`src/common/auth.rs`（认证中间件校验吊销）、`src/modules/user/service.rs`（缓存）。

### 6.1 场景一：滑动窗口登录限流（ZSET + Lua）

**要解决的问题**：同一用户名在 15 分钟内失败 5 次后，暂停接受登录尝试，返回 `429`。既不能用「锁定账号」的方式（攻击者可以故意输错把别人锁死），也不能每次失败都全表 `COUNT(*)`。

**键设计**：

```text
axum:auth:login_failures:<username>     ZSET，member = 一次失败的唯一标识，score = 失败时间（毫秒）
```

**读取计数**（`LOGIN_FAILURE_COUNT_SCRIPT`）：

```lua
local key = KEYS[1]
local now = tonumber(ARGV[1])
local window = tonumber(ARGV[2])
redis.call('ZREMRANGEBYSCORE', key, '-inf', now - window)
return redis.call('ZCARD', key)
```

**记录失败**（`LOGIN_FAILURE_RECORD_SCRIPT`）：

```lua
local key = KEYS[1]
local now = tonumber(ARGV[1])
local window = tonumber(ARGV[2])
local member = ARGV[3]
redis.call('ZREMRANGEBYSCORE', key, '-inf', now - window)
redis.call('ZADD', key, now, member)
redis.call('PEXPIRE', key, window)
return redis.call('ZCARD', key)
```

为什么这样设计：

- **修剪 + 统计放在同一个脚本**：两条独立命令之间可能插入新的失败写入，导致「刚修剪完又统计」得到不自洽的数字。
- **成员用 UUIDv7 而不是时间戳**：同一毫秒内两次失败如果用同一个成员，`ZADD` 只会更新分数，计数会少记。UUIDv7 本身包含时间信息，也便于排查。
- **`PEXPIRE` 重置整个键的寿命**：只要还有失败在写入，键就一直存在；连续 15 分钟没有失败，键自动消失，不需要清理任务。
- **成功登录不清空计数**：清空等于告诉攻击者「只要偶尔成功一次就能重置限额」。数据库中的过期审计记录会被清理（`clear_failures_before`），Redis 的窗口内计数保留。

**调用顺序**（`AuthService::login`）：

```text
统计窗口内失败次数 → 达到阈值返回 429
        ↓ 未达到
查询用户凭证（用户不存在也执行一次假校验，抹平响应时间）
        ↓
Argon2 校验密码 → 失败：写审计表 + 写 Redis 计数，返回 401
        ↓ 成功
清理窗口外审计记录 → 签发令牌对
```

顺序不能颠倒：Argon2 很慢，如果先校验密码，大量失败请求会持续占满阻塞线程池，限流本身反而变成消耗算力的攻击面。

**一个必须知道的取舍**：Redis 是计数的主路径，但它不是唯一记录——失败同时写入数据库审计表。因此：

- Redis 不可用时，判定回退到数据库统计，限流照常生效；
- Redis 重启（数据丢失）后，窗口内的历史失败只留在数据库里，Redis 计数从 0 开始。由于攻击者无法主动触发重启，这个窗口期的风险可以接受；若把「重启也不能丢计数」当作硬要求，就应该把计数完全放数据库，或为 Redis 开启 AOF（仍然不是强一致保证）。

### 6.2 场景二：access token 吊销列表（STRING + TTL）

**要解决的问题**：JWT 无状态，登出后已签发的 access token 在 15 分钟内仍然有效。以前只能在文档里解释「这是双令牌方案的固有性质」；引入 Redis 之后可以做到**登出立即失效**。

**键设计**：

```text
axum:auth:revoked_access:<jti>     STRING，值固定为 "1"，TTL = 令牌剩余有效期
```

- `jti` 是 JWT 的唯一标识，签发时由 `public_id + 毫秒时间戳` 组合生成，同一用户同一秒多次登录也不会重复；
- **TTL 用「剩余有效期」而不是固定值**：令牌自然过期后，吊销记录不再有任何作用，让它和令牌一起消失，键不会无限累积；
- 写入用 `SET key "1" EX <秒>` 单条命令，避免 `SET` 成功、`EXPIRE` 失败的中间状态。

**登出时**（`AuthService::revoke_access_token`）：

```text
读取 access_token Cookie → 解析 JWT → 得到 jti 和 exp
        ↓ 无法解析 / 已过期 → 直接返回（这类令牌本来就通过不了中间件）
剩余秒数 = exp - 当前时间 → SET axum:auth:revoked_access:<jti> "1" EX 剩余秒数
```

**每次认证时**（`common::auth::authenticate`）：

```text
校验 JWT 签名 / 签发者 / 有效期 / typ
        ↓
EXISTS axum:auth:revoked_access:<jti>
        ↓ 存在 → 401「认证令牌已失效，请重新登录」
        ↓ 不存在 → 放行
```

**失败时为什么放行**：吊销检查失败（Redis 不可达）时，本项目选择「按未吊销处理」并记录告警。理由是吊销是「登出立即生效」的增强，不是令牌有效性的前提；Redis 故障时退化为改动前的行为（令牌到期才失效），比把所有已登录用户一次性踢下线更合理。这个取舍写在第 6.4 节的降级表里。

> 副作用：吊销列表带来一次额外的 Redis 往返（每次认证一次 `EXISTS`）。这也是为什么第 5.4 节强调「命令要合并」——如果未来每个请求再加几项 Redis 查询，就应该把它们合并成一次脚本或 pipeline。

### 6.3 场景三：用户详情缓存（Cache-Aside）

**要解决的问题**：`GET /api/v1/users/{public_id}` 是读多写少的接口，每次请求都查库没有必要。

**键设计**：

```text
axum:user:detail:<public_id>     STRING，值是 User 的 JSON，TTL = 60 秒（可配置）
```

**读路径**（`UserService::get_user_by_public_id`）：

```text
GET 缓存 → 命中：直接返回（不查库）
   ↓ 未命中
查数据库 → 不存在：404
   ↓ 存在
SET 缓存 EX 60 → 返回
```

**写路径**（更新、删除）：

```text
UPDATE/DELETE 数据库 → 成功 → DEL 缓存 → 返回
```

几个关键决定：

- **先写数据库、再删缓存**。如果先删缓存再更新数据库，并发请求可能在两步之间把旧值查出来重新写入缓存，留下一个直到 TTL 到期才自愈的脏数据。
- **删缓存而不是更新缓存**。并发更新时，「谁后写缓存」决定最终内容，可能与数据库顺序相反；删掉之后由下一次读请求回填，结果必然与数据库一致。
- **回填/删除失败只告警**。缓存是加速层：写缓存失败不应该让一次成功的查询变成 500；删缓存失败由 TTL 兜底（最多陈旧 60 秒）。
- **列表接口不缓存**。分页结果和总数会被任意一次写入整体失效，命中率低、失效成本高；真正热点是单个详情。
- **TTL 是兜底而不是主要一致性手段**。主动删除负责「写后立即正确」，TTL 负责「删除失败后最多错多久」。

**缓存三兄弟**（面试与排障高频）：

| 名称 | 现象 | 常见对策 |
| --- | --- | --- |
| 缓存穿透 | 查询不存在的数据，每次都打到数据库 | 缓存空值（带较短 TTL）、布隆过滤器、参数校验 |
| 缓存击穿 | 某个热点键过期瞬间，大量请求同时回源 | 互斥重建（分布式锁）、逻辑过期、热点不过期 |
| 缓存雪崩 | 大量键在同一时间过期，或 Redis 整体不可用 | TTL 加随机抖动、多级缓存、Redis 高可用、降级回源 |

本项目选择了「回源 + TTL」的最小实现：用户不存在时不缓存空值（避免缓存穿透需要额外处理）、没有互斥重建。这些是文档里留给读者的扩展练习（见第 12 节）。

### 6.4 降级策略：Redis 挂了会怎样

Redis 在本项目是「可选加速组件」，每个使用点都定义了降级路径：

| 功能 | Redis 正常 | Redis 不可用 | 影响 |
| --- | --- | --- | --- |
| 登录失败计数 | Redis 滑动窗口 | 回退 `login_failures` 表统计 | 限流仍有效，只是每次多一条 SQL |
| 失败记录写入 | 审计表 + Redis 计数 | 审计表 + 告警 | 限流回退数据库，行为不变 |
| 登出吊销 access token | 写吊销列表，立即失效 | 告警并跳过 | 令牌自然过期前仍有效（与改动前一致） |
| 认证时吊销检查 | `EXISTS` 查询 | 告警并按未吊销放行 | 同上 |
| 用户详情查询 | 缓存优先 | 直接查库 | 延迟上升，功能正常 |
| 更新/删除失效 | `DEL` 缓存 | 告警 | 陈旧数据最多存在一个 TTL |
| `/ready` | `200` | `503` | 编排系统应把实例移出流量 |

**启动时不因 Redis 失败而退出**：引入一个加速组件不应该让原有服务在没有 Redis 的开发环境里完全无法启动；但 `/ready` 会明确标记未就绪，生产环境的编排系统据此不路由流量。两件事配合，既能本地开箱即用，又不会让生产环境「假装健康」。

### 6.5 就绪检查

`GET /ready` 会依次检查数据库与 Redis：

```json
{
  "code": 0,
  "message": "请求成功",
  "data": {
    "status": "ready",
    "database": "connected",
    "redis": "connected",
    "timestamp": "2026-01-01T00:00:00+00:00"
  }
}
```

任一依赖不可用返回 `503`（业务码 `50300`），`message` 分别为「数据库不可用」或「Redis 不可用」。`/health` 只表示进程能响应，不检查依赖，适合做存活探针（liveness）；`/ready` 适合做就绪探针（readiness）。

---

## 7. 动手实验：观察每一项功能

以下实验需要：PostgreSQL 可用、Redis 可用、服务已启动、已有一个可登录账号（见 `docs/reference/AUTH_API.md` 的「首次启动」）。命令行以 `curl` 为例，Windows PowerShell 中把 `\` 换行改为反引号或写在一行里。

### 实验 1：观察登录失败计数

```bash
# 故意用错密码登录 3 次
for i in 1 2 3; do
  curl -s -X POST http://127.0.0.1:3000/api/v1/auth/login \
    -H "Content-Type: application/json" \
    -d '{"username":"alice","password":"wrong-password"}' | head -c 200; echo
done

# 查看 Redis 中的计数（key_prefix 默认 axum）
redis-cli ZCARD axum:auth:login_failures:alice
# 期望输出 3

redis-cli TTL axum:auth:login_failures:alice
# 期望输出接近 900（15 分钟窗口）

redis-cli ZRANGE axum:auth:login_failures:alice 0 -1 WITHSCORES
# 每个成员是一次失败，score 是毫秒时间戳
```

### 实验 2：触发 429

继续再失败两次（累计 5 次），下一次尝试（第 6 次）就会被直接拒绝：

```bash
curl -s -o /dev/null -w '%{http_code}\n' -X POST http://127.0.0.1:3000/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"alice","password":"wrong-password"}'
# 期望输出 429
```

响应体：

```json
{
  "code": 42900,
  "message": "登录失败次数过多，请稍后再试",
  "data": null
}
```

注意此时即使密码正确也会被拒绝，这是限流的设计语义。等待窗口过去（或观察用短窗口配置的实例）后自动恢复：

```bash
redis-cli DEL axum:auth:login_failures:alice   # 仅用于本地实验
```

### 实验 3：缓存命中与失效

```bash
# 1. 查询详情（首次会回源数据库并回填缓存）
USER_ID=01991f3a-7c40-7d62-8f6d-4fb743e0f731
curl -s -b cookies.txt "http://127.0.0.1:3000/api/v1/users/$USER_ID" | head -c 300; echo

# 2. 直接查看缓存值
redis-cli GET "axum:user:detail:$USER_ID"
redis-cli TTL "axum:user:detail:$USER_ID"      # 期望接近 60

# 3. 更新用户后缓存应被删除
curl -s -b cookies.txt -X PUT "http://127.0.0.1:3000/api/v1/users/$USER_ID" \
  -H "Content-Type: application/json" \
  -d '{"nickname":"改名了"}' | head -c 200; echo

redis-cli EXISTS "axum:user:detail:$USER_ID"   # 期望 0
```

再请求一次详情，缓存会被重新写入；此时如果直接用 `redis-cli DEL` 删掉缓存，第二次请求仍会成功（回源数据库），这就是第 6.3 节的「缓存只是加速层」。

### 实验 4：登出后 access token 立即失效

```bash
# 1. 登出（cookies.txt 中带有 access_token 与 refresh_token）
curl -s -b cookies.txt -X POST http://127.0.0.1:3000/api/v1/auth/logout; echo

# 2. 查看吊销记录（jti 很长，用 scan 找）
redis-cli --scan --pattern 'axum:auth:revoked_access:*'

# 3. 关键验证：继续用旧的 access_token 访问受保护接口
#    步骤 1 只用了 -b，没有把响应里的清除 Cookie 写回文件，
#    所以 cookies.txt 里仍是登出前的令牌；这里为了清晰手工传入：
curl -s -H "Cookie: access_token=<登出前的 access_token>" \
  http://127.0.0.1:3000/api/v1/auth/me
# 期望 401，message 为「认证令牌已失效，请重新登录」
```

如果拿到的是「未登录或登录已过期」，说明 Cookie 没有带上；如果能正常返回用户资料，检查 `jti` 是否写入了吊销列表、`EXISTS` 是否执行。

### 实验 5：停掉 Redis，观察降级

```bash
docker stop axum-redis

curl -s -o /dev/null -w 'ready: %{http_code}\n' http://127.0.0.1:3000/ready
# 期望 ready: 503

curl -s -o /dev/null -w 'health: %{http_code}\n' http://127.0.0.1:3000/health
# 期望 health: 200

# 登录仍然可用：限流回退到 login_failures 表
curl -s -X POST http://127.0.0.1:3000/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"alice","password":"正确的密码"}' | head -c 200; echo

# 用户详情仍然可用：缓存跳过，直接查库
curl -s -b cookies.txt "http://127.0.0.1:3000/api/v1/users/$USER_ID" | head -c 200; echo

docker start axum-redis
```

同时观察服务日志：应出现「Redis 登录失败计数不可用，回退到数据库统计」「读取用户缓存失败，直接查询数据库」等告警，而不是 500 错误。这正是「降级」的可观测证据。

---

## 8. 运维与排错手册

### 8.1 常用命令

| 目的 | 命令 |
| --- | --- |
| 看整体状态 | `INFO`（`INFO memory` / `INFO stats` / `INFO keyspace`） |
| 看有多少键、有哪些库 | `DBSIZE`、`INFO keyspace` |
| 按模式遍历键 | `SCAN 0 MATCH 'axum:auth:*' COUNT 100` |
| 看键的类型/编码/内存 | `TYPE key`、`OBJECT ENCODING key`、`MEMORY USAGE key` |
| 看键的剩余寿命 | `TTL key` / `PTTL key` |
| 看慢命令 | `SLOWLOG GET 10`、`CONFIG GET slowlog-log-slower-than` |
| 看客户端连接 | `CLIENT LIST`、`CLIENT INFO` |
| 实时看命令流 | `MONITOR`（**只在排障时短时间使用**，会显著影响性能） |
| 找大 key | `redis-cli --bigkeys`（会遍历，低峰期执行） |
| 延迟诊断 | `redis-cli --latency`、`LATENCY DOCTOR` |
| 清理指定前缀 | `redis-cli --scan --pattern 'axum:*' \| xargs -r redis-cli DEL`（谨慎，先在测试环境验证） |

### 8.2 常见错误与处理

| 错误 | 常见原因 | 处理 |
| --- | --- | --- |
| `NOAUTH Authentication required` | 实例设置了 `requirepass`，客户端没带密码 | 在 URL 中写 `redis://:password@host:6379` 或执行 `AUTH` |
| `WRONGTYPE Operation against a key holding the wrong kind of value` | 同一个键被不同结构使用（例如先当 STRING 又当 ZSET） | 检查键名是否冲突；本项目的三类键有独立前缀 |
| `OOM command not allowed when used memory > 'maxmemory'` | 达到内存上限且策略为 `noeviction` | 淘汰策略改为 `allkeys-lru`、清理大 key、扩容 |
| `READONLY You can't write against a read only replica` | 连到了从节点 | 写请求指向主节点，或检查哨兵/Cluster 配置 |
| `LOADING Redis is loading the dataset in memory` | 实例重启后加载 RDB/AOF | 等待加载完成，就绪探针应在此期间返回失败 |
| `BUSY Redis is busy running a script` | Lua 脚本执行超过 `lua-time-limit` | 优化脚本，必要时 `SCRIPT KILL`（仅对未写入的脚本有效） |
| `MISCONF Errors writing to the AOF file` | 磁盘满或 AOF 写入失败 | 清理磁盘，检查 `stop-writes-on-bgsave-error` |
| `NOAUTH` 之外的 `NOPERM` | ACL 用户权限不足 | 检查 ACL 规则 |

### 8.3 本项目的排错顺序

1. `/ready` 返回什么？`data.redis` 是否为 `connected`？
2. 服务日志里是否出现「Redis 不可用」「回退到数据库统计」等告警？
3. `redis-cli ping` 是否正常？URL 中的密码、库号（`/0`）、IPv6、TLS（`rediss://`）是否正确？
4. `SCAN 0 MATCH 'axum:*'` 能否看到项目写入的键？键前缀是否与 `redis.key_prefix` 一致？
5. 键是否在预期时间过期？`TTL` 是否符合窗口 / 缓存配置？
6. 限流不生效时：`ZCARD axum:auth:login_failures:<username>` 是否增长？`AUTH__MAX_LOGIN_FAILURES` 与窗口配置是否被环境变量覆盖？

---

## 9. 生产环境实践

### 9.1 持久化

| 方式 | 原理 | 优点 | 代价 |
| --- | --- | --- | --- |
| RDB | 定时生成内存快照 | 文件小、恢复快 | 两次快照之间的数据可能丢失 |
| AOF | 追加写命令日志，`fsync` 策略可选 | 丢失窗口小（`everysec` 最多约 1 秒） | 文件更大、恢复更慢 |
| 混合（Redis 4+） | AOF 重写时嵌入 RDB 前缀 | 兼顾恢复速度与丢失窗口 | 配置更复杂 |

本项目的 Redis 数据都可以重建，**持久化不是关键**；但如果把登录失败计数视为安全审计的一部分，可以开启 AOF。真正不能丢的数据始终在 PostgreSQL。

### 9.2 高可用

- **主从复制**（`replicaof`）：从节点只读，提供读扩展与故障转移基础。
- **Sentinel**：监控主节点，自动选出新主并通知客户端。客户端需要支持 Sentinel（`redis` crate 对应 `sentinel` 特性）。
- **Cluster**：数据分片到 16384 个 slot，横向扩容。约束：多键命令与 Lua 只能操作同一 slot 的键，需要 hash tag（`{user:1}:...`）。

本项目当前使用单实例配置，切换 Sentinel 或 Cluster 需要同时调整客户端配置与键设计（尤其是脚本的键必须同 slot）。在单实例阶段就把键前缀设计好，是为了将来迁移时能按前缀识别数据来源。

### 9.3 安全

- **设置密码**：`requirepass`，或使用 ACL（Redis 6+）为不同应用分配最小权限用户。
- **不要暴露公网**：`bind 127.0.0.1` 或内网地址 + 安全组；默认端口 6379 是扫描重灾区。
- **TLS**：使用 `rediss://`，依赖已开启 `tokio-rustls-comp` 特性，证书默认通过系统根证书校验；私有 CA 需要额外配置证书来源。
- **禁用危险命令**：`rename-command FLUSHALL ""`、`rename-command KEYS ""`（Redis 7 可用 ACL 控制）。
- **不要把 Redis 当数据库**：它能丢数据，权限模型也比关系库简单。

### 9.4 部署检查清单

- [ ] `maxmemory` 与 `maxmemory-policy` 已按用途设置（缓存型建议 `allkeys-lru`）。
- [ ] 生产环境 `key_prefix` 与开发/测试不同，避免同一个实例互相污染。
- [ ] 监控：内存使用率、命中率（`keyspace_hits` / `keyspace_misses`）、连接数、慢日志、`evicted_keys`。
- [ ] 告警：Redis 不可达、内存接近上限、`evicted_keys` 异常增长、命令延迟。
- [ ] 备份策略明确（即使本项目数据可重建）。
- [ ] 应用侧：连接池上限、获取连接超时、降级开关与日志告警已就绪。
- [ ] `/ready` 已接入编排系统的就绪探针。

---

## 10. 常见误区清单

1. **用 `KEYS *` 遍历生产数据**：阻塞主线程，键多时直接卡死。用 `SCAN`。
2. **把 Redis 当主存储**：没有事务隔离、没有约束、持久化可能丢数据。权威数据留在 PostgreSQL。
3. **不设 TTL**：临时数据忘记过期，内存持续增长，最终触发淘汰或 OOM。
4. **键名不划分命名空间**：多环境共用一个实例时互相覆盖，甚至误删。
5. **以为 `INCR` + `EXPIRE` 是滑动窗口**：它是固定窗口，窗口边界会翻倍。需要精确窗口用 ZSET（本项目）或令牌桶。
6. **在循环里逐条调用 Redis**：每次一个 RTT，延迟线性增长。用 `MGET`、pipeline 或脚本合并。
7. **大 key**：一个键存几十万元素（大 HASH、大 LIST、大 ZSET），读写、删除、迁移都会卡顿。控制单键规模，必要时拆分。
8. **缓存与数据库双写顺序混乱**：先删缓存再写库、或更新缓存而不是删除，都会留下脏数据。用 Cache-Aside：先写库、再删缓存、TTL 兜底。
9. **忽略缓存穿透**：查询不存在的数据每次都打数据库，需要空值缓存或布隆过滤器。
10. **在 Lua 脚本里做重活**：脚本执行期间阻塞整个实例，必须保持 O(1) 或小规模循环。
11. **Redis 故障时假设「什么都没发生」**：每个使用点都要明确「降级还是拒绝」，并把策略写进文档（见 6.4）。

---

## 11. 代码与配置对照表

### 11.1 代码位置

| 关注点 | 文件 |
| --- | --- |
| 连接池、键命名、限流/吊销/缓存原语、Lua 脚本 | `src/infrastructure/redis.rs` |
| 登录限流与失败记录 | `src/modules/auth/service.rs`（`login`、`count_login_failures`、`record_login_failure`） |
| 登出吊销 access token | `src/modules/auth/service.rs`（`revoke_access_token`）、`src/modules/auth/handler.rs`（`logout`） |
| 认证时查询吊销列表 | `src/common/auth.rs`（`authenticate`） |
| 用户详情缓存与失效 | `src/modules/user/service.rs`（`get_user_by_public_id`、`evict_user_cache`） |
| 状态装配 | `src/app/state.rs`、`src/app/mod.rs`、`src/main.rs` |
| 就绪检查 | `src/app/routes.rs`（`readiness_handler`） |
| 配置结构与校验 | `src/config/mod.rs`（`RedisConfig`） |

### 11.2 配置项

| 配置键 | 环境变量 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `redis.url` | `APP__REDIS__URL` | `redis://127.0.0.1:6379` | 连接地址，支持 `redis://` 与 `rediss://` |
| `redis.pool_size` | `APP__REDIS__POOL_SIZE` | `8` | 连接池上限 |
| `redis.key_prefix` | `APP__REDIS__KEY_PREFIX` | `axum` | 键命名空间，多环境必须不同 |
| `redis.user_cache_ttl_seconds` | `APP__REDIS__USER_CACHE_TTL_SECONDS` | `60` | 用户详情缓存 TTL |

配置在启动时校验：地址必须以 `redis://` 或 `rediss://` 开头，`pool_size` 与 TTL 必须为正数，前缀不能为空。

### 11.3 键空间

| 键模式 | 类型 | TTL | 写入方 | 读取方 |
| --- | --- | --- | --- | --- |
| `{prefix}:auth:login_failures:{username}` | ZSET | 失败窗口（默认 15 分钟，每次写入续期） | `record_login_failure` | `login_failure_count` |
| `{prefix}:auth:revoked_access:{jti}` | STRING | 令牌剩余有效期 | `revoke_access_token`（登出） | `is_access_token_revoked`（中间件） |
| `{prefix}:user:detail:{public_id}` | STRING(JSON) | `user_cache_ttl_seconds` | 详情查询回填 | 详情查询、更新/删除时删除 |

---

## 12. 延伸学习路线

### 12.1 在本项目中继续练习

1. **给列表接口加缓存**：在 `UserService::get_users` 中缓存分页结果，思考「写入时如何失效」「分页参数如何进键名」「为什么本实现刻意不缓存」。
2. **缓存穿透防护**：用户不存在时写入一个短 TTL 的空值标记，并补充测试。
3. **热点键互斥重建**：用 `SET lock_key value NX PX 5000` 实现分布式锁，避免缓存击穿时大量请求同时回源。
4. **限流按 IP 维度扩展**：当前只按用户名计数，增加 `auth:login_failures:ip:{ip}` 维度，并思考两个维度的阈值如何组合。
5. **吊销列表改为布隆过滤器**：思考「误判」在安全场景下是否可接受（答案：吊销场景通常不能误判为已吊销，否则会把合法用户踢下线）。

### 12.2 推荐的学习顺序

1. 数据类型与命令：在 `redis-cli` 中把第 3 节的命令全部敲一遍，重点理解 ZSET 与过期。
2. 原子性与 Lua：写一个「取号 + 限流」的小脚本，理解 `KEYS` / `ARGV` 与脚本缓存。
3. 持久化与高可用：用 Docker 起主从，手动 `REPLICAOF`，再试 Sentinel。
4. 客户端与连接池：阅读 `redis` crate 的 `MultiplexedConnection` 与 `bb8` 的实现细节。
5. 生产问题：慢日志、大 key、内存碎片、Cluster 迁移。

### 12.3 官方资料

- Redis 官方文档：<https://redis.io/docs/>
- 命令速查：<https://redis.io/commands/>
- Redis 源码（数据结构与事件循环）：<https://github.com/redis/redis>
- redis-rs（Rust 客户端）：<https://github.com/redis-rs/redis-rs>

---

## 附：本文档与代码的同步维护

Redis 用法发生变化（新增键空间、调整降级策略、修改 TTL 语义）时，需要同时更新：

- 本文档第 6 节（场景）、第 11 节（对照表）；
- `docs/reference/ARCHITECTURE.md`（技术组件、启动流程、状态与缓存说明）；
- `docs/reference/AUTH_API.md`（登录保护、登出语义、配置表）；
- `config/default.toml` 与 `.env.example`（默认值与环境变量）。

完成修改后运行：

```bash
cargo fmt --check
cargo clippy --all-targets
cargo test
```
