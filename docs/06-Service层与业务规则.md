# 06 Service 层与业务规则

**本章目标**：搞清楚 Service 到底该放什么。很多项目的"Service"只是一个薄薄的转发层，最后业务规则散落在 Handler 里；本章要说明**哪些判断必须放在 Service，以及为什么**。

---

## 6.1 Service 存在的三个理由

| 理由 | 具体表现 |
| --- | --- |
| 复用 | 创建用户的规则既要给 `POST /api/v1/users` 用，也要给启动期引导账号用（`src/app/bootstrap.rs:56`） |
| 隔离 HTTP | Service 返回 `Result<User, AppError>`，不认识 `StatusCode`、`Json`、`IntoResponse` |
| 可测 | 密码哈希、错误归类、缓存失效这些逻辑可以单独写单元测试，不需要起 HTTP |

判断"某段代码该放 Handler 还是 Service"的一个实用问题：

> **换个入口（gRPC、定时任务、CLI）还成立吗？** 成立 → Service；只跟 HTTP 有关 → Handler。

## 6.2 创建用户：一个完整的 Service 方法

```rust
// src/modules/user/service.rs:17
pub async fn create_user(db: &PgPool, req: CreateUserRequest) -> Result<User, AppError> {
    let CreateUserRequest { username, email, password, nickname } = req;   // ① 解构
    let public_id = generate_public_id();                                  // ② 生成公开 ID

    // ③ CPU 密集操作放阻塞线程池
    let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|_| AppError::Internal("密码哈希任务失败"))??;

    // ④ 访问数据 + 归类错误
    match UserRepository::create(db, public_id, &username, &email, &password_hash, nickname.as_deref()).await {
        Ok(user) => Ok(user),
        Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
            Err(AppError::Conflict("用户名已存在"))
        }
        Err(error) => Err(error.into()),
    }
}
```

逐步看：

**① 解构请求类型。** 好处是后面用 `username`、`email` 这些短名字，而不是 `req.username` 到处跑。`CreateUserRequest` 有 `#[serde(deny_unknown_fields)]`，未知字段在反序列化阶段就被拒绝，Service 不需要再关心"多传了字段"。

**② 生成 `public_id`。** 注意这里用了 `pub_id` 生成器而不是数据库默认值——业务标识由业务层决定。

**③ `spawn_blocking` 与两个 `?`。** 详见[第 01 章](./01-从Rust到后端.md)。这里补一句：`move` 闭包拿走了 `password` 的所有权，所以之后不能再用 `password`；如果还需要它，要先 `clone()`。**这是所有权在真实代码里最常见的"突兀感"来源**——不是语法难，而是你必须想清楚"这个值之后还要不要用"。

**④ 错误归类。** 三层处理：

```text
唯一键冲突 → Conflict(409)          "用户名已存在"（业务语义，客户端可处理）
其他 DB 错误 → From<sqlx::Error> → Database → 500（系统故障，客户端不该看到细节）
```

`Err(error) => Err(error.into())` 里的 `.into()` 调用的就是 `From<sqlx::Error> for AppError`。这就是第 01 章说的"`From` 实现让 `?` 能用"——即使不写 `?`，显式 `.into()` 也走同一条路。

## 6.3 读路径与缓存：Cache-Aside

```rust
// src/modules/user/service.rs:66
pub async fn get_user_by_public_id(db: &PgPool, redis: &RedisStore, public_id: Uuid)
    -> Result<User, AppError>
{
    let key = redis.user_detail_key(public_id);

    match redis.get_json::<User>(&key).await {
        Ok(Some(user)) => {
            tracing::debug!(key, "用户详情命中缓存");
            return Ok(user);                                  // 命中：直接返回
        }
        Ok(None) => {}                                        // 未命中：往下走
        Err(error) => tracing::warn!(%error, "读取用户缓存失败，直接查询数据库"),  // 缓存故障：降级
    }

    let user = UserRepository::find_by_public_id(db, public_id)
        .await?
        .ok_or(AppError::NotFound("用户不存在"))?;            // 查不到 → 404

    // 回填失败只告警
    if let Err(error) = redis.set_json(&key, &user, redis.user_cache_ttl()).await {
        tracing::warn!(%error, "回填用户缓存失败");
    }

    Ok(user)
}
```

这段代码把"三个分支"写得很清楚，值得记住这个模式：

