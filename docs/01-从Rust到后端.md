# 01 从 Rust 到后端：先把心智模型摆正

**本章目标**：把 Rust 语言特性翻译成后端工程里的实际问题。读完你应该能回答："为什么这里的错误不用 `throw`？""为什么密码哈希要单独扔进 `spawn_blocking`？""为什么一个结构体只要不派生 `Serialize` 就安全了？"

**前置**：会基础的 Rust 语法（`struct`、`enum`、`match`、`Result`、trait 的写法）。不需要写过后端。

---

## 1.1 后端的本质，和语言无关

先把语言放一边。任何后端服务的骨架都是这五件事：

```text
1. 收请求          解析 HTTP：方法、路径、头、体
2. 定身份          这个请求是谁发的？他能不能做这件事？
3. 走业务          校验参数、执行规则、协调多个数据操作
4. 落数据          读写数据库，保证一致性
5. 回响应          成功与失败都要有稳定、可预期的形状
```

不同语言的差别只在"这五件事默认由谁兜底"：

| | Java/Spring | Go | Node/TS | **Rust/Axum（本项目）** |
| --- | --- | --- | --- | --- |
| 出错怎么传 | 异常，可跨层抛出 | `error` 返回值 | 异常 / Promise reject | **`Result<T, E>` 返回值，编译器强制处理** |
| 空值 | `null` | 零值 / `nil` | `undefined` / `null` | **`Option<T>`，类型系统区分** |
| 并发模型 | 线程池 | goroutine | 单线程事件循环 | **async 任务 + 多线程执行器** |
| 谁来保证"共享数据不出错" | 运行时 + 开发者约定 | 开发者约定 | 单线程 + 开发者约定 | **所有权与借用检查器（编译期）** |
| 内存管理 | GC | GC | GC | **确定性析构（`Drop`），无 GC** |
| 编译期能拦住多少 | 类型 | 类型 | 类型 | **类型 + 生命周期 + 可变性 + `Send`/`Sync`** |

这张表解释了一个初学者的困惑：**Go/Java 里"顺手就能写"的东西，在 Rust 里经常编译不过**。不是你写错了，是编译器把你以前靠 code review 才能发现的问题提前了。

代价也要认识清楚：Rust 的编译期保证只覆盖"不内存不安全、不数据竞争"，**不覆盖业务正确性**。所以本项目的重点经常不在"怎么写能编译过"，而在"怎么让顺序、边界和降级策略是对的"——那是第 06、09、10 章的主题。

## 1.2 心智模型一：错误是值，不是控制流

Rust 没有异常。错误是普通返回值，用 `?` 提前返回。

```rust
// src/common/error.rs:38  ——  让 ? 能用的关键
pub enum AppError {
    // ...
    #[error("数据库操作失败")]
    Database(#[from] sqlx::Error),
}
```

`#[from]` 由 `thiserror` 展开成 `impl From<sqlx::Error> for AppError`。意义：任何返回 `Result<T, sqlx::Error>` 的函数，在本项目里都可以直接 `.await?`，因为 `sqlx::Error` 能自动转换成 `AppError`。于是在 Repository、Service、Handler 里，`?` 一路把错误往上抛，最后在**一个地方**统一变成 HTTP 响应（`impl IntoResponse for AppError`，`src/common/error.rs:83`）。

对照一下心智差别：

| 做法 | 结果 |
| --- | --- |
| Java 抛异常 | 任何一层都能中断，调用方可能完全没有意识到 |
| Go 返回 `error` | 每层都要 `if err != nil`，容易漏写 |
| **Rust `Result` + `?`** | 每层都必须显式选择"处理"或"上抛"，编译期强制 |

本项目把这条原则落成了三层结构：

```rust
// src/modules/user/handler.rs:53
pub(crate) async fn create_user(
    State(state): State<AppState>,
    _current_user: CurrentUser,
    ValidatedJson(req): ValidatedJson<CreateUserRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user = UserService::create_user(&state.db, req).await?;   // 错误自动上抛
    Ok((StatusCode::CREATED, Json(ApiResponse::success("用户创建成功", user))))
}
```

Handler 里没有任何错误分支——因为它不该在这里决定"用户名重复返回什么"。业务语义错误在 Service 里转换（`Conflict("用户名已存在")`），系统错误交给 `AppError::Database`，两者都由 `IntoResponse` 收口。

> **自测**：如果把 `Database` 变体上的 `#[from]` 去掉，会有几处编译失败？先自己数，再用 `cargo build` 验证。

## 1.3 心智模型二：async 是"可暂停的普通函数"

