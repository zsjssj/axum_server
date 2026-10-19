# 04 数据库与 SQLx

**本章目标**：搞懂"Rust 后端怎么和 PostgreSQL 打交道"。重点是三件事：连接池为什么这样配、SQL 该怎么写进代码、数据库结构该怎么演进。这一章含金量最高，因为**数据层的错误最难回滚**。

---

## 4.1 连接池：不是"每请求一条连接"

```rust
// src/infrastructure/database.rs:6
pub async fn create_pool(database_url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)          // 上限
        .acquire_timeout(Duration::from_secs(10))  // 等连接最多 10 秒
        .connect(database_url)                     // 立刻建连并校验
        .await?;

    tracing::info!(max_connections, "database pool connected");
    Ok(pool)
}
```

三个参数各有含义：

| 参数 | 作用 | 配错的后果 |
| --- | --- | --- |
| `max_connections` | 池内最多持有多少条连接 | 太大 → 打满 PostgreSQL 的 `max_connections`，其他服务连不上；太小 → 请求排队 |
| `acquire_timeout` | 取连接的等待上限 | 不设 → 数据库挂掉时请求永久挂起，线程/任务全泄漏 |
| `connect()` 而不是 `connect_lazy()` | 启动时真的连一次 | 用 lazy 会"启动成功但第一个请求才炸" |

**`acquire_timeout` 是被严重低估的参数。** 没有它，数据库不可用时请求会无限期排队：连接不报错、请求不返回，监控上表现为"服务还活着但全部超时"，比直接 500 更难排查。

关于容量的经验法则（不是定律）：

```text
池大小 ≈ 数据库允许的连接数 / 实例数，并留 20% 余量给运维和迁移
```

`config/default.toml` 里 `max_connections = 10`，`production.toml` 里是 20——环境不同、实例数不同，所以这一层配置必须按环境覆盖（见[第 03 章](./03-配置系统.md)）。

> **为什么还有个"连接复用"的概念？** 建一条 PostgreSQL 连接要 TCP 握手 + TLS + 认证，约几十毫秒。池让连接长期存活、请求间复用，所以池大小的目标是"覆盖并发峰值"，而不是"每个请求一条"。

## 4.2 迁移：结构演进必须可重现

```rust
// src/infrastructure/database.rs:19
pub async fn run_migrations(pool: &PgPool) -> Result<(), MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await?;
    tracing::info!("database migrations completed");
    Ok(())
}
```

四个要点：

1. **编译期嵌入**：`sqlx::migrate!` 是宏，把 `migrations/*.sql` 打包进二进制。部署产物不依赖运行目录里有 SQL 文件。
2. **自动执行**：进程启动时跑，跑完才监听端口（见[第 02 章](./02-项目全景与启动流程.md)）。
3. **有记录**：SQLx 维护 `_sqlx_migrations` 表，记录每个文件的版本、描述和**校验和**，所以重复启动不会重复执行。
4. **不能改历史文件**：改了已执行的迁移，校验和不匹配 → 下次启动直接报错退出。**结构变更只能新增文件**，这是仓库规范写死的（`AGENTS.md`）。

当前迁移：

| 文件 | 内容 |
| --- | --- |
| `001_create_users_table.sql` | `users` 表 + email/username 索引 |
| `002_create_auth_tables.sql` | `user_sessions`、`login_failures` 两张表 + 索引 |

`002` 文件的注释解释了这个约定：

```sql
-- 单独新增迁移文件而不修改 001，因为已执行的迁移不会重复运行。
```

## 4.3 表结构精读：每个字段都在解决一个问题

### users

```sql
CREATE TABLE IF NOT EXISTS users (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    public_id UUID NOT NULL UNIQUE,
    username VARCHAR(50) NOT NULL UNIQUE,
    email VARCHAR(100) NOT NULL,
    nickname VARCHAR(100),
    password_hash VARCHAR(255) NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW()
);
```

| 设计 | 原因 |
| --- | --- |
| `id BIGINT IDENTITY` | 内部主键：定长、有序、索引友好，用于数据库关联和 JWT 的 `sub` |
| `public_id UUID UNIQUE` | 公开标识：不暴露用户数量、不可枚举，用于 API 路径 |
| `username UNIQUE` | 业务唯一约束，最后一道防线（应用层也会查重，但并发下只有数据库能保证） |
| `password_hash VARCHAR(255)` | Argon2 编码后约 100 字符，255 留足余量 |
| `nickname` 可空 | 可选信息，用 `Option<String>` 表达 |
| `created_at`/`updated_at` `DEFAULT NOW()` | 时间戳由数据库生成，不依赖应用服务器时钟 |
| 两个索引 | `username` 供登录查询，`email` 供将来按邮箱查找/唯一化 |