| 分支 | 处理 | 为什么 |
| --- | --- | --- |
| 命中 | 直接返回 | 缓存的意义 |
| 未命中（Redis 正常） | 查库 + 回填 | Cache-Aside 的标准流程 |
| Redis 出错 | **继续查库** | 缓存是加速层，不是正确性前提 |

注意**第三种情况的处理方式**：不是 `return Err(...)`，而是 `warn!` 后继续。这个"降级点"必须在代码里显式写出来——它是设计决策，不是随手忽略错误。

`.ok_or(AppError::NotFound("用户不存在"))?` 这一行把 `Option` 变成 `Result`。放在这里而不是 Repository，因为"查不到该返回什么"是业务语义。同样，回填失败**不能**让请求变成 500，因为这次响应已经有正确数据了。

缓存的完整策略（穿透/击穿/雪崩、为什么删除而不是更新）在[第 10 章](./10-Redis与降级设计.md)。

## 6.4 写路径：先写库，再删缓存

```rust
// src/modules/user/service.rs:98
pub async fn update_user(db: &PgPool, redis: &RedisStore, public_id: Uuid, req: UpdateUserRequest)
    -> Result<User, AppError>
{
    let user = UserRepository::update(db, public_id, req.email.as_deref(), req.nickname.as_deref())
        .await?
        .ok_or(AppError::NotFound("用户不存在"))?;

    // 先写数据库、再删缓存
    evict_user_cache(redis, public_id).await;

    Ok(user)
}
```

顺序为什么不能反？因为存在这样的交错：

```text
❌ 错误顺序：先删缓存，再写库
  T1: 删缓存
  T2: 读请求 → 未命中 → 读旧值 → 回填旧值到缓存
  T1: 写库（新值）
  结果：缓存里是旧值，直到 TTL 到期才自愈

✅ 正确顺序：先写库，再删缓存
  T1: 写库（新值）
  T1: 删缓存
  T2: 读请求 → 未命中 → 读新值 → 回填新值
  结果：一致
```

严格地说，"先写库再删缓存"仍有一个极小的窗口（读请求在写库前读到旧值、在删缓存后才回填）。要彻底解决需要延迟双删或版本号，本项目选择接受这个窗口——因为**下次读就会修正**，而且 TTL 兜底。这类"接受一个有界的不一致窗口"的判断，正是一致性设计中最常见的取舍。

还有第二个决定：

```rust
// src/modules/user/service.rs:138
/// 采用「删除」而不是「写入新值」：并发更新时写入会让两次写的先后顺序
/// 决定缓存内容，而删除之后由下一次读请求回填，结果必然与数据库一致。
async fn evict_user_cache(redis: &RedisStore, public_id: Uuid) {
    let key = redis.user_detail_key(public_id);

    if let Err(error) = redis.delete(&key).await {
        tracing::warn!(%error, "删除用户缓存失败，陈旧数据将在 TTL 到期后自动失效");
    }
}
```

**删缓存 vs 更新缓存**：更新缓存时，两个并发更新谁最后写缓存就决定了缓存内容，而这个顺序和数据库的提交顺序无关 → 可能留下旧值。删除则交给下一次读去回填，结果必然是数据库的最新值。

删除失败同样只告警——TTL 是最后一道兜底。`evict_user_cache` 返回 `()` 而不是 `Result`，是**刻意的**：调用方根本没法处理这个错误（重试？回滚事务？都不合适），返回 `Result` 只会诱导别人写无意义的 `?`。

## 6.5 顺序即正确性：本项目里的顺序约束清单

后端有很多 bug 不是"逻辑写错"，而是"顺序写反"。本项目里有五处关键顺序，值得当成检查表记住：

| 顺序约束 | 位置 | 顺序颠倒的后果 |
| --- | --- | --- |
| **先限流判定，再校验密码** | `src/modules/auth/service.rs:90` | Argon2 很慢，大量失败请求会占满阻塞线程池，限流本身变成算力攻击面 |
| **先写数据库，再删缓存** | `src/modules/user/service.rs:108` | 并发下旧值被重新回填，脏数据活到 TTL 到期 |
| **先清数据库审计，不清 Redis 计数** | `src/modules/auth/service.rs:119` | 清空窗口内计数 = 给攻击者"偶尔成功一次就重置限额"的入口 |
| **先落新会话，再把旧会话标记轮换** | `src/modules/auth/service.rs:225` | 旧会话失效但新会话没写成功 → 用户被踢下线且无法续期 |
| **吊销 refresh 会话在前，写 jti 吊销在后** | `src/modules/auth/handler.rs:238` | 顺序反了影响不大，但两者都必须尽力执行（不能因前者失败跳过后者） |