`async fn` 返回的是 `Future`，**不调用 `.await` 就一行都不执行**。这是 Rust 和其他语言异步模型最大的认知差异。

```rust
// src/main.rs:16
#[tokio::main(worker_threads = 4)]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("server error: {error:?}");
        std::process::exit(1);
    }
}
```

`error` 的类型是 `anyhow::Error`，所以这里用 `{:?}` 而不是 `{}`：`Debug` 会把 `Caused by` 逐层展开成完整错误链，`Display` 只剩最外层一句（见[第 07 章](./07-统一响应与错误处理.md) 7.4 节）。

`#[tokio::main]` 是一个宏，把 `main` 展开成：建运行时 → `block_on(run())`。`worker_threads = 4` 表示固定 4 个工作线程；一个 `Future` 被 `.await` 期间，线程可以去跑别的任务。

由此推出后端里最重要的一条纪律：

> **永远不要在执行器线程上做 CPU 密集或同步阻塞的事。**

因为工作线程只有 4 个，一个 Argon2 密码哈希要跑几十毫秒，一旦直接放在 async 函数里调用，这 4 个线程会被逐个占满，**所有**并发请求都卡住——不是变慢，是整体停摆。

本项目的两个正确写法：

```rust
// src/modules/user/service.rs:25  用户创建时算哈希
let password_hash = tokio::task::spawn_blocking(move || hash_password(&password))
    .await
    .map_err(|_| AppError::Internal("密码哈希任务失败"))??;
```

```rust
// src/modules/auth/service.rs:504  登录时校验密码
async fn verify_password(password: &str, encoded_hash: &str) -> Result<bool, AppError> {
    let password = password.to_owned();          // 必须 move 进去：'static 约束
    let encoded_hash = encoded_hash.to_owned();

    tokio::task::spawn_blocking(move || verify_password_blocking(&password, &encoded_hash))
        .await
        .map_err(|_| AppError::Internal("密码校验任务失败"))?
}
```

注意三个细节，都是新手容易踩的：

1. **`.to_owned()` 不是多余**。`spawn_blocking` 要求闭包 `'static`，不能借用外部引用——这正是所有权在异步代码里的实际体现。
2. **两个 `?` 不能省成一处**。第一个 `?` 处理"任务本身失败"（panic / 运行时关闭），第二个处理"哈希校验出错"。两者是不同的失败源。
3. **数据库和网络操作用异步接口就够了**，不要也塞进 `spawn_blocking`。SQLx 的 `.await` 内部已经是非阻塞的，再包一层反而浪费线程。

## 1.4 心智模型三：用类型表达状态，而不是用注释

Rust 的 `enum` 可以带数据，这让"用类型让非法状态无法表示"变得便宜。本项目的三个典型例子：

```rust
// src/common/error.rs:38  一个枚举表达全部错误类别
pub enum AppError {
    Validation(ValidationErrors),      // 带数据
    BadRequest(&'static str),          // 带数据
    MethodNotAllowed,                  // 不带数据
    Database(sqlx::Error),             // 包住底层错误
    // ...共 12 个变体
}
```

```rust
// src/modules/auth/model.rs:27  刻意"不能序列化"的类型
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
}
```

`TokenPair` 没有派生 `Serialize`。这意味着**任何**把它放进 JSON 响应的代码都编译不过。令牌只允许通过 `Set-Cookie` 头下发，这条安全约束不靠 code review，靠类型系统。同样的手法用在 `UserCredentials`（`src/modules/user/model.rs:33`）上：它装着 `password_hash`，但因为没有 `Serialize` 和 `ToSchema`，密码哈希没有进入响应或接口文档的路径。

```rust
// src/modules/user/model.rs:13  一个类型同时承担"数据库行"和"缓存值"
#[derive(Debug, Serialize, Deserialize, FromRow, ToSchema)]
pub struct User { /* ... */ }
```

这里反过来用：**故意**让它既能序列化又能反序列化，因为它既是 API 响应模型，也是 Redis 缓存值的格式。两端共用同一个类型，就不会出现"缓存结构体"和"响应模型"各自演进、最后返回字段不一致的问题。

> **设计直觉**：派生哪些 trait，就是给这个类型开哪些"出口"。每开一个出口问自己一句：**数据从这里出去会不会泄露不该出去的东西？**

## 1.5 心智模型四：模块树就是架构图

Rust 的模块系统不是"文件夹分组"，它直接控制可见性。本项目的 `src/` 树：

