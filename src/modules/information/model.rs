//! 聊天消息模块（information）的数据模型。
//!
//! 模型分成三组，边界是「谁能看到它」：
//!
//! | 组别 | 例子 | 说明 |
//! | --- | --- | --- |
//! | 数据库实体 | [`ConversationRow`]、[`MessageRow`] | 只派生 `FromRow`，含内部主键，不参与序列化 |
//! | 请求模型 | [`CreateConversationRequest`] | 派生 `Deserialize` + `Validate`，在进入 Service 之前完成校验 |
//! | 响应模型 | [`ConversationSummary`]、[`Message`] | 派生 `Serialize` + `ToSchema`，只含调用方需要的字段 |
//!
//! 实体与响应模型刻意不共用：实体带着内部自增主键和原始正文，一旦让它同时
//! 派生 `Serialize`，内部主键和「已撤回消息的原文」就会顺着响应和 OpenAPI
//! Schema 一起对外暴露。类型分离是这里唯一可靠的防线。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use validator::{Validate, ValidationError};

/// 单聊会话的 `conversations.kind` 取值。
pub const KIND_SINGLE: &str = "single";
/// 群聊会话的 `conversations.kind` 取值。
pub const KIND_GROUP: &str = "group";
/// 群主的 `conversation_members.role` 取值。
pub const ROLE_OWNER: &str = "owner";

/// 文本消息正文的长度上限（字符数）。
///
/// 即使正文允许非文本类型，也必须给 `TEXT` 字段一个上限：没有上限的正文是一个
/// 「存储放大」入口，一次请求就能把日志、备份和客户端内存一起拖垮。
pub const MAX_CONTENT_LENGTH: usize = 4000;
/// 图片/文件消息正文的长度上限。这类消息存的是资源标识或 URL，不需要文本那么长。
pub const MAX_RESOURCE_LENGTH: usize = 2048;
/// 单个会话的成员上限。
pub const MAX_MEMBERS: usize = 200;

// ---------------------------------------------------------------------------
// 数据库实体
// ---------------------------------------------------------------------------

/// 会话实体。
#[derive(Debug, FromRow)]
pub struct ConversationRow {
    /// 内部自增主键，仅用于数据库关联，不通过 API 暴露。
    pub id: i64,
    pub public_id: Uuid,
    pub kind: String,
    pub title: Option<String>,
    /// 会话内已分配的最大消息序号。
    pub message_seq: i64,
    pub created_at: DateTime<Utc>,
}

/// 会话成员实体，已关联 `users` 取出公开资料。
///
/// 不带内部主键：成员信息只用于对外展示，权限判断全部在 SQL 里由
/// `conversation_members.user_id` 完成，应用层拿到内部主键也没有用武之地。
#[derive(Debug, FromRow)]
pub struct MemberRow {
    pub public_id: Uuid,
    pub username: String,
    pub nickname: Option<String>,
    pub role: String,
    pub joined_at: DateTime<Utc>,
}

/// 用户公开 ID 与内部主键的映射。
///
/// 请求只携带公开 ID，而写入外键需要内部主键，因此需要一次批量换算。
/// 单独成类型而不是用 `(Uuid, i64)` 元组：元组在调用点看不出哪个是 ID、哪个是主键。
#[derive(Debug, FromRow)]
pub struct UserRef {
    pub public_id: Uuid,
    pub id: i64,
}