### 双 ID 设计（重要）

这是本项目最值得借鉴的一个决定：

```text
内部主键 id (BIGINT)         公开标识 public_id (UUIDv7)
  ├─ 数据库关联                ├─ API 路径 /users/{public_id}
  ├─ JWT 的 sub               ├─ 响应体
  └─ 排序                      └─ 跨系统传递
     ↑ 绝不通过 API 暴露
```

为什么不全用 UUID？因为 InnoDB/B-Tree 上随机 UUID 做主键会让插入位置随机化，索引碎片化、写放大。为什么不全用自增 ID？因为它会泄露"你有多少用户"（注册一天看 ID 差值就知道业务量），而且是可枚举的，配合越权漏洞很危险。

**UUIDv7** 是折中方案：前 48 位是毫秒时间戳，所以整体单调递增（对索引友好），后 80 位随机（不可枚举）。代码里就一行：

```rust
// src/modules/user/service.rs:146
fn generate_public_id() -> Uuid {
    Uuid::now_v7()
}
```

放在 Service 生成而不是数据库默认值，是因为 UUID 属于**业务标识**，Service 需要它来决定"这次创建的用户对外叫什么"，而不是等数据库回填。

### user_sessions

```sql
CREATE TABLE IF NOT EXISTS user_sessions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash CHAR(64) NOT NULL UNIQUE,          -- 只存 SHA-256 摘要
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,                       -- 软吊销，不是物理删除
    replaced_by BIGINT REFERENCES user_sessions(id) ON DELETE SET NULL,
    client_ip INET,
    user_agent TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

| 设计 | 原因 |
| --- | --- |
| `token_hash CHAR(64)` | SHA-256 十六进制正好 64 字符；**存摘要不存原文**，库泄露也无法冒用令牌 |
| `revoked_at` 软吊销 | 保留记录才能审计和识别"令牌重放" |
| `replaced_by` 轮换链 | 记录"旧会话被哪个新会话替代"，是重放检测的证据 |
| `ON DELETE CASCADE` | 用户删了会话就该没，交给数据库保证 |
| `INET` 而非 `TEXT` | 数据库校验 IP 格式，非法值写不进去 |
| `TIMESTAMPTZ` 而非 `TIMESTAMP` | 带时区，跨时区部署不会错位（`users` 表用 `TIMESTAMP` 是历史选择） |

索引对应真实查询：

```sql
CREATE INDEX idx_user_sessions_user_id ON user_sessions(user_id);      -- 按用户吊销全部会话
CREATE INDEX idx_user_sessions_expires_at ON user_sessions(expires_at);-- 清理过期会话
```

**索引不是"多加点更保险"。** 每个索引都让写入变慢一份、占用磁盘一份。判断标准永远是：有没有一条真实的高频查询需要它。

### login_failures

```sql
CREATE TABLE IF NOT EXISTS login_failures (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    username TEXT NOT NULL,        -- 注意：故意不设外键
    client_ip INET,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
```

`username` **不设外键**，因为这是未认证输入：攻击者可以填任意用户名，那个用户可能根本不存在。同时注释明确了为什么不学"账号锁定"：

```sql
-- 使用滑动窗口统计而不是在 users 表上加锁定字段：后者会让自动锁定变成
-- 针对特定账号的拒绝服务手段，攻击者只需故意输错密码即可锁死他人账号。
```

这是一个典型的"安全设计反直觉"案例：看起来更安全（锁定账号）的做法，实际上创造了一个更廉价的攻击面。

## 4.4 Repository：SQL 只写在一个地方

Repository 的职责边界很清晰：**执行 SQL、返回实体或 `Option`，不决定状态码、不组织业务流程。**

### 插入：`RETURNING` 一步到位

```rust
// src/modules/user/repository.rs:10
pub async fn create(db: &PgPool, public_id: Uuid, username: &str, email: &str,
                    password_hash: &str, nickname: Option<&str>) -> Result<User, sqlx::Error> {
    let user = sqlx::query_as::<_, User>(
        r#"
        INSERT INTO users (public_id, username, email, password_hash, nickname)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING public_id, username, email, nickname, created_at, updated_at
        "#,
    )
    .bind(public_id).bind(username).bind(email).bind(password_hash).bind(nickname)
    .fetch_one(db)
    .await?;

    Ok(user)
}
```

三个重点：

1. **`RETURNING`** 让"插入 + 读回"变成一次往返。要避免的是 `INSERT` 完再 `SELECT`——多一次网络往返，还可能读到别人刚改的值。
2. **`RETURNING` 的列里没有 `password_hash`**。返回值要映射到 `User`，而 `User` 故意不含密码哈希。
3. **`.bind()` 参数化**，永远不要 `format!` 拼 SQL。参数绑定同时解决 SQL 注入和类型转义两个问题。

### 查询单个：`fetch_optional`

```rust
// src/modules/user/repository.rs:30
let user = sqlx::query_as::<_, User>(
    r#"SELECT public_id, username, email, nickname, created_at, updated_at
       FROM users WHERE public_id = $1"#,
)
.bind(public_id)
.fetch_optional(db)     // 查不到 = Ok(None)，不是错误
.await?;

Ok(user)
```

`fetch_optional` vs `fetch_one` 的区别就是语义的区别：

| 方法 | 查不到时 | 用在哪 |
| --- | --- | --- |
| `fetch_one` | 返回 `Err(RowNotFound)` | 确定必须存在的查询（如 `COUNT`） |
| `fetch_optional` | 返回 `Ok(None)` | 可能不存在的资源查询 |
| `fetch_all` | 返回空 `Vec` | 列表查询 |

把"资源不存在"当成 `Err` 处理会导致 500——它明明是正常的业务分支，应该由 Service 转成 `404`。

### 认证专用查询：类型隔离

```rust
// src/modules/user/repository.rs:68
pub async fn find_credentials_by_username(db: &PgPool, username: &str)
    -> Result<Option<UserCredentials>, sqlx::Error>
{
    let credentials = sqlx::query_as::<_, UserCredentials>(
        r#"SELECT id, public_id, password_hash FROM users WHERE username = $1"#,
    )
    .bind(username)
    .fetch_optional(db)
    .await?;
    Ok(credentials)
}
```

返回 `UserCredentials` 而不是 `User`。这个类型在 `src/modules/user/model.rs:33`，只派生 `sqlx::FromRow`，**没有 `Serialize`、没有 `ToSchema`**。所以密码哈希在类型层面就无法顺着响应序列化或 OpenAPI 文档泄露出去。

### 分页：`LIMIT` + `OFFSET`

```rust
// src/modules/user/repository.rs:84
pub async fn find_all(db: &PgPool, page: i64, page_size: i64) -> Result<Vec<User>, sqlx::Error> {
    let offset = (page - 1) * page_size;

    let users = sqlx::query_as::<_, User>(
        r#"SELECT ... FROM users ORDER BY id DESC LIMIT $1 OFFSET $2"#,
    )
    .bind(page_size)
    .bind(offset)
    .fetch_all(db)
    .await?;
    Ok(users)
}
```

`(page - 1) * page_size` 这类算术在 Rust 里要留意溢出：`page` 是客户端输入，`i64` 上限内足够安全，但**必须在进入 Repository 之前就限制范围**——`Pagination` 的校验规则是 `page >= 1`、`page_size` 1–100（`src/common/pagination.rs:11`）。如果校验漏了，`page = 0` 会算出负 offset，PostgreSQL 直接报错。

> **OFFSET 分页的固有缺陷**：翻到第 10000 页时数据库仍要扫描前 10 万行再丢掉。数据量大时要换成游标分页（`WHERE id < $last_id ORDER BY id DESC LIMIT $n`）。这是一个"知道但不急着改"的问题——当前数据量下 OFFSET 完全够用，过早优化反而增加复杂度。

### 更新：`COALESCE` 实现"局部更新"

```rust
// src/modules/user/repository.rs:110
UPDATE users
SET email = COALESCE($2, email),
    nickname = COALESCE($3, nickname),
    updated_at = NOW()
WHERE public_id = $1
RETURNING public_id, username, email, nickname, created_at, updated_at
```

`COALESCE($2, email)` 的含义：参数是 NULL 就保留原值。这样"只改昵称"和"只改邮箱"共用一个 SQL，不用在 Rust 里拼动态 SQL 字符串。

代价是**无法用 NULL 来"清空"字段**（传 NULL 等于不修改）。当前业务上 `email` 不可空、`nickname` 清空需求也没提，所以这个取舍是合理的——但要在注释或文档里写明，否则将来实现"清空昵称"的人会困惑为什么改不掉。

### 删除：`rows_affected` 判断是否真的删了

```rust
// src/modules/user/repository.rs:131
pub async fn delete(db: &PgPool, public_id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM users WHERE public_id = $1")
        .bind(public_id).execute(db).await?;
    Ok(result.rows_affected() > 0)
}
```

返回 `bool` 而不是 `()`，让 Service 能区分"删掉了"和"本来就不存在"，后者应该返回 404 而不是 200。

## 4.5 事务：一个业务操作的原子边界

```rust
// src/modules/auth/repository.rs:128
pub async fn rotate_session(db: &PgPool, old_id: i64, new_id: i64) -> Result<(), sqlx::Error> {
    let mut transaction = db.begin().await?;

    sqlx::query(
        r#"UPDATE user_sessions
           SET revoked_at = NOW(), replaced_by = $2
           WHERE id = $1 AND revoked_at IS NULL"#,
    )
    .bind(old_id).bind(new_id)
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;
    Ok(())
}
```

要点：

1. **事务跟随调用链传递**：执行时用 `&mut *transaction`（解引用成 executor），而不是传 `db`。传错了会在事务外执行，事务白开。
2. **`commit()` 是显式的**。`Transaction` 被 drop 时默认**回滚**——这是一个安全默认值，忘了 commit 不会意外提交半成品。
3. **事务的边界应该是业务边界**。"吊销旧会话 + 记录轮换关系"必须同时成立，否则会出现"旧令牌已失效但不知道被谁替代"的中间状态。

注意事务放在 **Repository** 而不是 Service：Service 决定"要做哪些数据操作"，Repository 负责"这些操作怎么原子地落库"。如果将来需要跨多个 Repository 的事务，才需要把 `Transaction` 提升到 Service——那是另一种做法（`sqlx` 支持把 `&mut PgConnection` 传进 Repository）。

## 4.6 数据库错误 → 业务错误

数据库错误不能直接吐给客户端（会泄露表名、SQL、约束名），但也不能全部变成 500——有些是**预期内的业务冲突**。

```rust
// src/modules/user/service.rs:29
match UserRepository::create(db, public_id, &username, &email, &password_hash, nickname.as_deref()).await {
    Ok(user) => Ok(user),
    // 唯一键冲突 → 409，这是业务语义，不是系统故障
    Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
        Err(AppError::Conflict("用户名已存在"))
    }
    // 其他数据库错误 → From<sqlx::Error> → AppError::Database → 500
    Err(error) => Err(error.into()),
}
```

这体现了错误处理的两层策略：

| 错误类型 | 处理 | 状态码 |
| --- | --- | --- |
| 唯一键冲突 | `is_unique_violation()` 转 `Conflict` | 409 |
| 外键冲突、非空约束 | 视业务而定，通常也应是 4xx | 4xx |
| 连接失败、语法错误、超时 | `AppError::Database`，细节只进日志 | 500 |

**为什么要靠数据库报错而不是先查一次？** 业务层可以先 `SELECT` 判重给出更友好的提示，但那是"优化体验"，唯一约束才是"保证正确"。并发下两个请求可能同时通过 `SELECT` 检查，只有数据库的唯一索引能拦住它。本项目直接捕获约束冲突，少一次查询且绝对正确。

## 4.7 动手练习

1. **看执行计划**：`EXPLAIN ANALYZE SELECT * FROM users WHERE username = 'admin';` 确认走了 `idx_users_username`。然后临时删掉索引再跑一次对比。
2. **新增一个迁移**：`003_create_user_profiles.sql`，建一张 `user_profiles`（`user_id` 外键 + `bio TEXT` + `avatar_url`），启动服务确认 `_sqlx_migrations` 多了一行。**不要再改这个文件**，改内容重启一次，观察校验和报错。
3. **写一个游标分页查询**：在 Repository 加 `find_page_after(db, after_id, limit)`，用 `WHERE id < $1 ORDER BY id DESC LIMIT $2`，和现有 OFFSET 版本对比结果是否一致。
4. **体验类型隔离**：把 `find_credentials_by_username` 的返回类型临时改成 `User`（把 `password_hash` 加到 `User` 上），然后 `cargo build` 看认证模块还缺什么、`/api-docs/openapi.json` 里会不会多出字段。**看完立即还原。**
5. **验证事务回滚**：在 `rotate_session` 的 `commit()` 前加一句 `return Err(sqlx::Error::RowNotFound);`，调用一次刷新接口，然后查库确认 `revoked_at` 没有被写入。

## 4.8 自测题

1. `acquire_timeout` 不设会有什么后果？为什么它比 `max_connections` 更容易被忽略？
2. 为什么内部 `id` 和 `public_id` 要分开？只用 UUID 或用自增 ID 各自的代价是什么？
3. `token_hash` 存摘要而不是原文，防的是哪种泄露场景？
4. `fetch_one` 与 `fetch_optional` 用错会导致什么状态码错误？
5. `COALESCE($2, email)` 这个写法有什么无法表达的需求？
6. 为什么"用户名重复"要靠数据库唯一约束而不是应用层 `SELECT` 判重？
7. 已执行的迁移文件为什么不能修改？系统靠什么发现文件被改过？

---

下一步 → [05 请求生命周期](./05-请求生命周期.md)