```text
src/
├── main.rs              入口（crate root）
├── app/                 装配层：启动、路由、状态、中间件、文档
├── common/              跨模块复用：错误、响应、提取器、认证、Cookie、分页、密码
├── config/              配置结构与校验
├── infrastructure/      外部依赖适配：PostgreSQL、Redis
└── modules/             业务模块
    ├── auth/            mod.rs / handler / service / repository / model
    ├── information/     mod.rs / handler / service / repository / model（聊天消息）
    └── user/            mod.rs / handler / service / repository / model
```

可见性约定（读代码时非常有用）：

| 写法 | 含义 | 本项目中的例子 |
| --- | --- | --- |
| `pub` | 对 crate 外可见 | `pub struct AppState` |
| `pub(crate)` | 只在 crate 内可见 | Handler 函数全部是 `pub(crate)`，它们只给路由用 |
| 无修饰 | 仅当前模块及子模块可见 | `fn validation_details(...)`、`module` 内的辅助函数 |
| `pub use` | 重导出，简化外部路径 | `src/modules/user/mod.rs` 的 `pub use handler::routes;` |

`mod.rs` 在这里的作用就是"暴露模块的对外接口"：

```rust
// src/modules/user/mod.rs
pub mod handler;
pub mod model;
pub mod repository;
pub mod service;

pub use handler::routes;    // 外部只关心"怎么挂路由"
```

于是 `src/app/routes.rs:37` 可以写 `user::routes()`，而不需要知道 `handler` 内部长什么样。

## 1.6 心智模型五：`Clone` 不是复制业务数据，而是共享句柄

`AppState` 会被 Axum 为每个请求克隆一次。听起来很贵，其实不是：

```rust
// src/app/state.rs:17
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,          // 内部是 Arc<Pool>，克隆=加个引用计数
    pub redis: RedisStore,   // 内部是 bb8 连接池句柄，克隆成本同上
    pub jwt: Arc<JwtConfig>, // 显式 Arc
    pub auth: Arc<AuthConfig>,
}
```

判断"能不能便宜克隆"的标准是：**这个类型克隆时是否复制了大量数据**。连接池、`Arc`、`String` 的引用都便宜；`Vec<u8>` 的大缓冲区不便宜。本项目所有进 `AppState` 的东西都是便宜克隆，所以每请求克隆一次没问题。

## 1.7 本章小结

| 概念 | 在后端里的实际含义 | 本项目落点 |
| --- | --- | --- |
| `Result` | 错误必须显式处理或上抛 | `AppError` + `From` 实现 + `?` |
| `Future` 惰性 | 不 `.await` 就不执行；阻塞会拖死执行器 | `spawn_blocking` 处理密码哈希 |
| `enum` 带数据 | 用类型表达"有哪些失败/状态" | `AppError`、`ApiCode` |
| trait 派生 = 开出口 | 派生 `Serialize` 等于允许它进 JSON | `TokenPair`、`UserCredentials` 不派生 |
| 模块可见性 | 模块树就是架构边界 | `pub(crate)` Handler、`pub use routes` |
| `Clone` 语义 | 便宜克隆靠共享句柄，不靠复制数据 | `AppState` 每请求克隆 |

## 1.8 动手练习

1. **观察惰性**：新建 `examples/lazy.rs`，写一个 `async fn`，里面 `println!` 一句，只在 `main` 里调用而不 `.await`，观察什么都不打印；再 `.await` 一次。
2. **体会阻塞的代价**：把 `src/modules/user/service.rs:25` 的 `spawn_blocking` 临时改成直接调用 `hash_password(&password)?`。写一个 20 并发的压测（`for` 循环 + `tokio::spawn` 调创建用户接口），对比响应时间。**改完记得还原。**
3. **验证类型防线**：给 `TokenPair` 临时加 `#[derive(serde::Serialize)]`，然后在 `handler.rs` 里把 body 改成 `Json(tokens)`，看编译能不能过——它能过，但说明你刚刚给自己开了一条泄露通道。删掉。
4. **读一遍** `src/common/error.rs` 全文，不看注释，自己说出每个变体对应哪个 HTTP 状态码，再和文件里的 `IntoResponse` 对照。

## 1.9 自测题

1. 为什么 `spawn_blocking` 的闭包必须 `move`？不 `move` 会报什么错？
2. `Result<T, AppError>` 和"抛异常"相比，多付出的成本是什么？换来了什么？
3. 如果给 `UserCredentials` 加上 `Serialize`，会有什么后果？为什么"只加在内部类型上"也不行？
4. `pub(crate) async fn create_user(...)` 为什么不需要 `pub`？如果把模块整体改成 `pub mod`，影响是什么？
5. `AppState` 里为什么用 `Arc<JwtConfig>` 而不是直接放 `JwtConfig`？

---

下一步 → [02 项目全景与启动流程](./02-项目全景与启动流程.md)