/// 消息实体。
///
/// `content` 在这里是**原始正文**：撤回的消息正文仍然留在数据库里（供审计与
/// 后续可能的「撤销撤回」），清空发生在转换为 [`Message`] 时。实体不派生
/// `Serialize`，因此原始正文没有对外泄露的路径。
#[derive(Debug, FromRow)]
pub struct MessageRow {
    pub id: i64,
    pub conversation_id: i64,
    pub public_id: Uuid,
    pub seq: i64,
    pub sender_id: Option<i64>,
    pub sender_public_id: Option<Uuid>,
    pub content_type: String,
    pub content: String,
    pub reply_to_public_id: Option<Uuid>,
    pub recalled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// 会话列表用的投影：一个会话 + 我的已读位置 + 未读数 + 最后一条消息。
///
/// 用一条带 LATERAL 子查询的 SQL 一次取回，而不是「先查会话列表，再逐个查未读
/// 和最后一条消息」：后者是典型的 N+1 查询，会话一多就会变成几十次往返。
#[derive(Debug, FromRow)]
pub struct ConversationSummaryRow {
    pub public_id: Uuid,
    pub kind: String,
    pub title: Option<String>,
    pub member_count: i64,
    pub last_read_seq: i64,
    pub message_seq: i64,
    pub unread_count: i64,
    pub last_message_public_id: Option<Uuid>,
    pub last_message_seq: Option<i64>,
    pub last_message_sender_public_id: Option<Uuid>,
    pub last_message_content_type: Option<String>,
    pub last_message_content: Option<String>,
    pub last_message_recalled: Option<bool>,
    pub last_message_created_at: Option<DateTime<Utc>>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// 响应模型
// ---------------------------------------------------------------------------

/// 一条消息。
#[derive(Debug, Serialize, ToSchema)]
pub struct Message {
    pub public_id: Uuid,
    /// 会话内序号，同时是历史消息的翻页游标。
    pub seq: i64,
    /// 发送者的公开 ID；发送者注销后为 `null`。
    pub sender_public_id: Option<Uuid>,
    /// 消息类型：`text`、`image` 或 `file`。
    pub content_type: String,
    /// 消息正文。撤回后固定为空字符串。
    pub content: String,
    /// 被引用消息的公开 ID。
    pub reply_to_public_id: Option<Uuid>,
    /// 是否已撤回。客户端据此显示「对方撤回了一条消息」而不是空白气泡。
    pub recalled: bool,
    pub created_at: DateTime<Utc>,
}

impl From<MessageRow> for Message {
    fn from(row: MessageRow) -> Self {
        let recalled = row.recalled_at.is_some();

        Self {
            public_id: row.public_id,
            seq: row.seq,
            sender_public_id: row.sender_public_id,
            content_type: row.content_type,
            // 撤回必须把正文清空，否则「撤回」只是加了一个标记，
            // 原文仍然顺着接口返回给会话里的所有成员。
            content: if recalled { String::new() } else { row.content },
            reply_to_public_id: row.reply_to_public_id,
            recalled,
            created_at: row.created_at,
        }
    }
}

/// 会话成员。
#[derive(Debug, Serialize, ToSchema)]
pub struct ConversationMember {
    pub public_id: Uuid,
    pub username: String,
    pub nickname: Option<String>,
    /// `owner` 或 `member`。
    pub role: String,
    pub joined_at: DateTime<Utc>,
}

impl From<MemberRow> for ConversationMember {
    fn from(row: MemberRow) -> Self {
        Self {
            public_id: row.public_id,
            username: row.username,
            nickname: row.nickname,
            role: row.role,
            joined_at: row.joined_at,
        }
    }
}

/// 会话列表项。
#[derive(Debug, Serialize, ToSchema)]
pub struct ConversationSummary {
    pub public_id: Uuid,
    /// `single` 或 `group`。
    pub kind: String,
    pub title: Option<String>,
    pub member_count: i64,
    /// 我在该会话的已读位置（会话内序号）。
    pub last_read_seq: i64,
    /// 会话当前最新序号，可用来判断「我是否已经读到最新」。
    pub message_seq: i64,
    /// 未读数：别人发的、序号大于我的已读位置的消息条数。
    pub unread_count: i64,
    /// 最后一条消息；会话还没有任何消息时为 `null`。
    pub last_message: Option<Message>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<ConversationSummaryRow> for ConversationSummary {
    fn from(row: ConversationSummaryRow) -> Self {
        // 只有存在最后一条消息时才拼装 `Message`。这些列要么全有要么全无，
        // 因此只要公开 ID 存在就可以安全地取其余列的默认值。
        let last_message = row.last_message_public_id.map(|public_id| Message {
            public_id,
            seq: row.last_message_seq.unwrap_or(0),
            sender_public_id: row.last_message_sender_public_id,
            content_type: row.last_message_content_type.unwrap_or_default(),
            content: if row.last_message_recalled.unwrap_or(false) {
                String::new()
            } else {
                row.last_message_content.unwrap_or_default()
            },
            reply_to_public_id: None,
            recalled: row.last_message_recalled.unwrap_or(false),
            created_at: row.last_message_created_at.unwrap_or(row.created_at),
        });

        Self {
            public_id: row.public_id,
            kind: row.kind,
            title: row.title,
            member_count: row.member_count,
            last_read_seq: row.last_read_seq,
            message_seq: row.message_seq,
            unread_count: row.unread_count,
            last_message,
            last_message_at: row.last_message_at,
            created_at: row.created_at,
        }
    }
}

/// 会话详情，包含成员列表。
#[derive(Debug, Serialize, ToSchema)]
pub struct ConversationDetail {
    pub public_id: Uuid,
    /// `single` 或 `group`。
    pub kind: String,
    pub title: Option<String>,
    /// 群主的公开 ID；单聊或群主已注销时为 `null`。
    pub owner_public_id: Option<Uuid>,
    pub message_seq: i64,
    pub last_read_seq: i64,
    pub members: Vec<ConversationMember>,
    pub created_at: DateTime<Utc>,
}

/// 历史消息分页结果。
#[derive(Debug, Serialize, ToSchema)]
pub struct MessagePage {
    /// 按 `seq` 倒序排列（最新的在前），便于客户端从尾部向前翻页。
    pub items: Vec<Message>,
    /// 是否还有更早的消息。
    pub has_more: bool,
    /// 下一页游标：请求时把它作为 `before_seq` 即可。没有更早消息时为 `null`。
    pub next_before_seq: Option<i64>,
}

/// 已读回执结果。
#[derive(Debug, Serialize, ToSchema)]
pub struct ReadReceipt {
    pub conversation_public_id: Uuid,
    /// 推进后的已读位置。
    pub last_read_seq: i64,
    /// 推进后仍未读的消息条数。正常为 0，只有并发到达的新消息会大于 0。
    pub unread_count: i64,
}

// ---------------------------------------------------------------------------
// 请求模型
// ---------------------------------------------------------------------------

/// 会话类型。
///
/// 请求侧用枚举而不是字符串：非法取值在反序列化阶段就被拒绝为 `422`，
/// 不需要在 Service 里再写一遍 `match`。数据库实体侧仍用字符串，理由见
/// [`ConversationRow::kind`] 的注释。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    Single,
    Group,
}

impl ConversationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Single => KIND_SINGLE,
            Self::Group => KIND_GROUP,
        }
    }
}

