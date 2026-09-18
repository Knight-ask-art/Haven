//! 连接会话：握手、请求分派、配额、身份生成、并发与取消。
//!
//! 契约来源：`docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md` §5.3、§5.5、§6.2、§8.3。
//!
//! 本层是 **Broker 与 Application service 之间唯一的桥**，也是 A5 安全收敛的落点：
//! - 请求集合**恰好 2 项**：`context` / `create_proposal`。没有 `get_proposal`、没有
//!   `approve` / `reject` / `apply` / `receipt` —— 它们在协议层就不存在帧类型。
//! - **身份由服务端生成**：握手时产出 `AgentSessionId`，每个通过校验的 frame id 映射为
//!   新的 `AgentRequestId`。客户端的 `hello.client` 只进日志，不进领域。
//! - **配额前置**：在调用 Application **之前**检查，超限即返回且零写入。
//!
//! ## 为什么拆成 `prepare` / `execute`
//!
//! 连接驱动要**真正**支持最多 [`MAX_INFLIGHT_PER_CONNECTION`] 个在途请求，因此"读一帧、
//! 同步跑完、写回"的写法不够用。职责按"快 / 慢"拆开：
//! - [`BrokerSession::prepare`] 只做快速判定（解析、握手、id、配额、在途登记），
//!   返回立即回复或一个 [`Dispatch`]；**不 await Application**。
//! - [`BrokerSession::execute`] 在独立任务里 await Application，带 20 秒超时与取消。
//!
//! 这样取消才有意义：`cancel` 帧能在**另一个请求仍在执行时**被读到并真的打断它。
//!
//! [`BrokerAgentApi`] 存在的理由：让这些规则可以在没有 SQLite、没有 Tauri 的情况下被
//! 完整测试。生产实现直接转调 `AgentSettingsIpcService`，不新建任何 Repository。

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{watch, Mutex};

use haven_application::wire::{
    AgentSettingsProposalCreateRequest, AgentSettingsProposalDto, PreferenceReadingContentWidthDto,
    PreferenceReadingFontFamilyDto, PreferenceReadingFontSizeDto, PreferenceReadingFontWeightDto,
    PreferenceReadingLetterSpacingDto, PreferenceReadingLineHeightDto,
    PreferenceReadingPaginationDto, PreferenceReadingPatchDto, PreferenceReadingThemeDto,
};
use haven_domain::agent::AgentCapabilityManifest;
use haven_domain::ids::{AgentRequestId, AgentSessionId};

use super::error::BrokerError;
use super::protocol::{
    encode_server_frame, parse_client_frame, CancelFrame, ClientFrame, ClientInfo,
    CreateProposalFrame, HelloFrame, OkFrame, PlainRequest, WelcomeFrame,
    MAX_EXTERNAL_PROPOSALS_PER_PROCESS, MAX_INFLIGHT_PER_CONNECTION,
    MAX_PENDING_PROPOSALS_PER_CONNECTION, PROTOCOL_VERSION,
};

/// 单请求超时。
///
/// **刻意大于 MCP 侧的 15 秒**：让 MCP 层先超时并给出它自己的稳定错误，而不是两层同时
/// 超时、错误归因不明（设计 §7.1）。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// 只读上下文投影：`context` 请求的响应载荷。
pub type ContextPayload = Value;

/// Broker 需要的两个 Application 用例。
///
/// **只有两个方法。** 这个 trait 的形状本身就是"Broker 不能 approve"的证明：
/// 没有对应方法，就没有可达路径。
#[async_trait]
pub trait BrokerAgentApi: Send + Sync {
    /// 对应 `AgentSettingsIpcService::context`。
    async fn context(&self) -> Result<ContextPayload, BrokerError>;

    /// 对应 `AgentSettingsIpcService::create_proposal`。
    ///
    /// `session_id` / `request_id` 由 Broker 生成后传入——**不是**客户端给的。
    async fn create_proposal(
        &self,
        session_id: AgentSessionId,
        request_id: AgentRequestId,
        request: &AgentSettingsProposalCreateRequest,
    ) -> Result<AgentSettingsProposalDto, BrokerError>;
}

/// 进程窗口配额：外部提案创建次数。
///
/// **进程窗口计数，不是跨重启的持久计数。** 重启 Haven 会清零。这是文档化的取舍
/// （设计 §8.3）：配额用来抑制噪音，不充当权限边界——即使清零，恶意进程能得到的
/// 仍然只是"多写一些 pending 提案"，无法 Apply。
pub struct ProcessQuota {
    used: AtomicU32,
    limit: u32,
}

impl Default for ProcessQuota {
    fn default() -> Self {
        Self::with_limit(MAX_EXTERNAL_PROPOSALS_PER_PROCESS)
    }
}

impl ProcessQuota {
    pub fn with_limit(limit: u32) -> Self {
        Self {
            used: AtomicU32::new(0),
            limit,
        }
    }

    pub fn used(&self) -> u32 {
        self.used.load(Ordering::SeqCst)
    }

    pub fn limit(&self) -> u32 {
        self.limit
    }

