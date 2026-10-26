//! 聊天消息模块的业务逻辑层。
//!
//! 这一层集中处理「聊天语义」，也就是数据库本身表达不了、但产品行为必须成立的规则：
//!
//! - 单聊对同一对用户是幂等的（重复发起只会进入同一个会话）；
//! - 群主退出时群主身份必须转让，最后一个人退出时会话解散；
//! - 撤回只允许发送者本人（有时间窗口）或群主；
//! - 已读位置只能前进，不能后退。

use chrono::{Duration as ChronoDuration, Utc};
use sqlx::PgPool;
use std::collections::HashSet;
use uuid::Uuid;

use crate::common::error::AppError;
use crate::modules::information::model::{
    AddMembersRequest, ConversationDetail, ConversationKind, ConversationMember, ConversationSummary,
    CreateConversationRequest, KIND_GROUP, MAX_MEMBERS, Message, MessagePage, MessagePageQuery, ReadReceipt, ROLE_OWNER,
    SendMessageRequest,
};
use crate::modules::information::repository::{ConversationRepository, MessageRepository, UserRefRepository};

/// 消息可撤回的时间窗口（分钟）。
///
/// 只约束「撤回自己发的消息」：群主撤回群成员的消息属于管理行为，不受发送时间限制。
/// 窗口写在代码里而不是做成配置项，是因为它与客户端文案（「超过 2 分钟不能撤回」）
/// 绑定，单独调大只会让服务端放行而客户端按钮早已消失。
const RECALL_WINDOW_MINUTES: i64 = 2;

/// 聊天消息业务逻辑层。
pub struct InformationService;

impl InformationService {
    /// 创建会话。
    ///
    /// 返回值的第二个元素表示「是否新建」。单聊是幂等的：同一对用户重复调用会
    /// 拿到同一个会话，此时对外应当是 `200` 而不是 `201`，否则客户端无法区分
    /// 「刚创建」与「早就存在」。
    pub async fn create_conversation(
        db: &PgPool,
        user_id: i64,
        req: CreateConversationRequest,
    ) -> Result<(ConversationDetail, bool), AppError> {
        // 先换算成员，顺带去重并剔除自己：请求里出现自己的公开 ID 时，
        // 「单聊只能有一个成员」这类判断会被重复项绕过。
        let peers = resolve_peer_ids(db, &req.member_public_ids, user_id).await?;

        match req.kind {
            ConversationKind::Single => Self::create_single_conversation(db, user_id, &peers, &req.member_public_ids).await,
            ConversationKind::Group => Self::create_group_conversation(db, user_id, peers, req.title.as_deref()).await,
        }
    }