/// 消息类型。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageContentType {
    /// 纯文本。
    #[default]
    Text,
    /// 图片，`content` 保存资源标识或 URL。
    Image,
    /// 文件，`content` 保存资源标识或 URL。
    File,
}

impl MessageContentType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Image => "image",
            Self::File => "file",
        }
    }

    /// 该类型正文的长度上限。
    const fn content_limit(self) -> usize {
        match self {
            Self::Text => MAX_CONTENT_LENGTH,
            Self::Image | Self::File => MAX_RESOURCE_LENGTH,
        }
    }
}

/// 创建会话请求。
#[derive(Debug, Deserialize, ToSchema, Validate)]
#[validate(schema(function = "validate_create_conversation_request"))]
#[serde(deny_unknown_fields)]
pub struct CreateConversationRequest {
    pub kind: ConversationKind,
    /// 群聊标题；单聊不使用该字段。
    #[validate(length(max = 100, message = "群聊标题长度不能超过 100 个字符"))]
    #[schema(max_length = 100)]
    pub title: Option<String>,
    /// 其他成员的公开 ID。
    ///
    /// 单聊必须且只能给一个（对方）；群聊可给多个，创建者自动成为群主。
    /// 这里没有、也不应该有「以谁的身份创建」这类字段：归属只能来自令牌，
    /// 请求体里出现 `user_id` 之类的多余字段会被 `deny_unknown_fields` 拒绝。
    #[schema(min_items = 1, max_items = 200)]
    pub member_public_ids: Vec<Uuid>,
}

