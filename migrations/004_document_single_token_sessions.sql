-- 单令牌方案下的会话表说明。
--
-- 结构不变，只补充注释：本项目已下线刷新令牌，user_sessions 从「认证状态」
-- 降级为「登录审计」。token_hash 现在保存登录令牌（JWT）的 SHA-256 摘要，
-- 仅在登出时用于定位记录；replaced_by 属于已下线的轮换机制，保留历史数据。
--
-- 注意：不修改 002，因为它可能已经在其他环境执行过，sqlx 不会再运行它。
COMMENT ON TABLE user_sessions IS '登录会话审计记录：单令牌方案下不参与每次请求的认证';
COMMENT ON COLUMN user_sessions.token_hash IS '登录令牌的 SHA-256 摘要，仅用于登出时定位会话记录';
COMMENT ON COLUMN user_sessions.expires_at IS '登录令牌的过期时间，过期后该记录只剩审计价值';
COMMENT ON COLUMN user_sessions.replaced_by IS '已废弃：原刷新令牌轮换链，仅为保留历史数据而存在';