    /// 尝试占用一个名额。返回 `false` 表示已超限（**不得**调用 Application）。
    fn try_consume(&self) -> bool {
        let mut current = self.used.load(Ordering::SeqCst);
        loop {
            if current >= self.limit {
                return false;
            }
            match self.used.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    /// 归还一个名额（在途登记失败时使用，避免被拒的请求白吃额度）。
    fn refund(&self) {
        let mut current = self.used.load(Ordering::SeqCst);
        while current > 0 {
            match self.used.compare_exchange(
                current,
                current - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }
}

/// Broker 侧的 patch 形状：**snake_case**，与 MCP 工具 schema 同形。
///
/// Broker 是**命名边界**：对外是 snake_case（MCP 协议约定），对内要交给
/// `PreferenceReadingPatchDto`（Wire 约定是 camelCase + `deny_unknown_fields`）。
/// 这里显式列出字段并逐项映射，而不是写一个通用的 snake→camel 转换器：
/// 映射表是可审计的，任何新增字段都必须同时出现在这里，不会"看起来能跑"。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct BrokerReadingPatch {
    font_family: Option<PreferenceReadingFontFamilyDto>,
    custom_font_family: Option<String>,
    font_size: Option<PreferenceReadingFontSizeDto>,
    line_height: Option<PreferenceReadingLineHeightDto>,
    content_width: Option<PreferenceReadingContentWidthDto>,
    theme: Option<PreferenceReadingThemeDto>,
    custom_background: Option<String>,
    custom_text: Option<String>,
    font_weight: Option<PreferenceReadingFontWeightDto>,
    letter_spacing: Option<PreferenceReadingLetterSpacingDto>,
    system_auto: Option<bool>,
    pagination: Option<PreferenceReadingPaginationDto>,
}

impl BrokerReadingPatch {
    fn into_wire(self) -> PreferenceReadingPatchDto {
        PreferenceReadingPatchDto {
            font_family: self.font_family,
            custom_font_family: self.custom_font_family,
            font_size: self.font_size,
            line_height: self.line_height,
            content_width: self.content_width,
            theme: self.theme,
            custom_background: self.custom_background,
            custom_text: self.custom_text,
            font_weight: self.font_weight,
            letter_spacing: self.letter_spacing,
            system_auto: self.system_auto,
            pagination: self.pagination,
        }
    }
}

/// 解析 patch。
///
/// `null` 与"字段缺失"都映射为 `None`（= 不覆盖该字段），与既有 Wire DTO 的语义一致：
/// `PreferenceReadingPatchDto` 用 `Option` 表达"不覆盖"，没有独立的"清除"状态。
/// 这不是本层新引入的限制——Tauri UI 走的是同一个 DTO。
fn parse_reading_patch(value: Value) -> Result<PreferenceReadingPatchDto, ()> {
    serde_json::from_value::<BrokerReadingPatch>(value)
        .map(BrokerReadingPatch::into_wire)
        .map_err(|_| ())
}

/// 每连接的会话状态。
struct SessionState {
    session_id: AgentSessionId,
    app_version: String,
    handshaked: bool,
    /// 本连接已创建的提案数（配额）。
    ///
    /// 语义上等价于"未过期 pending 提案数"：TTL 是 24 小时，而连接通常在数分钟内结束，
    /// 因此连接存续期内已创建的都仍然 pending。若连接真的超过 24 小时，这个计数会比
    /// 实际 pending 数**偏大**——偏向更严格的一侧，符合 fail-closed 原则。
    created_proposals: u32,
    /// **活动**在途请求 id。已完成的 id 可以被复用。
    inflight: BTreeSet<i64>,
}

/// 一次通过校验、等待执行的 Application 调用。
#[derive(Debug)]
pub struct Dispatch {
    pub id: i64,
    action: DispatchAction,
}

#[derive(Debug)]
enum DispatchAction {
    Context,
    CreateProposal {
        session_id: AgentSessionId,
        request_id: AgentRequestId,
        request: Box<AgentSettingsProposalCreateRequest>,
    },
}

/// [`BrokerSession::prepare`] 的结果。
#[derive(Debug)]
pub enum Prepared {
    /// 立刻回复（或关闭），不进任务。
    Immediate(FrameOutcome),
    /// 交给任务执行；调用方必须在执行完毕后调用 [`BrokerSession::release`]。
    Dispatch(Dispatch),
}

/// 处理一帧之后的动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameOutcome {
    /// 发送这一帧（已编码为 JSON 文本），连接继续。
    Reply(String),
    /// 发送这一帧并**关闭**连接（协议级失败：版本不符、首帧不是 hello）。
    ReplyAndClose(String),
    /// 不回复，直接关闭（帧载荷超限——此时字节流已不可信）。
    Close,
}

/// 一个连接。
pub struct BrokerSession {
    state: Mutex<SessionState>,
    /// 在途请求 id → 取消信号。
    ///
    /// 用 **std** `Mutex`：临界区只有一次 HashMap 增删，从不跨 await。用 tokio 的
    /// 异步锁会让 `prepare` 多出一次 await，而它恰恰要在读循环里尽快返回。
    /// 锁序固定为 `state → cancels`，反向路径不存在。
    cancels: StdMutex<HashMap<i64, watch::Sender<bool>>>,
    api: Arc<dyn BrokerAgentApi>,
    quota: Arc<ProcessQuota>,
}

impl BrokerSession {
    pub fn new(
        api: Arc<dyn BrokerAgentApi>,
        quota: Arc<ProcessQuota>,
        app_version: impl Into<String>,
    ) -> Self {
        Self {
            state: Mutex::new(SessionState {
                session_id: AgentSessionId::new(),
                app_version: app_version.into(),
                handshaked: false,
                created_proposals: 0,
                inflight: BTreeSet::new(),
            }),
            cancels: StdMutex::new(HashMap::new()),
            api,
            quota,
        }
    }

    /// 本连接的会话身份。仅在握手后有意义。
    pub async fn session_id(&self) -> AgentSessionId {
        self.state.lock().await.session_id
    }

    /// 活动在途请求数（诊断与测试用）。
    pub async fn inflight_len(&self) -> usize {
        self.state.lock().await.inflight.len()
    }

    fn lock_cancels(&self) -> std::sync::MutexGuard<'_, HashMap<i64, watch::Sender<bool>>> {
        self.cancels
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 快速判定：解析、握手、id、配额、在途登记。**不 await Application**。
    pub async fn prepare(&self, text: &str) -> Prepared {
        // 帧载荷超限：字节流已不可信，断连不回复。
        if text.len() > super::protocol::MAX_FRAME_BYTES {
            return Prepared::Immediate(FrameOutcome::Close);
        }
        let frame = match parse_client_frame(text) {
            Ok(frame) => frame,
            Err(error) => {
                // 已握手的连接上，解析失败也要把顶层整数 id 回带：客户端据此把错误对回它发
                // 出的那条请求（未知帧、多余字段、载荷不合法都会走到这里）。握手阶段一律用
                // 0——那时还没有任何成立的请求 id，`0` 是协议里"没有请求 id"的保留值。
                let id = if self.state.lock().await.handshaked {
                    Self::recover_request_id(text)
                } else {
                    None
                };
                return Prepared::Immediate(self.reply_error(id, &error));
            }
        };

        let handshaked = self.state.lock().await.handshaked;
        // 首帧必须是 hello：未握手就发请求 → 协议失败并断连。
        if !handshaked && !matches!(frame, ClientFrame::Hello(_)) {
            return Prepared::Immediate(
                self.reply_and_close_error(None, &BrokerError::protocol_mismatch()),
            );
        }
        // 重复 hello 同样是协议失败。
        if handshaked && matches!(frame, ClientFrame::Hello(_)) {
            return Prepared::Immediate(
                self.reply_and_close_error(None, &BrokerError::protocol_mismatch()),
            );
        }

        match frame {
            ClientFrame::Hello(hello) => Prepared::Immediate(self.handshake(hello).await),
            ClientFrame::Cancel(frame) => self.cancel(frame),
            ClientFrame::Context(request) => self.prepare_context(request).await,
            ClientFrame::CreateProposal(frame) => self.prepare_create_proposal(frame).await,
        }
    }

    /// 执行一个 [`Dispatch`]，带 20 秒超时与取消。
    pub async fn execute(
        &self,
        dispatch: Dispatch,
        mut cancel: watch::Receiver<bool>,
    ) -> FrameOutcome {
        let id = dispatch.id;
        let action = dispatch.action;

        let cancelled = async move {
            // 已经置真则立刻返回，避免漏掉"取消先于 execute"的竞态。
            if *cancel.borrow() {
                return;
            }
            let _ = cancel.changed().await;
        };

        let result = tokio::select! {
            biased;
            _ = cancelled => Err(BrokerError::cancelled()),
            result = tokio::time::timeout(REQUEST_TIMEOUT, self.run_action(action)) => match result {
                Ok(inner) => inner,
                Err(_) => Err(BrokerError::timeout()),
            },
        };

        match result {
            Ok(payload) => self.reply_ok(id, payload),
            Err(error) => self.reply_error(Some(id), &error),
        }
    }

    /// 请求结束：释放在途登记与取消登记。**必须在 execute 之后调用**（含失败路径）。
    pub async fn release(&self, id: i64) {
        self.state.lock().await.inflight.remove(&id);
        self.lock_cancels().remove(&id);
    }

    async fn run_action(&self, action: DispatchAction) -> Result<Value, BrokerError> {
        match action {
            DispatchAction::Context => self.api.context().await,
            DispatchAction::CreateProposal {
                session_id,
                request_id,
                request,
            } => {
                let proposal = self
                    .api
                    .create_proposal(session_id, request_id, &request)
                    .await?;
                // 序列化失败**不得**退化成 `null` 成功：那会让客户端拿到一个看起来成功、
                // 实际没有内容的提案。宁可回一个内部错误。
                serde_json::to_value(&proposal).map_err(|_| BrokerError::internal())
            }
        }
    }

    // ---- 帧处理 ----

    async fn handshake(&self, hello: HelloFrame) -> FrameOutcome {
        if hello.protocol_version != PROTOCOL_VERSION {
            return self.reply_and_close_error(None, &BrokerError::protocol_mismatch());
        }
        log_client(&hello.client);

        let mut state = self.state.lock().await;
        // 会话身份在这里生成——客户端无从提供。
        state.session_id = AgentSessionId::new();
        state.handshaked = true;
        let welcome = WelcomeFrame::new(
            state.session_id.to_string(),
            state.app_version.clone(),
            AgentCapabilityManifest::for_current_slice(),
        );
        drop(state);

        match encode_server_frame(&welcome) {
            Ok(text) => FrameOutcome::Reply(text),
            Err(error) => self.reply_error(None, &error),
        }
    }

    async fn prepare_context(&self, request: PlainRequest) -> Prepared {
        match self.claim(request.id).await {
            Ok(()) => Prepared::Dispatch(Dispatch {
                id: request.id,
                action: DispatchAction::Context,
            }),
            Err(error) => Prepared::Immediate(self.reply_error(Some(request.id), &error)),
        }
    }

    async fn prepare_create_proposal(&self, frame: CreateProposalFrame) -> Prepared {
        let id = frame.id;
        let payload = frame.payload;

        // 只有 reading 分区：闭合集合，其他值直接拒绝。
        if payload.section != "reading" {
            return Prepared::Immediate(self.reply_error(
                Some(id),
                &BrokerError::invalid_argument("section 只支持 reading。"),
            ));
        }
        let patch = match parse_reading_patch(payload.patch) {
            Ok(patch) => patch,
            Err(()) => {
                return Prepared::Immediate(self.reply_error(
                    Some(id),
                    &BrokerError::invalid_argument("patch 字段不合法。"),
                ));
            }
        };

        // ---- 配额与在途登记：必须在进入 Application **之前** ----
        let (session_id, request_id) = {
            let mut state = self.state.lock().await;
            if !state.handshaked {
                return Prepared::Immediate(
                    self.reply_and_close_error(None, &BrokerError::protocol_mismatch()),
                );
            }
            if state.created_proposals >= MAX_PENDING_PROPOSALS_PER_CONNECTION {
                return Prepared::Immediate(
                    self.reply_error(Some(id), &BrokerError::quota_exceeded()),
                );
            }
            if !self.quota.try_consume() {
                return Prepared::Immediate(
                    self.reply_error(Some(id), &BrokerError::quota_exceeded()),
                );
            }
            // 在途登记失败（重复活动 id / 超限）时把刚占的配额还回去，
            // 否则被拒绝的请求会白吃进程窗口额度。
            if let Err(error) = claim_inflight(&mut state, id) {
                self.quota.refund();
                return Prepared::Immediate(self.reply_error(Some(id), &error));
            }
            let (sender, _) = watch::channel(false);
            self.lock_cancels().insert(id, sender);
            state.created_proposals += 1;
            (state.session_id, AgentRequestId::new())
        };

        // 客户端**不能**传这两个字段：帧载荷里没有它们，这里由 Broker 填。
        let request = AgentSettingsProposalCreateRequest {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            context_id: payload.context_id,
            context_hash: payload.context_hash,
            base_revision: payload.base_revision,
            patch,
        };

        Prepared::Dispatch(Dispatch {
            id,
            action: DispatchAction::CreateProposal {
                session_id,
                request_id,
                request: Box::new(request),
            },
        })
    }

    /// `cancel`：真的打断对应在途请求；未知 id 幂等。
    fn cancel(&self, frame: CancelFrame) -> Prepared {
        if let Some(sender) = self.lock_cancels().remove(&frame.id) {
            let _ = sender.send(true);
        }
        Prepared::Immediate(self.reply_ok(frame.id, json!({ "cancelled": true })))
    }

    // ---- 在途登记 ----

    async fn claim(&self, id: i64) -> Result<(), BrokerError> {
        let mut state = self.state.lock().await;
        claim_inflight(&mut state, id)?;
        let (sender, _) = watch::channel(false);
        self.lock_cancels().insert(id, sender);
        Ok(())
    }

    /// 取回某个在途请求的取消接收端。未知 id 返回一个永不触发的接收端。
    pub async fn cancel_receiver(&self, id: i64) -> watch::Receiver<bool> {
        self.lock_cancels()
            .get(&id)
            .map(watch::Sender::subscribe)
            .unwrap_or_else(|| watch::channel(false).1)
    }

    // ---- 回复构造 ----

    fn reply_ok(&self, id: i64, payload: Value) -> FrameOutcome {
        match encode_server_frame(&OkFrame::new(id, payload)) {
            Ok(text) => FrameOutcome::Reply(text),
            Err(error) => self.reply_error(Some(id), &error),
        }
    }

    fn reply_error(&self, id: Option<i64>, error: &BrokerError) -> FrameOutcome {
        FrameOutcome::Reply(self.error_text(id, error))
    }

    fn reply_and_close_error(&self, id: Option<i64>, error: &BrokerError) -> FrameOutcome {
        FrameOutcome::ReplyAndClose(self.error_text(id, error))
    }

    /// 从**解析失败**的原始帧里尽力取回顶层整数 `id`。
    ///
    /// 只认一种形状：合法 JSON 对象、且 `id` 字段是整数（`41.5` / `"41"` / `null` 都不算）。
    /// 这**不**放宽任何校验——请求本身仍然照原样被拒绝，`parse_client_frame` 的闭合集合、
    /// `deny_unknown_fields` 与 `validate_request_id` 的正整数校验一个都没动。这里只解决
    /// "错误帧对不上请求"：否则客户端拿到的每一条失败都是 `id: 0`，无法区分是哪次调用。
    fn recover_request_id(text: &str) -> Option<i64> {
        serde_json::from_str::<Value>(text)
            .ok()?
            .get("id")?
            .as_i64()
    }

    /// 错误帧文本。`id` 为 `None` 时用 `0` 占位（协议要求 `id` 字段存在）。
    ///
    /// `None` 恰好覆盖两种"没有可回带的请求 id"的情形：握手阶段的失败，以及连顶层整数
    /// `id` 都读不出来的帧。
    fn error_text(&self, id: Option<i64>, error: &BrokerError) -> String {
        let frame = super::protocol::ErrorFrame::new(id.unwrap_or(0), error.to_body());
        encode_server_frame(&frame).unwrap_or_else(|_| {
            r#"{"type":"error","id":0,"error":{"code":"INTERNAL_ERROR","message":"Broker 内部错误；该失败不携带任何可展示的细节。","retryable":false}}"#
                .to_owned()
        })
    }
}

/// 活动在途登记。已完成的 id 可以复用；**活动**的 id 重复或超过上限即拒绝。
fn claim_inflight(state: &mut SessionState, id: i64) -> Result<(), BrokerError> {
    if state.inflight.contains(&id) {
        return Err(BrokerError::invalid_argument(
            "请求 id 在本连接内已在使用。",
        ));
    }
    if state.inflight.len() >= MAX_INFLIGHT_PER_CONNECTION {
        return Err(BrokerError::too_many_inflight());
    }
    state.inflight.insert(id);
    Ok(())
}

/// `hello.client` 只用于诊断。
///
/// **不把客户端字符串写进日志**：`name` 是任意字符串，直接落日志会让"日志注入"变成
/// 一件只需起个怪名字就能做的事（换行、终端转义序列）。这里刻意不做插值。
fn log_client(_client: &ClientInfo) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32};

