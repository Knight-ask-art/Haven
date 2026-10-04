//! 云盘服务端视图；生产 Wire 另作受控投影。

/// 账户投影：只含内部 UUID 与非敏感事实，没有 Provider 身份、没有凭据引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAccountView {
    pub id: String,
    pub display_name: String,
    pub connected: bool,
    pub generation: i64,
}

/// 目录绑定投影：`location_id` 是内部位置 UUID。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudFolderView {
    pub location_id: String,
    pub account_id: String,
    pub created_at_ms: i64,
}

/// 对象绑定投影：全部是内部 UUID。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudObjectView {
    pub object_id: String,
    pub media_item_id: String,
    pub resource_id: String,
    pub display_name: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudConnectStatus {
    Pending,
    Completing,
    Authorized,
    Cancelled,
    Expired,
    Failed,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudConnectAttemptView {
    pub attempt_id: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudConnectPollView {
    pub status: CloudConnectStatus,
    pub expires_at_ms: i64,
}

/// `account_id = None` 表示新连接；`Some` 表示重新授权既有账户（含断开墓碑）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudConnectBeginRequest {
    pub account_id: Option<String>,
}