    /// 创建（或复用）单聊会话。
    async fn create_single_conversation(
        db: &PgPool,
        user_id: i64,
        peers: &[i64],
        requested_public_ids: &[Uuid],
    ) -> Result<(ConversationDetail, bool), AppError> {
        // `peers` 已经把「自己」过滤掉了，因此这里为 0 说明请求里只有一个自己：
        // 单聊需要一个对话对象，和自己聊天没有意义。
        if peers.len() != 1 || requested_public_ids.len() != 1 {
            return Err(AppError::UnprocessableEntity("单聊必须且只能指定一个其他用户"));
        }

        let peer_id = peers[0];
        let direct_key = direct_key(user_id, peer_id);

        // 同一对用户的单聊唯一：重复点开必须进入同一个会话，
        // 否则同一段对话会分叉成两段各自残缺的历史。
        if let Some(existing) = ConversationRepository::find_by_direct_key(db, &direct_key).await? {
            let detail = Self::get_conversation(db, user_id, existing.public_id).await?;
            return Ok((detail, false));
        }

        let public_id = Uuid::now_v7();
        let members = [user_id, peer_id];

        match ConversationRepository::create_with_members(db, public_id, ConversationKind::Single.as_str(), None, None, Some(&direct_key), &members).await {
            Ok(conversation) => {
                let detail = Self::get_conversation(db, user_id, conversation.public_id).await?;
                Ok((detail, true))
            }
            // 两个客户端同时点开同一个单聊时，唯一索引只会放行一个。
            // 另一个按「已存在」处理，而不是把唯一键冲突暴露成 500。
            Err(sqlx::Error::Database(error)) if error.is_unique_violation() => {
                let existing = ConversationRepository::find_by_direct_key(db, &direct_key)
                    .await?
                    .ok_or(AppError::Internal("单聊会话冲突后无法读取"))?;
                let detail = Self::get_conversation(db, user_id, existing.public_id).await?;
                Ok((detail, false))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// 创建群聊会话，创建者自动成为群主。
    async fn create_group_conversation(
        db: &PgPool,
        user_id: i64,
        peers: Vec<i64>,
        title: Option<&str>,
    ) -> Result<(ConversationDetail, bool), AppError> {
        if peers.is_empty() {
            return Err(AppError::UnprocessableEntity("群聊至少需要一个其他成员"));
        }
        if peers.len() + 1 > MAX_MEMBERS {
            return Err(AppError::UnprocessableEntity("群聊成员数量不能超过 200 人"));
        }

        let public_id = Uuid::now_v7();
        let mut members = Vec::with_capacity(peers.len() + 1);
        // 创建者放在第一个成员位：`joined_at` 相同时，插入顺序决定群主排序的稳定性。
        members.push(user_id);
        members.extend(peers);

        let title = title.map(str::trim);
        let conversation = ConversationRepository::create_with_members(
            db,
            public_id,
            ConversationKind::Group.as_str(),
            title,
            Some(user_id),
            None,
            &members,
        )
        .await?;

        let detail = Self::get_conversation(db, user_id, conversation.public_id).await?;
        Ok((detail, true))
    }

    /// 列出我参与的会话（分页），按最后活跃时间倒序。
    pub async fn list_conversations(
        db: &PgPool,
        user_id: i64,
        page: i64,
        page_size: i64,
    ) -> Result<(Vec<ConversationSummary>, i64), AppError> {
        let offset = (page - 1) * page_size;

        let conversations = ConversationRepository::list_for_user(db, user_id, page_size, offset).await?;
        let total = ConversationRepository::count_for_user(db, user_id).await?;

        Ok((conversations.into_iter().map(ConversationSummary::from).collect(), total))
    }

    /// 获取会话详情（含成员列表）。
    ///
    /// 不是成员时同样返回「会话不存在」而不是 `403`：`403` 会告诉攻击者
    /// 「这个会话确实存在，只是你不在里面」，等于提供了一个探测接口。
    pub async fn get_conversation(db: &PgPool, user_id: i64, public_id: Uuid) -> Result<ConversationDetail, AppError> {
        let conversation = ConversationRepository::find_for_member(db, public_id, user_id)
            .await?
            .ok_or(AppError::NotFound("会话不存在"))?;

        let members = ConversationRepository::find_members(db, conversation.id).await?;
        let last_read_seq = ConversationRepository::find_last_read_seq(db, conversation.id, user_id).await?.unwrap_or(0);

        // 群主身份只来自成员表的 `role`：单聊没有群主，因此这里为 `None`；
        // 群主注销后其成员行被级联删除，也不会出现「群主指向一个不存在的用户」。
        let owner_public_id = members
            .iter()
            .find(|member| member.role == ROLE_OWNER)
            .map(|member| member.public_id);

        Ok(ConversationDetail {
            public_id: conversation.public_id,
            kind: conversation.kind,
            title: conversation.title,
            owner_public_id,
            message_seq: conversation.message_seq,
            last_read_seq,
            members: members.into_iter().map(ConversationMember::from).collect(),
            created_at: conversation.created_at,
        })
    }

    /// 发送消息。
    ///
    /// 客户端可以带 `client_message_id` 做幂等：同一个标识重复发送只会落一条消息，
    /// 且重试返回的是**第一次**那条消息，客户端不需要判断上一次请求到底成没成功。
    pub async fn send_message(
        db: &PgPool,
        user_id: i64,
        conversation_public_id: Uuid,
        req: SendMessageRequest,
    ) -> Result<Message, AppError> {
        // 先确认成员身份，让「不在会话里」得到明确的 `404`。
        // 写入语句里也有同样的成员判断，这里保留双重校验：会话可能在两次查询
        // 之间被解散，那条语句才是最终生效的守卫。
        let conversation = ConversationRepository::find_for_member(db, conversation_public_id, user_id)
            .await?
            .ok_or(AppError::NotFound("会话不存在"))?;

        // 引用目标必须落在同一个会话内：跨会话引用会让消息指向自己看不到的内容。
        let reply_to_id = match req.reply_to_public_id {
            Some(reply_public_id) => Some(
                MessageRepository::find_id_in_conversation(db, conversation.id, reply_public_id)
                    .await?
                    .ok_or(AppError::UnprocessableEntity("被引用的消息不属于该会话"))?,
            ),
            None => None,
        };

        let sent = MessageRepository::send(
            db,
            Uuid::now_v7(),
            user_id,
            req.content_type.as_str(),
            &req.content,
            req.client_message_id.as_deref(),
            reply_to_id,
            conversation_public_id,
        )
        .await?;

        if let Some(message) = sent {
            return Ok(Message::from(message));
        }

        // 没有写入成功时只有两种可能：幂等键命中了已发送的消息，或会话刚刚被解散。
        if let Some(client_message_id) = req.client_message_id.as_deref() {
            let existing = MessageRepository::find_by_client_message_id(db, conversation_public_id, user_id, client_message_id).await?;
            if let Some(existing) = existing {
                return Ok(Message::from(existing));
            }
        }

        Err(AppError::NotFound("会话不存在"))
    }

    /// 拉取历史消息（游标分页）。
    pub async fn list_messages(
        db: &PgPool,
        user_id: i64,
        conversation_public_id: Uuid,
        query: MessagePageQuery,
    ) -> Result<MessagePage, AppError> {
        ConversationRepository::find_for_member(db, conversation_public_id, user_id)
            .await?
            .ok_or(AppError::NotFound("会话不存在"))?;

        // 多取一条用来判断「还有没有更早的消息」，省掉一次 COUNT。
        let mut rows =
            MessageRepository::list_page(db, conversation_public_id, user_id, query.before_seq(), query.limit() + 1).await?;

        let has_more = rows.len() as i64 > query.limit();
        rows.truncate(query.limit() as usize);

        // 游标取本页最后一条（也就是最旧的一条）的序号：下一页从它往前翻。
        let next_before_seq = if has_more { rows.last().map(|row| row.seq) } else { None };

        Ok(MessagePage {
            items: rows.into_iter().map(Message::from).collect(),
            has_more,
            next_before_seq,
        })
    }

    /// 把我在该会话的已读位置推进到最新。
    pub async fn mark_read(db: &PgPool, user_id: i64, conversation_public_id: Uuid) -> Result<ReadReceipt, AppError> {
        let conversation = ConversationRepository::find_for_member(db, conversation_public_id, user_id)
            .await?
            .ok_or(AppError::NotFound("会话不存在"))?;

        let last_read_seq = ConversationRepository::mark_read(db, conversation.id, user_id)
            .await?
            .ok_or(AppError::NotFound("会话不存在"))?;

        // 推进之后仍有未读，只可能是那一刻之后到达的新消息。
        let unread_count = ConversationRepository::count_unread(db, conversation.id, user_id, last_read_seq).await?;

        Ok(ReadReceipt {
            conversation_public_id,
            last_read_seq,
            unread_count,
        })
    }

    /// 撤回消息。
    ///
    /// 允许两种人：发送者本人（限 2 分钟内），以及群主（管理行为，不限时间）。
    pub async fn recall_message(db: &PgPool, user_id: i64, message_public_id: Uuid) -> Result<Message, AppError> {
        let message = MessageRepository::find_for_member(db, message_public_id, user_id)
            .await?
            .ok_or(AppError::NotFound("消息不存在"))?;

        if message.recalled_at.is_some() {
            return Err(AppError::Conflict("消息已撤回"));
        }

        if message.sender_id == Some(user_id) {
            let deadline = message.created_at + ChronoDuration::minutes(RECALL_WINDOW_MINUTES);
            if Utc::now() > deadline {
                return Err(AppError::Conflict("超过可撤回时间，消息发出已超过 2 分钟"));
            }
        } else {
            let role = ConversationRepository::find_member_role(db, message.conversation_id, user_id).await?;
            if role.as_deref() != Some(ROLE_OWNER) {
                return Err(AppError::Forbidden("只能撤回自己发送的消息"));
            }
        }

        // 读出来到写回去之间可能被另一个请求抢先撤回，因此以「改动 0 行」为准。
        if !MessageRepository::mark_recalled(db, message.id).await? {
            return Err(AppError::Conflict("消息已撤回"));
        }

        let recalled = MessageRepository::find_by_id(db, message.id)
            .await?
            .ok_or(AppError::Internal("撤回后无法读取消息"))?;

        Ok(Message::from(recalled))
    }

    /// 向群聊添加成员，仅群主可操作。
    pub async fn add_members(
        db: &PgPool,
        user_id: i64,
        conversation_public_id: Uuid,
        req: AddMembersRequest,
    ) -> Result<ConversationDetail, AppError> {
        let conversation = ConversationRepository::find_for_member(db, conversation_public_id, user_id)
            .await?
            .ok_or(AppError::NotFound("会话不存在"))?;

        if conversation.kind != KIND_GROUP {
            return Err(AppError::Conflict("单聊会话不能添加成员"));
        }

        let role = ConversationRepository::find_member_role(db, conversation.id, user_id).await?;
        if role.as_deref() != Some(ROLE_OWNER) {
            return Err(AppError::Forbidden("只有群主可以添加成员"));
        }

        let new_member_ids = resolve_peer_ids(db, &req.member_public_ids, user_id).await?;
        if new_member_ids.is_empty() {
            return Err(AppError::UnprocessableEntity("没有需要添加的成员"));
        }

        // 成员上限用「现有成员数 + 本次请求人数」保守估算：精确值要等插入完成才知道，
        // 而那时数据已经写进数据库了。宁可少加几个，也不要先超限再回滚。
        let current_members = ConversationRepository::count_members(db, conversation.id).await?;
        if current_members + new_member_ids.len() as i64 > MAX_MEMBERS as i64 {
            return Err(AppError::Conflict("群聊成员数量不能超过 200 人"));
        }

        if ConversationRepository::add_members(db, conversation.id, &new_member_ids).await? == 0 {
            return Err(AppError::Conflict("所选用户已全部在会话中"));
        }

        Self::get_conversation(db, user_id, conversation_public_id).await
    }

    /// 退出群聊。
    ///
    /// 单聊不支持退出：退出一个两个人的会话等于删除对话，语义与「清空聊天记录」
    /// 混在一起会让另一方看到一半的对话。
    pub async fn leave_conversation(db: &PgPool, user_id: i64, conversation_public_id: Uuid) -> Result<(), AppError> {
        let conversation = ConversationRepository::find_for_member(db, conversation_public_id, user_id)
            .await?
            .ok_or(AppError::NotFound("会话不存在"))?;

        if conversation.kind != KIND_GROUP {
            return Err(AppError::Conflict("单聊会话不支持退出"));
        }

        let role = ConversationRepository::find_member_role(db, conversation.id, user_id).await?;

        if !ConversationRepository::remove_member(db, conversation.id, user_id).await? {
            return Err(AppError::NotFound("会话不存在"));
        }

        // 群主退出必须先移除自己再转让：反过来的话，转让候选里还包括自己。
        if role.as_deref() == Some(ROLE_OWNER) && ConversationRepository::promote_earliest_member(db, conversation.id).await?.is_none() {
            // 最后一个成员也走了，会话失去存在意义：删掉它，
            // 否则会留下一个没有成员、谁都打不开的僵尸会话。
            ConversationRepository::delete(db, conversation.id).await?;
        }

        Ok(())
    }
}

/// 把请求里的成员公开 ID 换算成内部主键，并去重、剔除自己。
///
/// 去重与剔除自己不是「顺手做的清理」，而是校验的前提：用户只要把同一个公开 ID
/// 传两遍，就能绕过「单聊只能有一个成员」的判断；把自己写进成员列表则会让
/// 成员表出现重复插入或让成员数虚高，进而影响人数上限判断。
async fn resolve_peer_ids(db: &PgPool, public_ids: &[Uuid], current_user_id: i64) -> Result<Vec<i64>, AppError> {
    let unique = public_ids.iter().copied().collect::<HashSet<_>>();
    let resolved = UserRefRepository::find_by_public_ids(db, public_ids).await?;

    // 任何一个 ID 查不到都直接报错，而不是「忽略不存在的用户」：
    // 静默忽略会让调用方以为成员已经加进去了，而群聊里根本没有这个人。
    if resolved.len() != unique.len() {
        return Err(AppError::NotFound("指定的用户不存在"));
    }

    // 保留请求中的顺序：成员列表的展示顺序与「群主 + 依次加入」的直觉一致。
    let mut ids = Vec::with_capacity(public_ids.len());
    for public_id in public_ids {
        let Some(user) = resolved.iter().find(|user| user.public_id == *public_id) else {
            continue;
        };

        if user.id == current_user_id || ids.contains(&user.id) {
            continue;
        }

        ids.push(user.id);
    }

    Ok(ids)
}

/// 单聊唯一键：始终用「较小的主键:较大的主键」。
///
/// 方向无关是关键：A 找 B 与 B 找 A 必须落到同一个键，否则同一段对话会出现
/// 两个会话，各自只有一方发出的消息。分隔符也不能省略，否则 `1` 和 `23`
/// 与 `12` 和 `3` 会拼出同一个键。
fn direct_key(left: i64, right: i64) -> String {
    let (low, high) = if left <= right { (left, right) } else { (right, left) };

    format!("{low}:{high}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 找 B 与 B 找 A 必须得到同一个键，否则同一段对话会分裂成两个会话。
    #[test]
    fn direct_key_is_order_independent() {
        assert_eq!(direct_key(1, 2), direct_key(2, 1));
        assert_eq!(direct_key(7, 3), "3:7");
    }

    /// 分隔符不能省：否则 `(1, 23)` 与 `(12, 3)` 会拼出同一个键，
    /// 让两组完全不同的用户共享同一个单聊会话。
    #[test]
    fn direct_key_distinguishes_different_pairs() {
        assert_ne!(direct_key(1, 23), direct_key(12, 3));
    }

    /// 与自己配对是合法输入（调用方已在上层拒绝），但键仍必须稳定。
    #[test]
    fn direct_key_handles_same_user() {
        assert_eq!(direct_key(5, 5), "5:5");
    }
}