    /// 记录调用的假 API：让"Application 是否被调用"成为可断言的事实。
    #[derive(Default)]
    struct FakeApi {
        context_calls: AtomicU32,
        proposal_calls: AtomicU32,
        last_session: StdMutex<Option<String>>,
        last_request: StdMutex<Option<String>>,
        fail_with: StdMutex<Option<BrokerError>>,
        /// 置真时 `context` 会一直等待，用来测试并发、取消与超时。
        block_context: AtomicBool,
        /// 置真时 `create_proposal` 返回一个**无法序列化**的 DTO 是不可能的（类型固定），
        /// 因此"序列化失败"分支改由 `context` 返回不可序列化的 `Value` 覆盖不到——
        /// 见 `serialization_failure_is_not_a_null_success` 的说明。
        context_payload: StdMutex<Option<Value>>,
    }

    #[async_trait]
    impl BrokerAgentApi for FakeApi {
        async fn context(&self) -> Result<ContextPayload, BrokerError> {
            self.context_calls.fetch_add(1, Ordering::SeqCst);
            if self.block_context.load(Ordering::SeqCst) {
                // 一直挂起，直到测试主动取消/超时。
                std::future::pending::<()>().await;
            }
            if let Some(error) = self.fail_with.lock().unwrap().clone() {
                return Err(error);
            }
            if let Some(payload) = self.context_payload.lock().unwrap().clone() {
                return Ok(payload);
            }
            Ok(json!({ "context_hash": "a".repeat(64), "revision": "rev-1" }))
        }

