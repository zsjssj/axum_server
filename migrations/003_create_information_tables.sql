-- 聊天消息模块（information）相关表：会话、会话成员与消息。
-- 单独新增迁移文件而不修改 001/002，因为已执行的迁移不会重复运行。

-- 会话表。
--
-- kind 区分单聊（single）与会话群聊（group）。两种形态记录在同一个表里，
-- 因为它们共享「成员 + 消息 + 已读位置」这一整套结构，拆成两张表会让
-- 消息查询与权限校验各写两遍，且后续新增形态（如系统通知会话）需要再改一遍。
--
-- direct_key 只对单聊有值，取值是「较小的用户主键:较大的用户主键」。
-- 唯一索引让同一对用户永远只有一个单聊会话：即使两个客户端同时点开
-- 「和他聊天」，也只会有一条记录，而不是两条各自只有一半历史消息的会话。
--
-- message_seq 是会话内的消息序号计数器：发送消息时在该行上自增，
-- 既作为对外分页游标，也顺带把同一会话的写入串行化（见 004 之后的 repository 注释）。
CREATE TABLE IF NOT EXISTS conversations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    public_id UUID NOT NULL UNIQUE,
    kind VARCHAR(16) NOT NULL CHECK (kind IN ('single', 'group')),
    title VARCHAR(100),
    -- 这里刻意不存 owner_id：「谁是群主」只在 conversation_members.role 里记录。
    -- 两处都存就必须在每一次转让时同时更新，而漏更新一处的结果是「成员表说 A 是群主、
    -- 会话表说 B 是群主」——权限判断与详情展示各读一边，行为会自相矛盾。
    direct_key VARCHAR(80) UNIQUE,
    message_seq BIGINT NOT NULL DEFAULT 0,
    last_message_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- 单聊必须有 direct_key、群聊必须没有：两种形态混用同一个唯一索引会
    -- 让「群聊也去查 direct_key」，或在群聊之间产生莫名其妙的唯一冲突。
    CONSTRAINT conversations_direct_key_matches_kind
        CHECK ((kind = 'single') = (direct_key IS NOT NULL))
);

-- 会话列表按最后活跃时间倒序，NULL 表示还没有任何消息。
CREATE INDEX IF NOT EXISTS idx_conversations_last_message_at
    ON conversations(last_message_at DESC NULLS LAST);

-- 会话成员表。
--
-- 复合主键 (conversation_id, user_id) 同时承担两件事：保证「同一会话里一个用户
-- 只有一条成员记录」，以及支撑「按会话列出成员」。所有权限判断都只查这张表——
-- 归属是数据访问层的一部分，不是在业务层补一个 if。
CREATE TABLE IF NOT EXISTS conversation_members (
    conversation_id BIGINT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role VARCHAR(16) NOT NULL DEFAULT 'member' CHECK (role IN ('owner', 'member')),
    -- 已读位置存会话内序号而不是消息主键：撤回不会改变序号，
    -- 已读位置因此不会因为某条消息被撤回或清理而失效。
    last_read_seq BIGINT NOT NULL DEFAULT 0,
    joined_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (conversation_id, user_id)
);

-- 「我参与了哪些会话」按 user_id 反查。
CREATE INDEX IF NOT EXISTS idx_conversation_members_user
    ON conversation_members(user_id);

-- 消息表。
CREATE TABLE IF NOT EXISTS messages (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    public_id UUID NOT NULL UNIQUE,
    conversation_id BIGINT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    -- 会话内序号：对外的分页游标。
    -- 不复用自增主键作游标，是因为全局自增值会泄露「全站一共发了多少条消息」，
    -- 而会话内序号只暴露该会话自身的规模。
    seq BIGINT NOT NULL,
    -- 发送者。ON DELETE SET NULL 保留消息本身：一个人注销不应让群聊出现黑洞。
    sender_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    content_type VARCHAR(16) NOT NULL DEFAULT 'text'
        CHECK (content_type IN ('text', 'image', 'file')),
    content TEXT NOT NULL DEFAULT '',
    -- 客户端生成的幂等键。弱网重试时同一个键只会落一条消息。
    client_message_id VARCHAR(64),
    reply_to_id BIGINT REFERENCES messages(id) ON DELETE SET NULL,
    -- 撤回用「标记 + 清空正文」表达，而不是物理删除：
    -- 物理删除会让历史消息的序号出现不可解释的空洞，也无法告诉客户端
    -- 「这里原本有一条被撤回的消息」。
    recalled_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- 同一会话内序号唯一：序号既是排序依据又是分页游标，
    -- 出现重复会让「翻到某条之后」的分页语义变得不确定。
    UNIQUE (conversation_id, seq)
);

-- 拉取历史消息：WHERE conversation_id = $1 AND seq < $2 ORDER BY seq DESC LIMIT n。
-- 索引列顺序与查询条件完全对齐，避免额外排序。
CREATE INDEX IF NOT EXISTS idx_messages_conversation_seq
    ON messages(conversation_id, seq DESC);

-- 客户端幂等键：同一会话、同一发送者、同一幂等键只允许一条消息。
-- 用部分唯一索引而不是把 NULL 也算进去，是因为大部分请求不会带幂等键。
CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_client_idempotency
    ON messages(conversation_id, sender_id, client_message_id)
    WHERE client_message_id IS NOT NULL;
