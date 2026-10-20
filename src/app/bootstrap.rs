//! 启动期初始化：数据库还没有任何用户时创建首个可登录账号。
//!
//! 这一步不是"开发便利"，而是消除一个死锁：`/api/v1` 下的接口默认要求登录，
//! 而创建用户的 `POST /api/v1/users` 自身也在该路由组内，且没有匿名注册入口。
//! 空库时「要登录得先有用户、要建用户得先登录」无法自解，服务虽然正常启动，
//! 但每个业务接口都只能返回 `401`。
//!
//! 边界约束（三者缺一都会把一次性出口变成长期后门）：
//! - 必须显式开启配置开关，默认关闭；
//! - 只在 `users` 表为空时创建，已有账号的库永远不会被写入；
//! - 账号规则全部复用创建用户接口的校验与哈希逻辑，不另开一套弱约束。
//!
//! 放在 `app` 层而不是模块层：它只做启动期装配，业务规则（字段校验、Argon2
//! 哈希、唯一键冲突处理）全部由 `UserService` 承担，依赖方向仍然是`app → modules`。

use crate::common::error::AppError;
use crate::config::BootstrapConfig;
use crate::modules::user::model::CreateUserRequest;
use crate::modules::user::repository::UserRepository;
use crate::modules::user::service::UserService;
use sqlx::PgPool;
use validator::Validate;

/// 按配置创建引导账号；未开启开关或已存在用户时直接返回成功。
///
/// 返回 `Result` 而不是静默吞掉错误：引导账号创建失败意味着部署后依然无人能登录，
/// 此时让启动失败并打印原因，比让服务"看起来正常但全接口 401"更容易定位。
pub async fn ensure_bootstrap_user(db: &PgPool, config: &BootstrapConfig) -> Result<(), AppError> {
    if !config.enabled {
        return Ok(());
    }
    // 以"表为空"作为唯一触发条件，而不是"该用户名不存在"：
    // 后者会让开关在每次启动时都尝试写库，等于留下一条绕过认证的建号通道。
    let existing = UserRepository::count(db).await?;
    if existing > 0 {
        tracing::info!(existing, "用户表非空，跳过引导账号创建");
        return Ok(());
    }
    let request = CreateUserRequest {
        username: config.username.clone(),
        email: config.email.clone(),
        password: config.password.clone(),
        nickname: config.nickname.clone(),
    };
    // 复用创建用户接口的校验规则，避免引导账号绕过用户名长度、邮箱格式和密码长度约束。
    // 失败详情只进日志：其中包含字段取值，不应出现在启动错误输出之外的地方。
    request.validate().map_err(|errors| {
        tracing::error!(?errors, "引导账号配置不合法，未能创建");
        AppError::Internal("引导账号配置不合法")
    })?;
    match UserService::create_user(db, request).await {
        Ok(user) => {
            tracing::warn!(
                username = %user.username,
                "已创建引导账号，请登录后尽快修改密码并关闭 bootstrap 开关"
            );
            Ok(())
        }
        // 多实例同时启动时，两个实例可能都看到空表。唯一键冲突说明另一个实例
        // 已经写好了账号，目标状态（存在可登录账号）已经达成，因此不算启动失败。
        Err(AppError::Conflict(_)) => {
            tracing::warn!("引导账号已由其他实例创建，跳过");
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BootstrapConfig;

    fn disabled_config() -> BootstrapConfig {
        BootstrapConfig {
            enabled: false,
            username: String::new(),
            email: String::new(),
            password: String::new(),
            nickname: None,
        }
    }

    /// 开关关闭时不应触碰数据库：因此这里可以传一个永远不会连上的连接池，
    /// 只要能返回 `Ok` 就证明该分支在访问数据库之前就退出了。
    #[tokio::test]
    async fn skips_creation_when_disabled() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://user:password@localhost/test")
            .expect("测试数据库连接字符串应合法");

        assert!(
            ensure_bootstrap_user(&pool, &disabled_config()).await.is_ok(),
            "开关关闭时应直接跳过，不得访问数据库"
        );
    }

    /// 关闭状态下即使账号字段残缺也不应报错：这些配置在未启用时没有任何含义。
    #[test]
    fn disabled_config_needs_no_credentials() {
        assert!(!disabled_config().enabled, "默认构造的引导配置必须是关闭状态");
    }
}