        async fn create_proposal(
            &self,
            session_id: AgentSessionId,
            request_id: AgentRequestId,
            _request: &AgentSettingsProposalCreateRequest,
        ) -> Result<AgentSettingsProposalDto, BrokerError> {
            self.proposal_calls.fetch_add(1, Ordering::SeqCst);
            *self.last_session.lock().unwrap() = Some(session_id.to_string());
            *self.last_request.lock().unwrap() = Some(request_id.to_string());
            if let Some(error) = self.fail_with.lock().unwrap().clone() {
                return Err(error);
            }
            Err(BrokerError::capability_unavailable())
        }
    }

    fn session_with(api: Arc<FakeApi>, quota: Arc<ProcessQuota>) -> BrokerSession {
        BrokerSession::new(api, quota, "0.1.0-beta.1")
    }

    async fn handshake(session: &BrokerSession) -> String {
        let hello = r#"{"type":"hello","protocol_version":1,"client":{"name":"codex"}}"#;
        match session.prepare(hello).await {
            Prepared::Immediate(FrameOutcome::Reply(text)) => text,
            other => panic!("握手应成功，得到 {other:?}"),
        }
    }

    fn parse(text: impl AsRef<str>) -> Value {
        serde_json::from_str(text.as_ref()).unwrap()
    }

    fn immediate(outcome: Prepared) -> FrameOutcome {
        match outcome {
            Prepared::Immediate(outcome) => outcome,
            other => panic!("期望立即回复，得到 {other:?}"),
        }
    }

    /// 走完一次完整请求：prepare → execute → release。
    async fn round_trip(session: &BrokerSession, frame: &str) -> FrameOutcome {
        match session.prepare(frame).await {
            Prepared::Immediate(outcome) => outcome,
            Prepared::Dispatch(dispatch) => {
                let id = dispatch.id;
                let receiver = session.cancel_receiver(id).await;
                let outcome = session.execute(dispatch, receiver).await;
                session.release(id).await;
                outcome
            }
        }
    }

    fn proposal_frame(id: i64) -> String {
        format!(
            r#"{{"type":"create_proposal","id":{id},"payload":{{"section":"reading","context_id":"{CONTEXT_ID}","context_hash":"{CONTEXT_HASH}","base_revision":null,"patch":{{"font_size":"large"}}}}}}"#
        )
    }