fn validate_create_conversation_request(request: &CreateConversationRequest) -> Result<(), ValidationError> {
    match request.kind {
        ConversationKind::Single => {
            if request.member_public_ids.len() != 1 {
                return Err(custom_error("single_member_count", "单聊必须且只能指定一个成员"));
            }
            // 单聊没有标题：标题只在群聊里有意义，允许单聊传标题会让客户端
            // 以为「可以给单聊改名」，而实际它永远显示为对方的昵称。
            if request.title.is_some() {
                return Err(custom_error("single_title", "单聊不支持自定义标题"));
            }
        }
        ConversationKind::Group => {
            if request.member_public_ids.is_empty() {
                return Err(custom_error("group_member_count", "群聊至少需要指定一个成员"));
            }
            if request.member_public_ids.len() > MAX_MEMBERS {
                return Err(custom_error("group_member_limit", "群聊成员数量不能超过 200 人"));
            }
            if request.title.as_ref().is_some_and(|title| title.trim().is_empty()) {
                return Err(custom_error("group_title", "群聊标题不能只包含空白字符"));
            }
        }
    }

    Ok(())
}

/// 发送消息请求。
#[derive(Debug, Deserialize, ToSchema, Validate)]
#[validate(schema(function = "validate_send_message_request"))]
#[serde(deny_unknown_fields)]
pub struct SendMessageRequest {
    /// 消息类型，省略时为 `text`。
    #[serde(default)]
    pub content_type: MessageContentType,
    /// 消息正文：文本消息是原文，图片/文件消息是资源标识或 URL。
    #[schema(max_length = 4000, example = "在吗？")]
    pub content: String,
    /// 客户端生成的幂等标识。
    ///
    /// 弱网重试时同一个标识只会落一条消息，客户端因此可以直接重发，
    /// 不必先确认上一次请求是否到达服务端。
    #[validate(length(max = 64, message = "客户端消息标识长度不能超过 64 个字符"))]
    #[schema(max_length = 64)]
    pub client_message_id: Option<String>,
    /// 被引用消息的公开 ID，必须属于同一个会话。
    pub reply_to_public_id: Option<Uuid>,
}

fn validate_send_message_request(request: &SendMessageRequest) -> Result<(), ValidationError> {
    let content = request.content.trim();
    if content.is_empty() {
        return Err(custom_error("content_empty", "消息内容不能为空"));
    }

    // 按字符数而不是字节数校验：中文正文按字节算会先撞上限，
    // 同一个上限对中文用户和英文用户的实际可用长度就完全不同了。
    if request.content.chars().count() > request.content_type.content_limit() {
        let message = match request.content_type {
            MessageContentType::Text => "消息内容长度不能超过 4000 个字符",
            MessageContentType::Image | MessageContentType::File => "资源地址长度不能超过 2048 个字符",
        };
        return Err(custom_error("content_too_long", message));
    }

    if request
        .client_message_id
        .as_ref()
        .is_some_and(|id| id.trim().is_empty())
    {
        return Err(custom_error("client_message_id", "客户端消息标识不能为空字符串"));
    }

    Ok(())
}

/// 历史消息查询参数。
#[derive(Debug, Deserialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct MessagePageQuery {
    /// 只返回序号小于该值的历史消息；省略时从最新一条开始。
    #[validate(range(min = 1, message = "分页游标必须大于等于 1"))]
    #[param(minimum = 1)]
    pub before_seq: Option<i64>,
    /// 每页数量，默认 20，最大 100。
    #[serde(default = "default_limit")]
    #[validate(range(min = 1, max = 100, message = "每页数量必须在 1 到 100 之间"))]
    #[param(minimum = 1, maximum = 100, default = 20)]
    pub limit: i64,
}

fn default_limit() -> i64 {
    20
}

