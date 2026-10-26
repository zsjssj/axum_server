//! 聊天消息模块的数据访问层。
//!
//! 本模块贯穿一条规则：**「我是否属于这个会话」写在 SQL 的 `WHERE` 里，
//! 不在 Service 里比较字段**。
//!
//! 反面写法是先按公开 ID 把会话查出来，再在 Rust 里判断当前用户是否在成员列表中。
//! 那种写法有三个问题：每新增一个查询方法就多一次漏写判断的机会（一次漏写就是
//! 越权漏洞）；数据已经到了应用内存里，日志、panic 与调试输出都可能把它带出去；
//! 数据库本来能用 `(conversation_id, user_id)` 一次命中并直接过滤，却改成在应用层做。
//!
//! 因此本文件的每个读方法都接收 `user_id`，由 SQL 决定「查不到」——查不到统一
//! 由 Service 转成 `404`，而不是 `403`：`403` 会告诉攻击者「这条记录存在，只是
//! 不属于你」，等于顺手提供了一个探测接口。

use crate::modules::information::model::{
    ConversationRow, ConversationSummaryRow, MemberRow, MessageRow, UserRef,
};
use sqlx::PgPool;
use uuid::Uuid;

/// 会话与成员的数据访问。
pub struct ConversationRepository;

impl ConversationRepository {
    /// 创建会话并一次性写入全部成员。
    ///
    /// 会话与成员必须同时成功或同时失败：只写入会话而没有成员，会产生一个
    /// 谁都看不见、也删不掉的空会话，且它已经占用了 `direct_key` 唯一键。
    /// 因此这里自行开启事务，而不是让 Service 分开调用两次。
    ///
    /// `owner_id` 只用于决定成员表里谁的 `role` 是 `owner`，不会写进 `conversations`：
    /// 群主身份只有成员表一处来源，转让时只需要改一行。
    pub async fn create_with_members(
        db: &PgPool,
        public_id: Uuid,
        kind: &str,
        title: Option<&str>,
        owner_id: Option<i64>,
        direct_key: Option<&str>,
        member_ids: &[i64],
    ) -> Result<ConversationRow, sqlx::Error> {
        let mut transaction = db.begin().await?;

        let conversation = sqlx::query_as::<_, ConversationRow>(
            r#"
            INSERT INTO conversations (public_id, kind, title, direct_key)
            VALUES ($1, $2, $3, $4)
            RETURNING id, public_id, kind, title, message_seq, created_at
            "#,
        )
        .bind(public_id)
        .bind(kind)
        .bind(title)
        .bind(direct_key)
        .fetch_one(&mut *transaction)
        .await?;

        // 创建者（群主）本身也写进成员表：后续所有权限判断只看成员表，
        // 不再从 `conversations.owner_id` 反推「我是否属于这个会话」，
        // 避免出现「是群主但不在成员表里」这种自相矛盾的状态。
        for member_id in member_ids {
            let role = if Some(*member_id) == owner_id {
                crate::modules::information::model::ROLE_OWNER
            } else {
                "member"
            };

            sqlx::query(
                r#"
                INSERT INTO conversation_members (conversation_id, user_id, role)
                VALUES ($1, $2, $3)
                ON CONFLICT (conversation_id, user_id) DO NOTHING
                "#,
            )
            .bind(conversation.id)
            .bind(member_id)
            .bind(role)
            .execute(&mut *transaction)
            .await?;
        }

        transaction.commit().await?;

        Ok(conversation)
    }

    /// 按单聊唯一键查询会话。
    ///
    /// 用于「和某人聊天」的幂等语义：同一对用户只会有一个会话。
    pub async fn find_by_direct_key(db: &PgPool, direct_key: &str) -> Result<Option<ConversationRow>, sqlx::Error> {
        let conversation = sqlx::query_as::<_, ConversationRow>(
            r#"
            SELECT id, public_id, kind, title, message_seq, created_at
            FROM conversations
            WHERE direct_key = $1
            "#,
        )
        .bind(direct_key)
        .fetch_optional(db)
        .await?;

        Ok(conversation)
    }