    const CONTEXT_ID: &str = "0196f0d2-0000-7000-8000-0000000000c1";
    const CONTEXT_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[tokio::test]
    async fn handshake_issues_server_side_session_and_authoritative_capabilities() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api, Arc::new(ProcessQuota::default()));
        let welcome = parse(&handshake(&session).await);

        assert_eq!(welcome["type"], "welcome");
        assert_eq!(welcome["protocol_version"], 1);
        assert_eq!(
            welcome["granted_requests"],
            json!(["context", "create_proposal"])
        );
        assert_eq!(welcome["haven"]["capabilities"]["settings_read"], true);
        assert_eq!(welcome["haven"]["capabilities"]["secret_read"], false);
        let session_id = welcome["session_id"].as_str().unwrap().to_owned();
        assert_eq!(session_id.len(), 36, "必须是 UUID 形态");
        assert_eq!(session.session_id().await.to_string(), session_id);
    }

    #[tokio::test]
    async fn welcome_capabilities_are_projected_from_the_domain_manifest() {
        // 领域清单是唯一事实源：Broker 不得维护第二份布尔值。
        let manifest = AgentCapabilityManifest::for_current_slice();
        let report = super::super::protocol::CapabilityReport::from_manifest(manifest);
        assert_eq!(report.settings_read, manifest.capabilities.settings_read);
        assert_eq!(
            report.library_summary_read,
            manifest.capabilities.library_summary_read
        );
        assert_eq!(report.secret_read, manifest.capabilities.secret_read);
        assert_eq!(
            report.filesystem_write,
            manifest.capabilities.filesystem_write
        );

        // 变一个能力的清单必须产出不同的报告——证明是投影而不是复制。
        let mut mutated = manifest;
        mutated.capabilities.library_summary_read = true;
        let mutated_report = super::super::protocol::CapabilityReport::from_manifest(mutated);
        assert_ne!(
            mutated_report.library_summary_read,
            report.library_summary_read
        );
    }

    #[tokio::test]
    async fn client_info_cannot_influence_identity_or_capabilities() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api, Arc::new(ProcessQuota::default()));
        let hello = r#"{"type":"hello","protocol_version":1,"client":{"name":"admin","version":"999","instance_id":"x"}}"#;
        let welcome = parse(match session.prepare(hello).await {
            Prepared::Immediate(FrameOutcome::Reply(text)) => text,
            other => panic!("{other:?}"),
        });
        assert_eq!(welcome["haven"]["capabilities"]["filesystem_write"], false);
        assert_eq!(welcome["haven"]["capabilities"]["settings_proposal"], true);
        assert_ne!(welcome["session_id"], "admin");
    }

    #[tokio::test]
    async fn protocol_version_mismatch_closes_connection() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api, Arc::new(ProcessQuota::default()));
        for version in [0, 2, 99] {
            let hello = format!(
                r#"{{"type":"hello","protocol_version":{version},"client":{{"name":"x"}}}}"#
            );
            match immediate(session.prepare(&hello).await) {
                FrameOutcome::ReplyAndClose(text) => {
                    assert_eq!(
                        parse(&text)["error"]["code"],
                        "HAVEN_BROKER_PROTOCOL_MISMATCH"
                    );
                }
                other => panic!("版本 {version} 必须断连，得到 {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn request_before_hello_closes_connection() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api.clone(), Arc::new(ProcessQuota::default()));
        let outcome = immediate(
            session
                .prepare(r#"{"type":"context","id":1,"payload":{}}"#)
                .await,
        );
        assert!(matches!(outcome, FrameOutcome::ReplyAndClose(_)));
        assert_eq!(
            api.context_calls.load(Ordering::SeqCst),
            0,
            "未握手不得调用 Application"
        );
    }

    #[tokio::test]
    async fn duplicate_hello_closes_connection() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api, Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;
        let again = immediate(
            session
                .prepare(r#"{"type":"hello","protocol_version":1,"client":{"name":"x"}}"#)
                .await,
        );
        assert!(matches!(again, FrameOutcome::ReplyAndClose(_)));
    }

    #[tokio::test]
    async fn context_dispatches_to_application_and_returns_payload() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api.clone(), Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;
        let value = parse(
            match round_trip(&session, r#"{"type":"context","id":7,"payload":{}}"#).await {
                FrameOutcome::Reply(text) => text,
                other => panic!("{other:?}"),
            },
        );
        assert_eq!(value["type"], "ok");
        assert_eq!(value["id"], 7);
        assert_eq!(value["payload"]["revision"], "rev-1");
        assert_eq!(api.context_calls.load(Ordering::SeqCst), 1);
        assert_eq!(session.inflight_len().await, 0, "完成后必须释放在途登记");
    }

    #[tokio::test]
    async fn context_rejects_non_empty_payload() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api.clone(), Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;
        for frame in [
            r#"{"type":"context","id":1,"payload":{"section":"reading"}}"#,
            r#"{"type":"context","id":1,"payload":[]}"#,
            r#"{"type":"context","id":1,"payload":"x"}"#,
        ] {
            let value = parse(match immediate(session.prepare(frame).await) {
                FrameOutcome::Reply(text) => text,
                other => panic!("{other:?}"),
            });
            assert_eq!(value["error"]["code"], "INVALID_ARGUMENT", "帧：{frame}");
        }
        assert_eq!(api.context_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn request_ids_must_be_positive() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api, Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;
        for id in [0, -1, i64::MIN] {
            let frame = format!(r#"{{"type":"context","id":{id},"payload":{{}}}}"#);
            let value = parse(match immediate(session.prepare(&frame).await) {
                FrameOutcome::Reply(text) => text,
                other => panic!("{other:?}"),
            });
            assert_eq!(value["error"]["code"], "INVALID_ARGUMENT", "id={id}");
        }
    }

    /// 解析失败的错误帧必须回带**顶层整数 id**，而不是一律写 0。
    ///
    /// 这三类失败以前都退化成 `id: 0`：未知帧类型、多余字段、载荷不合法。客户端因此无法
    /// 把错误对回自己发出的请求。
    #[tokio::test]
    async fn parse_failure_errors_echo_the_top_level_integer_id() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api, Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;

        for (frame, code, id) in [
            (
                r#"{"type":"approve","id":41}"#,
                "HAVEN_BROKER_UNKNOWN_FRAME",
                41,
            ),
            (r#"{"id":45}"#, "HAVEN_BROKER_UNKNOWN_FRAME", 45),
            (
                r#"{"type":"context","id":42,"payload":{"unexpected":true}}"#,
                "INVALID_ARGUMENT",
                42,
            ),
            (
                r#"{"type":"context","id":43,"payload":{},"extra":true}"#,
                "INVALID_ARGUMENT",
                43,
            ),
            (
                r#"{"type":"create_proposal","id":44,"payload":{"section":"metadata"}}"#,
                "INVALID_ARGUMENT",
                44,
            ),
            // id 自身不合法（负数）时请求照样被拒，但回带的仍是客户端发来的那个 id。
            (r#"{"type":"cancel","id":-45}"#, "INVALID_ARGUMENT", -45),
        ] {
            let value = parse(match immediate(session.prepare(frame).await) {
                FrameOutcome::Reply(text) => text,
                other => panic!("帧 {frame} 必须立即回复，得到 {other:?}"),
            });
            assert_eq!(value["error"]["code"], code, "帧：{frame}");
            assert_eq!(value["id"], id, "错误帧必须回带请求 id，帧：{frame}");
            assert_ne!(value["id"], 0, "带 id 的失败不得固定为 0，帧：{frame}");
        }
    }

    /// 读不出顶层整数 id 时仍然用 `0`：非 JSON、非对象、缺 `id`、`id` 不是整数。
    #[tokio::test]
    async fn parse_failure_without_a_usable_integer_id_uses_zero() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api, Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;

        for frame in [
            "not json",
            "[1,2,3]",
            r#"{"type":"approve"}"#,
            r#"{"type":"approve","id":"41"}"#,
            r#"{"type":"approve","id":41.5}"#,
            r#"{"type":"approve","id":null}"#,
        ] {
            let value = parse(match immediate(session.prepare(frame).await) {
                FrameOutcome::Reply(text) => text,
                other => panic!("帧 {frame} 必须立即回复，得到 {other:?}"),
            });
            assert_eq!(value["id"], 0, "读不出整数 id 必须用 0，帧：{frame}");
        }
    }

    /// 握手阶段的错误仍然是 `id: 0`——那时还没有任何成立的请求 id。
    #[tokio::test]
    async fn handshake_stage_errors_still_use_zero() {
        // 未握手就发请求帧：协议失败并断连，id 是 0。
        let session = session_with(
            Arc::new(FakeApi::default()),
            Arc::new(ProcessQuota::default()),
        );
        match immediate(
            session
                .prepare(r#"{"type":"context","id":7,"payload":{}}"#)
                .await,
        ) {
            FrameOutcome::ReplyAndClose(text) => assert_eq!(parse(&text)["id"], 0),
            other => panic!("{other:?}"),
        }

        // 未握手 + 解析失败（顶层 id 本可读出）：同样用 0。
        let session = session_with(
            Arc::new(FakeApi::default()),
            Arc::new(ProcessQuota::default()),
        );
        let value = parse(
            match immediate(session.prepare(r#"{"type":"approve","id":8}"#).await) {
                FrameOutcome::Reply(text) => text,
                other => panic!("{other:?}"),
            },
        );
        assert_eq!(value["error"]["code"], "HAVEN_BROKER_UNKNOWN_FRAME");
        assert_eq!(value["id"], 0, "握手阶段的错误一律用 0");

        // 协议版本不符：仍然是 0。
        let session = session_with(
            Arc::new(FakeApi::default()),
            Arc::new(ProcessQuota::default()),
        );
        let frame = r#"{"type":"hello","protocol_version":99,"client":{"name":"x"}}"#;
        match immediate(session.prepare(frame).await) {
            FrameOutcome::ReplyAndClose(text) => {
                let value = parse(&text);
                assert_eq!(value["error"]["code"], "HAVEN_BROKER_PROTOCOL_MISMATCH");
                assert_eq!(value["id"], 0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn duplicate_active_request_id_is_rejected_but_reusable_after_release() {
        let api = Arc::new(FakeApi::default());
        api.block_context.store(true, Ordering::SeqCst);
        let session = session_with(api.clone(), Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;

        // 第一条在途并**保持**在途。
        let first = session
            .prepare(r#"{"type":"context","id":5,"payload":{}}"#)
            .await;
        assert!(matches!(first, Prepared::Dispatch(_)));
        assert_eq!(session.inflight_len().await, 1);

        // 同一 id 再来一次：必须拒绝。
        let value = parse(
            match immediate(
                session
                    .prepare(r#"{"type":"context","id":5,"payload":{}}"#)
                    .await,
            ) {
                FrameOutcome::Reply(text) => text,
                other => panic!("{other:?}"),
            },
        );
        assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");

        // 释放之后可以复用。
        session.release(5).await;
        assert_eq!(session.inflight_len().await, 0);
        let again = session
            .prepare(r#"{"type":"context","id":5,"payload":{}}"#)
            .await;
        assert!(matches!(again, Prepared::Dispatch(_)));
    }

    #[tokio::test]
    async fn up_to_eight_requests_can_be_in_flight_concurrently() {
        let api = Arc::new(FakeApi::default());
        api.block_context.store(true, Ordering::SeqCst);
        let session = Arc::new(session_with(api.clone(), Arc::new(ProcessQuota::default())));
        let _ = handshake(&session).await;

        let mut dispatches = Vec::new();
        for id in 1..=MAX_INFLIGHT_PER_CONNECTION as i64 {
            match session
                .prepare(&format!(r#"{{"type":"context","id":{id},"payload":{{}}}}"#))
                .await
            {
                Prepared::Dispatch(dispatch) => dispatches.push(dispatch),
                other => panic!("第 {id} 条应在途，得到 {other:?}"),
            }
        }
        assert_eq!(
            session.inflight_len().await,
            MAX_INFLIGHT_PER_CONNECTION,
            "必须真的能同时挂住 8 条"
        );

        // 第 9 条被拒。
        let value = parse(
            match immediate(
                session
                    .prepare(&format!(
                        r#"{{"type":"context","id":{},"payload":{{}}}}"#,
                        MAX_INFLIGHT_PER_CONNECTION + 1
                    ))
                    .await,
            ) {
                FrameOutcome::Reply(text) => text,
                other => panic!("{other:?}"),
            },
        );
        assert_eq!(value["error"]["code"], "HAVEN_BROKER_TOO_MANY_INFLIGHT");
        assert_eq!(value["error"]["retryable"], true);

        // 这个用例验证的是 prepare 阶段的真实在途登记，不执行永久阻塞的假 API。
        // 每个 dispatch 都必须显式释放，模拟连接驱动在断线时的统一清理路径。
        for dispatch in dispatches {
            session.release(dispatch.id).await;
        }
        assert_eq!(session.inflight_len().await, 0);
    }

    #[tokio::test]
    async fn cancel_actually_interrupts_an_in_flight_request() {
        let api = Arc::new(FakeApi::default());
        api.block_context.store(true, Ordering::SeqCst);
        let session = Arc::new(session_with(api.clone(), Arc::new(ProcessQuota::default())));
        let _ = handshake(&session).await;

        let dispatch = match session
            .prepare(r#"{"type":"context","id":42,"payload":{}}"#)
            .await
        {
            Prepared::Dispatch(dispatch) => dispatch,
            other => panic!("{other:?}"),
        };
        let receiver = session.cancel_receiver(42).await;
        let worker = {
            let session = session.clone();
            tokio::spawn(async move { session.execute(dispatch, receiver).await })
        };

        // 等它真的挂起来，再发 cancel 帧。
        tokio::task::yield_now().await;
        let cancel_reply = parse(
            match immediate(session.prepare(r#"{"type":"cancel","id":42}"#).await) {
                FrameOutcome::Reply(text) => text,
                other => panic!("{other:?}"),
            },
        );
        assert_eq!(cancel_reply["payload"]["cancelled"], true);

        let outcome = tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .expect("取消必须让请求结束，而不是一直挂着")
            .unwrap();
        let value = parse(match outcome {
            FrameOutcome::Reply(text) => text,
            other => panic!("{other:?}"),
        });
        assert_eq!(
            value["error"]["code"], "HAVEN_BROKER_CANCELLED",
            "被取消的请求必须回稳定错误码"
        );
        assert_eq!(value["id"], 42);
        session.release(42).await;
    }

    #[tokio::test]
    async fn cancel_before_execute_still_works() {
        // 取消先于 execute：不能在 `cancel.changed()` 上永久等待。
        let api = Arc::new(FakeApi::default());
        api.block_context.store(true, Ordering::SeqCst);
        let session = session_with(api, Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;

        let dispatch = match session
            .prepare(r#"{"type":"context","id":8,"payload":{}}"#)
            .await
        {
            Prepared::Dispatch(dispatch) => dispatch,
            other => panic!("{other:?}"),
        };
        let receiver = session.cancel_receiver(8).await;
        let _ = immediate(session.prepare(r#"{"type":"cancel","id":8}"#).await);

        let outcome =
            tokio::time::timeout(Duration::from_secs(2), session.execute(dispatch, receiver))
                .await
                .expect("取消先到也必须立刻结束");
        let value = parse(match outcome {
            FrameOutcome::Reply(text) => text,
            other => panic!("{other:?}"),
        });
        assert_eq!(value["error"]["code"], "HAVEN_BROKER_CANCELLED");
    }

    #[tokio::test]
    async fn cancel_is_idempotent_and_does_not_touch_application() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api.clone(), Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;
        for id in [1, 2, 999] {
            let frame = format!(r#"{{"type":"cancel","id":{id}}}"#);
            let value = parse(match immediate(session.prepare(&frame).await) {
                FrameOutcome::Reply(text) => text,
                other => panic!("{other:?}"),
            });
            assert_eq!(value["type"], "ok");
            assert_eq!(value["payload"]["cancelled"], true);
        }
        assert_eq!(api.context_calls.load(Ordering::SeqCst), 0);
        assert_eq!(api.proposal_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn forbidden_frames_never_reach_application() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api.clone(), Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;

        for forbidden in super::super::protocol::FORBIDDEN_FRAME_TYPES {
            let frame = format!(r#"{{"type":"{forbidden}","id":1}}"#);
            let value = parse(match immediate(session.prepare(&frame).await) {
                FrameOutcome::Reply(text) => text,
                other => panic!("{forbidden} 应回错误帧，得到 {other:?}"),
            });
            assert_eq!(
                value["error"]["code"], "HAVEN_BROKER_UNKNOWN_FRAME",
                "{forbidden} 必须被拒绝"
            );
        }
        assert_eq!(api.context_calls.load(Ordering::SeqCst), 0);
        assert_eq!(api.proposal_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn create_proposal_generates_identity_and_rejects_client_supplied_ids() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api.clone(), Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;

        for field in ["session_id", "request_id"] {
            let frame = format!(
                r#"{{"type":"create_proposal","id":1,"payload":{{"section":"reading","context_id":"{CONTEXT_ID}","context_hash":"{CONTEXT_HASH}","base_revision":null,"patch":{{}},"{field}":"x"}}}}"#
            );
            let value = parse(match immediate(session.prepare(&frame).await) {
                FrameOutcome::Reply(text) => text,
                other => panic!("{other:?}"),
            });
            assert_eq!(value["error"]["code"], "INVALID_ARGUMENT", "{field}");
        }
        assert_eq!(api.proposal_calls.load(Ordering::SeqCst), 0);

        let _ = round_trip(&session, &proposal_frame(2)).await;
        assert_eq!(api.proposal_calls.load(Ordering::SeqCst), 1);
        let expected_session = session.session_id().await.to_string();
        let actual_session = api.last_session.lock().unwrap().clone();
        assert_eq!(
            actual_session.as_deref(),
            Some(expected_session.as_str()),
            "传给 Application 的 session 必须是本连接的服务端身份"
        );
        assert!(
            api.last_request.lock().unwrap().is_some(),
            "必须生成请求身份"
        );
    }

    #[tokio::test]
    async fn all_create_proposals_share_one_session_but_get_distinct_requests() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api.clone(), Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;
        let _ = round_trip(&session, &proposal_frame(2)).await;
        let first = api.last_request.lock().unwrap().clone().unwrap();
        let _ = round_trip(&session, &proposal_frame(2)).await;
        let second = api.last_request.lock().unwrap().clone().unwrap();
        assert_ne!(first, second, "每次请求必须是新身份");
        let expected_session = session.session_id().await.to_string();
        let actual_session = api.last_session.lock().unwrap().clone();
        assert_eq!(
            actual_session,
            Some(expected_session),
            "同一连接共享一个 session"
        );
    }

    #[tokio::test]
    async fn only_reading_section_is_accepted() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api.clone(), Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;
        let frame = format!(
            r#"{{"type":"create_proposal","id":1,"payload":{{"section":"general","context_id":"{CONTEXT_ID}","context_hash":"{CONTEXT_HASH}","base_revision":null,"patch":{{}}}}}}"#
        );
        let value = parse(match immediate(session.prepare(&frame).await) {
            FrameOutcome::Reply(text) => text,
            other => panic!("{other:?}"),
        });
        assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
        assert_eq!(api.proposal_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn proposal_context_and_revision_are_bounded() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api.clone(), Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;

        let cases = [
            // context_id 超长
            format!(
                r#"{{"type":"create_proposal","id":1,"payload":{{"section":"reading","context_id":"{}","context_hash":"{CONTEXT_HASH}","base_revision":null,"patch":{{}}}}}}"#,
                "a".repeat(65)
            ),
            // context_id 空
            format!(
                r#"{{"type":"create_proposal","id":1,"payload":{{"section":"reading","context_id":"","context_hash":"{CONTEXT_HASH}","base_revision":null,"patch":{{}}}}}}"#
            ),
            // context_id 含控制字符
            format!(
                r#"{{"type":"create_proposal","id":1,"payload":{{"section":"reading","context_id":"a\u0000b","context_hash":"{CONTEXT_HASH}","base_revision":null,"patch":{{}}}}}}"#
            ),
            // context_hash 太短
            format!(
                r#"{{"type":"create_proposal","id":1,"payload":{{"section":"reading","context_id":"{CONTEXT_ID}","context_hash":"abc","base_revision":null,"patch":{{}}}}}}"#
            ),
            // context_hash 大写（不是 canonical digest）
            format!(
                r#"{{"type":"create_proposal","id":1,"payload":{{"section":"reading","context_id":"{CONTEXT_ID}","context_hash":"{}","base_revision":null,"patch":{{}}}}}}"#,
                "A".repeat(64)
            ),
            // base_revision 超长
            format!(
                r#"{{"type":"create_proposal","id":1,"payload":{{"section":"reading","context_id":"{CONTEXT_ID}","context_hash":"{CONTEXT_HASH}","base_revision":"{}","patch":{{}}}}}}"#,
                "r".repeat(129)
            ),
        ];
        for frame in cases {
            let value = parse(match immediate(session.prepare(&frame).await) {
                FrameOutcome::Reply(text) => text,
                other => panic!("{other:?}"),
            });
            assert_eq!(
                value["error"]["code"],
                "INVALID_ARGUMENT",
                "必须拒绝：{}",
                &frame[..80.min(frame.len())]
            );
        }
        assert_eq!(api.proposal_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn hello_client_fields_are_bounded() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api, Arc::new(ProcessQuota::default()));
        for hello in [
            format!(
                r#"{{"type":"hello","protocol_version":1,"client":{{"name":"{}"}}}}"#,
                "n".repeat(65)
            ),
            r#"{"type":"hello","protocol_version":1,"client":{"name":""}}"#.to_owned(),
            r#"{"type":"hello","protocol_version":1,"client":{"name":"a\nb"}}"#.to_owned(),
            format!(
                r#"{{"type":"hello","protocol_version":1,"client":{{"name":"ok","version":"{}"}}}}"#,
                "v".repeat(33)
            ),
            format!(
                r#"{{"type":"hello","protocol_version":1,"client":{{"name":"ok","instance_id":"{}"}}}}"#,
                "i".repeat(65)
            ),
        ] {
            let value = parse(match immediate(session.prepare(&hello).await) {
                FrameOutcome::Reply(text) => text,
                other => panic!("{other:?}"),
            });
            assert_eq!(value["error"]["code"], "INVALID_ARGUMENT", "{hello}");
        }
    }

    #[tokio::test]
    async fn per_connection_pending_quota_blocks_before_application() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api.clone(), Arc::new(ProcessQuota::with_limit(100)));
        let _ = handshake(&session).await;

        for _ in 0..MAX_PENDING_PROPOSALS_PER_CONNECTION {
            let _ = round_trip(&session, &proposal_frame(2)).await;
        }
        assert_eq!(
            api.proposal_calls.load(Ordering::SeqCst),
            MAX_PENDING_PROPOSALS_PER_CONNECTION
        );

        let value = parse(match immediate(session.prepare(&proposal_frame(2)).await) {
            FrameOutcome::Reply(text) => text,
            other => panic!("{other:?}"),
        });
        assert_eq!(value["error"]["code"], "HAVEN_BROKER_QUOTA_EXCEEDED");
        assert_eq!(value["error"]["retryable"], false);
        assert_eq!(
            api.proposal_calls.load(Ordering::SeqCst),
            MAX_PENDING_PROPOSALS_PER_CONNECTION,
            "超限后 Application **不得**被调用"
        );
    }

    #[tokio::test]
    async fn process_window_quota_blocks_before_application() {
        let api = Arc::new(FakeApi::default());
        let quota = Arc::new(ProcessQuota::with_limit(2));
        for _ in 0..2 {
            let session = session_with(api.clone(), quota.clone());
            let _ = handshake(&session).await;
            let _ = round_trip(&session, &proposal_frame(2)).await;
        }
        assert_eq!(api.proposal_calls.load(Ordering::SeqCst), 2);
        assert_eq!(quota.used(), 2);

        let session = session_with(api.clone(), quota.clone());
        let _ = handshake(&session).await;
        let value = parse(match immediate(session.prepare(&proposal_frame(2)).await) {
            FrameOutcome::Reply(text) => text,
            other => panic!("{other:?}"),
        });
        assert_eq!(value["error"]["code"], "HAVEN_BROKER_QUOTA_EXCEEDED");
        assert_eq!(
            api.proposal_calls.load(Ordering::SeqCst),
            2,
            "跨连接也不得越界"
        );
    }

    #[tokio::test]
    async fn quota_is_not_consumed_by_reads_rejections_or_duplicate_ids() {
        let api = Arc::new(FakeApi::default());
        api.block_context.store(true, Ordering::SeqCst);
        let quota = Arc::new(ProcessQuota::with_limit(4));
        let session = session_with(api.clone(), quota.clone());
        let _ = handshake(&session).await;

        // 只读
        let _ = session
            .prepare(r#"{"type":"context","id":1,"payload":{}}"#)
            .await;
        session.release(1).await;
        // 未知帧
        let _ = immediate(session.prepare(r#"{"type":"nonsense","id":2}"#).await);
        // section 非法
        let _ = immediate(
            session
                .prepare(&format!(
                    r#"{{"type":"create_proposal","id":3,"payload":{{"section":"general","context_id":"{CONTEXT_ID}","context_hash":"{CONTEXT_HASH}","base_revision":null,"patch":{{}}}}}}"#
                ))
                .await,
        );
        assert_eq!(quota.used(), 0, "这些都不该计数");

        // 在途 id 重复：配额必须被**归还**。
        let first = session.prepare(&proposal_frame(9)).await;
        assert!(matches!(first, Prepared::Dispatch(_)));
        assert_eq!(quota.used(), 1);
        let duplicate = immediate(session.prepare(&proposal_frame(9)).await);
        let value = parse(match duplicate {
            FrameOutcome::Reply(text) => text,
            other => panic!("{other:?}"),
        });
        assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
        assert_eq!(quota.used(), 1, "被拒绝的重复 id 不得白吃配额");
        session.release(9).await;
    }

    #[tokio::test]
    async fn disconnect_cleanup_releases_inflight_and_cancels() {
        let api = Arc::new(FakeApi::default());
        api.block_context.store(true, Ordering::SeqCst);
        let session = session_with(api.clone(), Arc::new(ProcessQuota::with_limit(4)));
        let _ = handshake(&session).await;

        for id in 1..=3 {
            let _ = session
                .prepare(&format!(r#"{{"type":"context","id":{id},"payload":{{}}}}"#))
                .await;
        }
        assert_eq!(session.inflight_len().await, 3);

        // 断线清理：释放全部在途登记与其取消通道。
        for id in 1..=3 {
            session.release(id).await;
        }
        assert_eq!(session.inflight_len().await, 0);
        assert!(
            session.cancel_receiver(1).await.has_changed().is_err(),
            "断线清理后不应残留可触发的取消发送端"
        );
    }

    #[tokio::test]
    async fn application_errors_are_passed_through_with_their_code() {
        let api = Arc::new(FakeApi::default());
        *api.fail_with.lock().unwrap() = Some(BrokerError::new(
            "SETTING_PROPOSAL_CONTEXT_STALE",
            "设置上下文已过期。",
            false,
        ));
        let session = session_with(api, Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;
        let value = parse(
            match round_trip(&session, r#"{"type":"context","id":1,"payload":{}}"#).await {
                FrameOutcome::Reply(text) => text,
                other => panic!("{other:?}"),
            },
        );
        assert_eq!(value["error"]["code"], "SETTING_PROPOSAL_CONTEXT_STALE");
        assert_eq!(value["error"]["retryable"], false);
    }

    #[tokio::test]
    async fn oversized_frame_closes_without_reply() {
        let api = Arc::new(FakeApi::default());
        let session = session_with(api, Arc::new(ProcessQuota::default()));
        let _ = handshake(&session).await;
        let huge = "x".repeat(super::super::protocol::MAX_FRAME_BYTES + 1);
        assert_eq!(immediate(session.prepare(&huge).await), FrameOutcome::Close);
    }
}