这类约束有一个共同特征：**单看某一行的代码都是对的**，只有放在流程里才能判断顺序对不对。所以它们必须写进注释（本项目都写了），否则下一个改代码的人会"顺手"调换。

## 6.6 业务错误 vs 系统错误

项目在 Service 层完成的"错误归类"，可以用一张表概括：

| Service 看到的情况 | 转换成 | 客户端看到 |
| --- | --- | --- |
| 唯一键冲突 | `AppError::Conflict` | 409 `用户名已存在` |
| 查不到记录 | `AppError::NotFound` | 404 `用户不存在` |
| 密码不匹配 | `AppError::Unauthorized` | 401 `用户名或密码错误` |
| 触发限流 | `AppError::TooManyRequests` | 429 `登录失败次数过多，请稍后再试` |
| 哈希任务 panic | `AppError::Internal` | 500 `服务器内部错误` |
| 其他 `sqlx::Error` | `AppError::Database` | 500 `服务器内部错误`（细节只进日志） |

判断原则：

> **客户端能不能就这个错误做出不同的正确处理？** 能 → 给具体的 4xx 业务错误；不能 → 500，并且不要泄露内部细节。

注意 `AppError::Internal` 与 `AppError::Database` 的区别：前者用于"程序自身的意外"（任务失败、数据不一致），后者专指数据库。两者对外都是 500，但日志里的信息不同，排障时能快速定位方向。

## 6.7 Service 不该做什么

| 反模式 | 为什么不行 |
| --- | --- |
| 返回 `impl IntoResponse` / `StatusCode` | 把 HTTP 依赖注入业务层，将来接 gRPC/CLI 就得重写 |
| 直接读 `env::var` 或全局配置 | 无法在测试里替换，且让依赖不可见 |
| 自己 `PgPool::connect` | 连接池应作为参数传入，事务边界才能跨方法 |
| 吞掉错误（`let _ = ...`） | 错误消失后只能靠日志猜；本项目所有忽略错误的地方都有注释说明为什么可以忽略 |
| 在循环里逐条查询（N+1） | 数据库往返是主要成本，应合并成一条 `IN` 查询 |

关于最后一条：`get_users` 是两次查询（列表 + 总数），这是合理的，因为它们必须一致地反映"当前页"和"总量"。但如果要查每个用户的头像，就应该用 `WHERE user_id = ANY($1)` 一次查完，而不是循环。

## 6.8 动手练习

1. **加一个业务规则**：让 `update_user` 在"邮箱没变、昵称也没变"时直接返回当前数据、不执行 UPDATE、不发删除缓存。想清楚：这个规则该放 Service 还是 Repository？为什么？
2. **观察删除缓存的必要性**：临时注释掉 `update_user` 里的 `evict_user_cache`，更新一个用户后立刻查询详情，你会看到旧值（TTL 60 秒）。等 60 秒再查，值变新了——这就是 TTL 兜底。**改完还原。**
3. **制造一次 N+1**：在 `get_users` 里对每个用户额外查一次详情（循环 + `await`），用 `tracing` 数一下 SQL 次数，然后改成一条 `IN` 查询。
4. **给引导账号路径加日志**：读 `src/app/bootstrap.rs`，确认它复用了 `UserService::create_user`（`:56`）。想想如果不复用、直接写 SQL 会有什么风险。

## 6.9 自测题

1. 为什么 `create_user` 里 `spawn_blocking` 的闭包要 `move`？如果之后还要用 `password` 怎么办？
2. `Err(error) => Err(error.into())` 里的 `.into()` 依赖哪个 trait 实现？
3. 缓存读失败为什么只 `warn!` 而不返回错误？如果这个缓存是"防重复提交锁"，还能这样处理吗？
4. 为什么更新用户时是"删缓存"而不是"写缓存"？两条并发更新的区别是什么？
5. `evict_user_cache` 为什么返回 `()` 而不是 `Result`？
6. "先限流后校验密码"如果不遵守，攻击者能获得什么好处？
7. `AppError::Internal` 和 `AppError::Database` 对外都是 500，分开的意义是什么？

---

下一步 → [07 统一响应与错误处理](./07-统一响应与错误处理.md)