    /// 以成员身份查询会话：不是成员时返回 `None`。
    pub async fn find_for_member(db: &PgPool, public_id: Uuid, user_id: i64) -> Result<Option<ConversationRow>, sqlx::Error> {
        let conversation = sqlx::query_as::<_, ConversationRow>(
            r#"
            SELECT c.id, c.public_id, c.kind, c.title, c.message_seq, c.created_at
            FROM conversations c
            JOIN conversation_members cm
              ON cm.conversation_id = c.id AND cm.user_id = $2
            WHERE c.public_id = $1
            "#,
        )
        .bind(public_id)
        .bind(user_id)
        .fetch_optional(db)
        .await?;

        Ok(conversation)
    }

    /// 列出某个用户参与的会话，附带未读数与最后一条消息。
    ///
    /// 两个 `LEFT JOIN LATERAL` 各解决一个问题：取最后一条消息，以及统计未读数。
    /// 把未读数和最后一条消息写在同一条 SQL 里，是为了避免 N+1——会话列表
    /// 一页 10 条，逐个补查就要多 20 次往返。
    ///
    /// 未读数的定义是「别人发的、序号大于我的已读位置的消息条数」。已撤回的消息
    /// 同样计入：它在会话里仍然占一个位置，客户端也确实要显示一条撤回提示；
    /// 排除它只会让「未读数」和「屏幕上能看到的消息数」对不上。
    pub async fn list_for_user(
        db: &PgPool,
        user_id: i64,
        page_size: i64,
        offset: i64,
    ) -> Result<Vec<ConversationSummaryRow>, sqlx::Error> {
        let conversations = sqlx::query_as::<_, ConversationSummaryRow>(
            r#"
            SELECT c.public_id,
                   c.kind,
                   c.title,
                   (SELECT COUNT(*) FROM conversation_members cm WHERE cm.conversation_id = c.id) AS member_count,
                   me.last_read_seq,
                   c.message_seq,
                   COALESCE(unread.unread_count, 0) AS unread_count,
                   lm.public_id AS last_message_public_id,
                   lm.seq AS last_message_seq,
                   lm.sender_public_id AS last_message_sender_public_id,
                   lm.content_type AS last_message_content_type,
                   lm.content AS last_message_content,
                   lm.recalled AS last_message_recalled,
                   lm.created_at AS last_message_created_at,
                   c.last_message_at,
                   c.created_at
            FROM conversations c
            JOIN conversation_members me
              ON me.conversation_id = c.id AND me.user_id = $1
            LEFT JOIN LATERAL (
                SELECT m.public_id,
                       m.seq,
                       m.content_type,
                       m.content,
                       m.created_at,
                       (m.recalled_at IS NOT NULL) AS recalled,
                       u.public_id AS sender_public_id
                FROM messages m
                LEFT JOIN users u ON u.id = m.sender_id
                WHERE m.conversation_id = c.id
                ORDER BY m.seq DESC
                LIMIT 1
            ) lm ON TRUE
            LEFT JOIN LATERAL (
                SELECT COUNT(*) AS unread_count
                FROM messages m
                WHERE m.conversation_id = c.id
                  AND m.seq > me.last_read_seq
                  AND m.sender_id <> $1
            ) unread ON TRUE
            ORDER BY COALESCE(c.last_message_at, c.created_at) DESC, c.id DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(user_id)
        .bind(page_size)
        .bind(offset)
        .fetch_all(db)
        .await?;

        Ok(conversations)
    }

    /// 统计某个用户参与的会话总数。
    pub async fn count_for_user(db: &PgPool, user_id: i64) -> Result<i64, sqlx::Error> {
        let (count,): (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*)
            FROM conversations c
            JOIN conversation_members me
              ON me.conversation_id = c.id AND me.user_id = $1
            "#,
        )
        .bind(user_id)
        .fetch_one(db)
        .await?;

        Ok(count)
    }