impl MessagePageQuery {
    pub fn limit(&self) -> i64 {
        self.limit
    }
    pub fn before_seq(&self) -> Option<i64> {
        self.before_seq
    }
}

/// 添加群成员请求。
#[derive(Debug, Deserialize, ToSchema, Validate)]
#[serde(deny_unknown_fields)]
pub struct AddMembersRequest {
    /// 待添加成员的公开 ID。
    #[validate(length(min = 1, max = 50, message = "每次必须添加 1 到 50 个成员"))]
    #[schema(min_items = 1, max_items = 50)]
    pub member_public_ids: Vec<Uuid>,
}

/// 构造带自定义提示的校验错误。
fn custom_error(code: &'static str, message: &'static str) -> ValidationError {
    let mut error = ValidationError::new(code);
    error.message = Some(message.into());
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_request(members: usize) -> CreateConversationRequest {
        CreateConversationRequest {
            kind: ConversationKind::Single,
            title: None,
            member_public_ids: (0..members).map(|_| Uuid::now_v7()).collect(),
        }
    }

    fn group_request(members: usize) -> CreateConversationRequest {
        CreateConversationRequest {
            kind: ConversationKind::Group,
            title: Some("项目讨论组".to_owned()),
            member_public_ids: (0..members).map(|_| Uuid::now_v7()).collect(),
        }
    }

    #[test]
    fn accepts_valid_single_and_group_requests() {
        single_request(1).validate().expect("单聊指定一个成员应通过校验");
        group_request(3).validate().expect("群聊指定多个成员应通过校验");
    }

    /// 单聊的成员数必须是 1：不传无法确定和谁聊，传多个则语义不明确。
    #[test]
    fn rejects_single_conversation_with_wrong_member_count() {
        for members in [0, 2] {
            let errors = single_request(members)
                .validate()
                .expect_err("单聊成员数不为 1 时应校验失败");

            assert!(errors.field_errors().contains_key("__all__"));
        }
    }

    /// 单聊没有标题：允许传标题会让客户端误以为可以给单聊改名。
    #[test]
    fn rejects_title_on_single_conversation() {
        let mut request = single_request(1);
        request.title = Some("不该存在的标题".to_owned());

        assert!(request.validate().is_err(), "单聊不应接受标题");
    }

    #[test]
    fn rejects_empty_group() {
        let errors = group_request(0).validate().expect_err("空群聊应校验失败");

        assert!(errors.field_errors().contains_key("__all__"));
    }

    #[test]
    fn rejects_blank_group_title() {
        let mut request = group_request(1);
        request.title = Some("   ".to_owned());

        assert!(request.validate().is_err(), "纯空白标题应被拒绝");
    }

    #[test]
    fn rejects_too_long_group_title() {
        let mut request = group_request(1);
        request.title = Some("标".repeat(101));

        let errors = request.validate().expect_err("超长标题应校验失败");
        assert!(errors.field_errors().contains_key("title"));
    }

    fn text_request(content: &str) -> SendMessageRequest {
        SendMessageRequest {
            content_type: MessageContentType::Text,
            content: content.to_owned(),
            client_message_id: None,
            reply_to_public_id: None,
        }
    }

    #[test]
    fn accepts_valid_text_message() {
        text_request("你好").validate().expect("普通文本消息应通过校验");
    }

    /// 空白正文必须被拒绝：一条只有空格的消息在客户端显示为空气泡，
    /// 既没有信息量，又会占满会话列表的预览位。
    #[test]
    fn rejects_blank_message_content() {
        for content in ["", "   ", "\n\t"] {
            assert!(
                text_request(content).validate().is_err(),
                "空白正文应被拒绝：{content:?}"
            );
        }
    }

    #[test]
    fn rejects_text_longer_than_limit() {
        let content = "字".repeat(MAX_CONTENT_LENGTH + 1);

        assert!(text_request(&content).validate().is_err(), "超长正文应被拒绝");
    }

    /// 长度按字符数计算：4000 个中文字符是 12000 字节，
    /// 若按字节判断，中文用户的实际可用长度只有英文用户的三分之一。
    #[test]
    fn counts_content_length_in_characters() {
        let content = "字".repeat(MAX_CONTENT_LENGTH);

        text_request(&content)
            .validate()
            .expect("正好达到字符数上限的正文应通过校验");
    }

    /// 图片/文件消息存的是 URL，上限应比文本更小。
    #[test]
    fn applies_smaller_limit_to_resource_messages() {
        let content = "u".repeat(MAX_RESOURCE_LENGTH + 1);
        let request = SendMessageRequest {
            content_type: MessageContentType::Image,
            content,
            client_message_id: None,
            reply_to_public_id: None,
        };

        assert!(request.validate().is_err(), "超长资源地址应被拒绝");
    }

    #[test]
    fn rejects_blank_client_message_id() {
        let mut request = text_request("你好");
        request.client_message_id = Some("  ".to_owned());

        assert!(request.validate().is_err(), "空白幂等键应被拒绝");
    }

    #[test]
    fn message_page_query_uses_defaults() {
        let query: MessagePageQuery = serde_json::from_str("{}").expect("空查询参数应可反序列化");

        assert_eq!(query.limit(), 20);
        assert_eq!(query.before_seq(), None);
    }

    #[test]
    fn rejects_out_of_range_message_page_limit() {
        for limit in [0, 101] {
            let query: MessagePageQuery =
                serde_json::from_str(&format!("{{\"limit\":{limit}}}")).expect("查询参数应可反序列化");

            assert!(query.validate().is_err(), "limit={limit} 应校验失败");
        }
    }

    #[test]
    fn rejects_empty_member_addition() {
        let request = AddMembersRequest { member_public_ids: Vec::new() };

        assert!(request.validate().is_err(), "空的添加成员请求应校验失败");
    }

    /// 撤回后的消息必须交出空正文。
    ///
    /// 这条用例锁定的是「撤回」的真正含义：只在响应里加一个 `recalled: true`
    /// 标记而保留正文，等于把撤回做成了一次前端动画。
    #[test]
    fn recalled_message_hides_its_content() {
        let row = MessageRow {
            id: 1,
            conversation_id: 1,
            public_id: Uuid::now_v7(),
            seq: 7,
            sender_id: Some(1),
            sender_public_id: Some(Uuid::now_v7()),
            content_type: "text".to_owned(),
            content: "这条内容不该被看到".to_owned(),
            reply_to_public_id: None,
            recalled_at: Some(Utc::now()),
            created_at: Utc::now(),
        };

        let message = Message::from(row);

        assert!(message.recalled);
        assert!(message.content.is_empty(), "撤回后正文必须为空");
    }

    #[test]
    fn keeps_content_for_normal_message() {
        let row = MessageRow {
            id: 1,
            conversation_id: 1,
            public_id: Uuid::now_v7(),
            seq: 1,
            sender_id: None,
            sender_public_id: None,
            content_type: "text".to_owned(),
            content: "你好".to_owned(),
            reply_to_public_id: None,
            recalled_at: None,
            created_at: Utc::now(),
        };

        let message = Message::from(row);

        assert!(!message.recalled);
        assert_eq!(message.content, "你好");
    }

    /// 会话还没有消息时，列表项的最后一条消息必须是 `null`，
    /// 而不是一个各字段为空串的伪消息。
    #[test]
    fn summary_without_messages_has_no_last_message() {
        let row = ConversationSummaryRow {
            public_id: Uuid::now_v7(),
            kind: KIND_GROUP.to_owned(),
            title: None,
            member_count: 2,
            last_read_seq: 0,
            message_seq: 0,
            unread_count: 0,
            last_message_public_id: None,
            last_message_seq: None,
            last_message_sender_public_id: None,
            last_message_content_type: None,
            last_message_content: None,
            last_message_recalled: None,
            last_message_created_at: None,
            last_message_at: None,
            created_at: Utc::now(),
        };

        let summary = ConversationSummary::from(row);

        assert!(summary.last_message.is_none());
    }
}
