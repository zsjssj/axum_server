-- 认证相关表：刷新令牌会话与登录失败计数。
-- 单独新增迁移文件而不修改 001，因为已执行的迁移不会重复运行。

-- 刷新令牌会话表。
-- access token 是无状态的，服务端无法单独吊销，因此登出与令牌轮换都依赖该表状态。
CREATE TABLE IF NOT EXISTS user_sessions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- 只保存 SHA-256 摘要：数据库泄露时无法直接冒用刷新令牌，与密码不存明文同理。
    token_hash CHAR(64) NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    -- 软吊销而非物理删除，保留记录以便审计和识别令牌重放。
    revoked_at TIMESTAMPTZ,
    -- 轮换链：新会话指向被替换的旧会话，便于追踪同一登录会话的延续关系。
    replaced_by BIGINT REFERENCES user_sessions(id) ON DELETE SET NULL,
    client_ip INET,
    user_agent TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 按用户快速吊销全部会话，并支持按用户排查登录设备。
CREATE INDEX IF NOT EXISTS idx_user_sessions_user_id ON user_sessions(user_id);
-- 清理过期会话时按到期时间扫描。
CREATE INDEX IF NOT EXISTS idx_user_sessions_expires_at ON user_sessions(expires_at);

-- 登录失败记录表。
-- 使用滑动窗口统计而不是在 users 表上加锁定字段：后者会让自动锁定变成
-- 针对特定账号的拒绝服务手段，攻击者只需故意输错密码即可锁死他人账号。
CREATE TABLE IF NOT EXISTS login_failures (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    -- 未认证的输入，因此不设外键：用户名可能根本不存在。
    username TEXT NOT NULL,
    client_ip INET,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 按用户名加时间倒序统计窗口内失败次数。
CREATE INDEX IF NOT EXISTS idx_login_failures_lookup
    ON login_failures(username, created_at DESC);