    /// 列出会话成员（群主排在最前，其余按加入时间）。
    pub async fn find_members(db: &PgPool, conversation_id: i64) -> Result<Vec<MemberRow>, sqlx::Error> {
        let members = sqlx::query_as::<_, MemberRow>(
            r#"
            SELECT u.public_id, u.username, u.nickname, cm.role, cm.joined_at
            FROM conversation_members cm
            JOIN users u ON u.id = cm.user_id
            WHERE cm.conversation_id = $1
            ORDER BY (cm.role = 'owner') DESC, cm.joined_at ASC, cm.user_id ASC
            "#,
        )
        .bind(conversation_id)
        .fetch_all(db)
        .await?;

        Ok(members)
    }

    /// 统计会话成员数。
    pub async fn count_members(db: &PgPool, conversation_id: i64) -> Result<i64, sqlx::Error> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM conversation_members WHERE conversation_id = $1")
            .bind(conversation_id)
            .fetch_one(db)
            .await?;

        Ok(count)
    }

    /// 查询我在会话中的角色；不是成员时返回 `None`。
    pub async fn find_member_role(db: &PgPool, conversation_id: i64, user_id: i64) -> Result<Option<String>, sqlx::Error> {
        let role = sqlx::query_scalar::<_, String>(
            r#"
            SELECT role
            FROM conversation_members
            WHERE conversation_id = $1 AND user_id = $2
            "#,
        )
        .bind(conversation_id)
        .bind(user_id)
        .fetch_optional(db)
        .await?;

        Ok(role)
    }

    /// 查询我的已读位置；不是成员时返回 `None`。
    pub async fn find_last_read_seq(db: &PgPool, conversation_id: i64, user_id: i64) -> Result<Option<i64>, sqlx::Error> {
        let last_read_seq = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT last_read_seq
            FROM conversation_members
            WHERE conversation_id = $1 AND user_id = $2
            "#,
        )
        .bind(conversation_id)
        .bind(user_id)
        .fetch_optional(db)
        .await?;

        Ok(last_read_seq)
    }

    /// 把已读位置推进到会话当前最新序号，返回推进后的位置。
    ///
    /// 「读到最新」由一条 SQL 决定，而不是先查 `message_seq` 再 `UPDATE`：
    /// 两步之间到达的新消息会被误标成已读，用户就永远看不到那条消息了。
    /// 写成一条语句时，新的 `message_seq` 来自本语句的快照，最坏情况是
    /// **少**标已读（那条消息仍显示为未读），方向是安全的。
    ///
    /// `GREATEST` 保证已读位置只能前进：并发或重放的请求不会把已读位置往回拨，
    /// 否则「已读」会变成可以重置的状态，未读数也会莫名其妙地复活。
    pub async fn mark_read(db: &PgPool, conversation_id: i64, user_id: i64) -> Result<Option<i64>, sqlx::Error> {
        let last_read_seq = sqlx::query_scalar::<_, i64>(
            r#"
            UPDATE conversation_members cm
            SET last_read_seq = GREATEST(cm.last_read_seq, c.message_seq)
            FROM conversations c
            WHERE c.id = cm.conversation_id
              AND cm.conversation_id = $1
              AND cm.user_id = $2
            RETURNING cm.last_read_seq
            "#,
        )
        .bind(conversation_id)
        .bind(user_id)
        .fetch_optional(db)
        .await?;

        Ok(last_read_seq)
    }

    /// 统计已读位置之后仍有几条别人发的消息。
    pub async fn count_unread(
        db: &PgPool,
        conversation_id: i64,
        user_id: i64,
        last_read_seq: i64,
    ) -> Result<i64, sqlx::Error> {
        let (count,): (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*)
            FROM messages
            WHERE conversation_id = $1
              AND seq > $2
              AND sender_id <> $3
            "#,
        )
        .bind(conversation_id)
        .bind(last_read_seq)
        .bind(user_id)
        .fetch_one(db)
        .await?;

        Ok(count)
    }

    /// 批量添加成员，返回实际新增的人数。
    ///
    /// `ON CONFLICT DO NOTHING` 让重复添加同一个用户变成幂等操作：
    /// 返回的受影响行数才是「真正新加入的人」，调用方据此判断是否全是老成员。
    pub async fn add_members(db: &PgPool, conversation_id: i64, user_ids: &[i64]) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            r#"
            INSERT INTO conversation_members (conversation_id, user_id, role)
            SELECT $1, member_id, 'member'
            FROM unnest($2::bigint[]) AS member_id
            ON CONFLICT (conversation_id, user_id) DO NOTHING
            "#,
        )
        .bind(conversation_id)
        .bind(user_ids)
        .execute(db)
        .await?;

        Ok(result.rows_affected())
    }

    /// 移除一个成员，返回是否真的移除了。
    pub async fn remove_member(db: &PgPool, conversation_id: i64, user_id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM conversation_members WHERE conversation_id = $1 AND user_id = $2")
            .bind(conversation_id)
            .bind(user_id)
            .execute(db)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// 把群主转让给最早加入的剩余成员，返回新的群主内部主键。
    ///
    /// 一个群没有群主会变成「谁都不能加人、谁都不能管」的僵局，所以群主退出时
    /// 必须转让而不是留下空缺。返回 `None` 表示已经没有其他成员，会话应当解散。
    pub async fn promote_earliest_member(db: &PgPool, conversation_id: i64) -> Result<Option<i64>, sqlx::Error> {
        let promoted = sqlx::query_scalar::<_, i64>(
            r#"
            UPDATE conversation_members
            SET role = 'owner'
            WHERE conversation_id = $1
              AND user_id = (
                  SELECT user_id
                  FROM conversation_members
                  WHERE conversation_id = $1
                  ORDER BY joined_at ASC, user_id ASC
                  LIMIT 1
              )
            RETURNING user_id
            "#,
        )
        .bind(conversation_id)
        .fetch_optional(db)
        .await?;

        Ok(promoted)
    }

    /// 删除会话（成员与消息由外键 `ON DELETE CASCADE` 一并清理）。
    pub async fn delete(db: &PgPool, conversation_id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM conversations WHERE id = $1")
            .bind(conversation_id)
            .execute(db)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}

/// 消息的数据访问。
pub struct MessageRepository;

impl MessageRepository {
    /// 发送一条消息。
    ///
    /// 返回 `None` 有两种可能，Service 通过「是否带幂等键」区分：
    ///
    /// - 用户不是会话成员（`target` 为空，`EXISTS` 判定不通过）；
    /// - 同一个 `client_message_id` 已经写入过（重试）。
    ///
    /// 序号分配、会话活跃时间更新与消息插入放在**一条语句**里完成：并发发送时
    /// `UPDATE conversations` 会对该会话行加锁，第二个请求排队等待，因此各自拿到
    /// 不同的 `message_seq`，不会出现两条消息序号相同的竞争结果。
    ///
    /// 代价是同一会话的写入被串行化——但这正是聊天需要的语义：同一会话内
    /// 消息的顺序必须唯一确定，而不同会话之间互不影响。
    ///
    /// 重试路径上 `message_seq` 会留下一个空洞：`NOT EXISTS` 拦住了绝大多数重试，
    /// 只有「两个请求同时判定不存在」的竞态才会走到 `ON CONFLICT`，此时序号已经
    /// 自增过。空洞只影响顺序游标的连续性，不影响任何语义。
    #[allow(clippy::too_many_arguments)]
    pub async fn send(
        db: &PgPool,
        public_id: Uuid,
        sender_id: i64,
        content_type: &str,
        content: &str,
        client_message_id: Option<&str>,
        reply_to_id: Option<i64>,
        conversation_public_id: Uuid,
    ) -> Result<Option<MessageRow>, sqlx::Error> {
        let message = sqlx::query_as::<_, MessageRow>(
            r#"
            WITH target AS (
                SELECT c.id
                FROM conversations c
                WHERE c.public_id = $7
                  AND EXISTS (
                      SELECT 1 FROM conversation_members cm
                      WHERE cm.conversation_id = c.id AND cm.user_id = $2
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM messages m
                      WHERE m.conversation_id = c.id
                        AND m.sender_id = $2
                        AND m.client_message_id = $5::text
                  )
            ),
            bumped AS (
                UPDATE conversations c
                SET message_seq = c.message_seq + 1,
                    last_message_at = NOW(),
                    updated_at = NOW()
                FROM target t
                WHERE c.id = t.id
                RETURNING c.id, c.message_seq
            ),
            inserted AS (
                INSERT INTO messages (
                    public_id, conversation_id, seq, sender_id,
                    content_type, content, client_message_id, reply_to_id, created_at
                )
                SELECT $1, b.id, b.message_seq, $2, $3, $4, $5, $6, NOW()
                FROM bumped b
                ON CONFLICT (conversation_id, sender_id, client_message_id)
                    WHERE client_message_id IS NOT NULL
                    DO NOTHING
                RETURNING id, conversation_id, public_id, seq, sender_id,
                          content_type, content, reply_to_id, recalled_at, created_at
            )
            SELECT i.id, i.conversation_id, i.public_id, i.seq, i.sender_id,
                   u.public_id AS sender_public_id, i.content_type, i.content,
                   r.public_id AS reply_to_public_id, i.recalled_at, i.created_at
            FROM inserted i
            LEFT JOIN users u ON u.id = i.sender_id
            LEFT JOIN messages r ON r.id = i.reply_to_id
            "#,
        )
        .bind(public_id)
        .bind(sender_id)
        .bind(content_type)
        .bind(content)
        .bind(client_message_id)
        .bind(reply_to_id)
        .bind(conversation_public_id)
        .fetch_optional(db)
        .await?;

        Ok(message)
    }

    /// 以成员身份查询单条消息：不是会话成员时返回 `None`。
    pub async fn find_for_member(db: &PgPool, public_id: Uuid, user_id: i64) -> Result<Option<MessageRow>, sqlx::Error> {
        let message = sqlx::query_as::<_, MessageRow>(
            r#"
            SELECT m.id, m.conversation_id, m.public_id, m.seq, m.sender_id,
                   u.public_id AS sender_public_id, m.content_type, m.content,
                   r.public_id AS reply_to_public_id, m.recalled_at, m.created_at
            FROM messages m
            JOIN conversation_members cm
              ON cm.conversation_id = m.conversation_id AND cm.user_id = $2
            LEFT JOIN users u ON u.id = m.sender_id
            LEFT JOIN messages r ON r.id = m.reply_to_id
            WHERE m.public_id = $1
            "#,
        )
        .bind(public_id)
        .bind(user_id)
        .fetch_optional(db)
        .await?;

        Ok(message)
    }

    /// 按内部主键读取单条消息。用于撤回后重新取回最新状态。
    pub async fn find_by_id(db: &PgPool, id: i64) -> Result<Option<MessageRow>, sqlx::Error> {
        let message = sqlx::query_as::<_, MessageRow>(
            r#"
            SELECT m.id, m.conversation_id, m.public_id, m.seq, m.sender_id,
                   u.public_id AS sender_public_id, m.content_type, m.content,
                   r.public_id AS reply_to_public_id, m.recalled_at, m.created_at
            FROM messages m
            LEFT JOIN users u ON u.id = m.sender_id
            LEFT JOIN messages r ON r.id = m.reply_to_id
            WHERE m.id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(db)
        .await?;

        Ok(message)
    }

    /// 查询被引用消息的内部主键，且必须属于指定会话。
    ///
    /// 把会话条件写进 `WHERE` 而不是先查出来再比较：跨会话引用会让消息指向
    /// 当前会话成员看不到的内容，等于开了一个读取他人内容的入口。
    pub async fn find_id_in_conversation(
        db: &PgPool,
        conversation_id: i64,
        public_id: Uuid,
    ) -> Result<Option<i64>, sqlx::Error> {
        let id = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT id
            FROM messages
            WHERE conversation_id = $1 AND public_id = $2
            "#,
        )
        .bind(conversation_id)
        .bind(public_id)
        .fetch_optional(db)
        .await?;

        Ok(id)
    }

    /// 按客户端幂等键查询我已发送过的消息。
    pub async fn find_by_client_message_id(
        db: &PgPool,
        conversation_public_id: Uuid,
        user_id: i64,
        client_message_id: &str,
    ) -> Result<Option<MessageRow>, sqlx::Error> {
        let message = sqlx::query_as::<_, MessageRow>(
            r#"
            SELECT m.id, m.conversation_id, m.public_id, m.seq, m.sender_id,
                   u.public_id AS sender_public_id, m.content_type, m.content,
                   r.public_id AS reply_to_public_id, m.recalled_at, m.created_at
            FROM messages m
            JOIN conversations c ON c.id = m.conversation_id
            LEFT JOIN users u ON u.id = m.sender_id
            LEFT JOIN messages r ON r.id = m.reply_to_id
            WHERE c.public_id = $1
              AND m.sender_id = $2
              AND m.client_message_id = $3
            "#,
        )
        .bind(conversation_public_id)
        .bind(user_id)
        .bind(client_message_id)
        .fetch_optional(db)
        .await?;

        Ok(message)
    }

    /// 按序号倒序拉取历史消息。
    ///
    /// `before_seq` 为空表示从最新一条开始；`limit` 由调用方传「期望条数 + 1」，
    /// 多取的那一条只用来判断是否还有更早的消息，避免额外一次 `COUNT`。
    pub async fn list_page(
        db: &PgPool,
        conversation_public_id: Uuid,
        user_id: i64,
        before_seq: Option<i64>,
        limit: i64,
    ) -> Result<Vec<MessageRow>, sqlx::Error> {
        let messages = sqlx::query_as::<_, MessageRow>(
            r#"
            SELECT m.id, m.conversation_id, m.public_id, m.seq, m.sender_id,
                   u.public_id AS sender_public_id, m.content_type, m.content,
                   r.public_id AS reply_to_public_id, m.recalled_at, m.created_at
            FROM messages m
            JOIN conversations c ON c.id = m.conversation_id
            JOIN conversation_members cm
              ON cm.conversation_id = c.id AND cm.user_id = $2
            LEFT JOIN users u ON u.id = m.sender_id
            LEFT JOIN messages r ON r.id = m.reply_to_id
            WHERE c.public_id = $1
              AND ($3::bigint IS NULL OR m.seq < $3)
            ORDER BY m.seq DESC
            LIMIT $4
            "#,
        )
        .bind(conversation_public_id)
        .bind(user_id)
        .bind(before_seq)
        .bind(limit)
        .fetch_all(db)
        .await?;

        Ok(messages)
    }

    /// 把消息标记为已撤回，返回是否真的改动了这一行。
    ///
    /// `AND recalled_at IS NULL` 让重复撤回变成「改动 0 行」：调用方据此区分
    /// 「刚撤回成功」与「已经被撤回」，不需要先读后写（那会在并发下双重撤回）。
    pub async fn mark_recalled(db: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("UPDATE messages SET recalled_at = NOW() WHERE id = $1 AND recalled_at IS NULL")
            .bind(id)
            .execute(db)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}

/// 用户公开 ID 到内部主键的批量换算。
///
/// 请求只携带公开 ID，而写外键需要内部主键。一次批量查询而不是逐个查询：
/// 群聊一次可能添加几十人，逐个换算就是几十次往返。
pub struct UserRefRepository;

impl UserRefRepository {
    pub async fn find_by_public_ids(db: &PgPool, public_ids: &[Uuid]) -> Result<Vec<UserRef>, sqlx::Error> {
        let users = sqlx::query_as::<_, UserRef>(
            r#"
            SELECT public_id, id
            FROM users
            WHERE public_id = ANY($1)
            "#,
        )
        .bind(public_ids)
        .fetch_all(db)
        .await?;

        Ok(users)
    }
}
