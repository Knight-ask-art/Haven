//! A5 外部 Agent Broker：把 MCP server 接到运行中的 Haven。
//!
//! 契约来源：`docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md`。
//!
//! 分层：
//! - [`protocol`] —— 纯协议：framing、闭合帧集合、上限、错误码。无平台依赖。
//! - [`session`] —— 连接状态机：握手、分派、配额、身份生成。只依赖 [`BrokerAgentApi`]。
//! - [`endpoint`] —— 每用户稳定端点的计算与校验。
//! - [`listener`] —— 平台原语（Named Pipe / Unix socket）与 ACL。
//! - 本文件 —— 组合：生产用的 [`BrokerAgentApi`] 实现 + 可启停的 [`AgentBrokerManager`]。
//!
//! **默认关闭。** 端点只在用户显式开启后才创建。

pub mod endpoint;
pub mod error;
pub mod listener;
pub mod protocol;
pub mod session;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

use haven_application::services::agent_settings_ipc::AgentSettingsIpcService;
use haven_application::wire::{AgentSettingsProposalCreateRequest, AgentSettingsProposalDto};
use haven_domain::ids::{AgentRequestId, AgentSessionId};

use endpoint::BrokerEndpoint;
use error::BrokerError;
use session::{BrokerAgentApi, ContextPayload, ProcessQuota};

/// 生产用的 [`BrokerAgentApi`]：直接转调 A1 已有的 Application service。
///
/// **不新建 Repository、不新建 SQLite 连接**：这里只持有一个 `AgentSettingsIpcService`
/// 的克隆，与 UI 走的是同一份实现、同一个 UoW、同一个设置事实源。
pub struct AgentSettingsBrokerApi {
    service: AgentSettingsIpcService,
}

impl AgentSettingsBrokerApi {
    pub fn new(service: AgentSettingsIpcService) -> Self {
        Self { service }
    }
}

#[async_trait]
impl BrokerAgentApi for AgentSettingsBrokerApi {
    async fn context(&self) -> Result<ContextPayload, BrokerError> {
        let context = self
            .service
            .context()
            .await
            .map_err(|error| BrokerError::from(&error))?;
        // 投影成 DTO 再序列化：只有经过既有投影的类型才可能出现在响应里。
        serde_json::to_value(context.to_dto()).map_err(|_| BrokerError::internal())
    }

    async fn create_proposal(
        &self,
        session_id: AgentSessionId,
        request_id: AgentRequestId,
        request: &AgentSettingsProposalCreateRequest,
    ) -> Result<AgentSettingsProposalDto, BrokerError> {
        self.service
            .create_proposal(session_id, request_id, request)
            .await
            .map_err(|error| BrokerError::from(&error))
    }
}

/// Broker 对外状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrokerStatus {
    /// 用户未开启（默认）。
    Disabled,
    /// 正在监听。
    Listening { endpoint: String },
    /// 端点被另一个 Haven 实例占用；本实例 fail closed。
    ///
    /// **[`AgentBrokerManager::status`] 永远不会投影出这个变体。** 判定"端点是否被占用"
    /// 只有一个可靠时刻：真正去 `bind` 的那一次。`status` 按设计不连接、不探测端点
    /// （见 `AgentBrokerManager::project_status`），因此它只可能报 `disabled` /
    /// `listening` / `unavailable`，不会凭一次非探测的读取断言"另一个实例在跑"——
    /// 那会把不确定的环境失败说成已知事实，制造虚假可达性。
    ///
    /// busy 的**唯一**对外出口是 [`AgentBrokerManager::enable`] 的失败结果
    /// （`HAVEN_BROKER_ENDPOINT_BUSY`，见 [`error::BrokerError::endpoint_busy`]）。
    /// 保留这个变体是为了让那条失败在类型层面可表达、并被投影成 wire 的 `busy`，
    /// 而不是声称 `status` 会主动去发现它。
    EndpointBusy,
    /// 当前平台不支持，或端点无法解析。
    Unavailable { reason: String },
}

impl BrokerStatus {
    /// 稳定的状态标签，供 UI 与 wire 使用。
    pub fn label(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Listening { .. } => "listening",
            Self::EndpointBusy => "busy",
            Self::Unavailable { .. } => "unavailable",
        }
    }

    /// 当前暴露给客户端的端点（仅在监听时有值）。
    pub fn endpoint(&self) -> Option<&str> {
        match self {
            Self::Listening { endpoint } => Some(endpoint.as_str()),
            _ => None,
        }
    }
}

struct RunningBroker {
    endpoint: BrokerEndpoint,
    shutdown: watch::Sender<bool>,
    /// serve 相位：`true` = 平台 serve 仍在 accept；`false` = 已退出 accept 循环，正在
    /// abort/drain 在途连接。由 `listener::serve` 在确定不再接收新连接的那一刻置假。
    serving: watch::Receiver<bool>,
    task: JoinHandle<()>,
}

impl RunningBroker {
    /// 这个 serve 是否**确实还在 accept**。两个条件缺一不可：
    ///
    /// - `is_finished` 为假——任务本身还在跑。任务在进入平台 serve 之前就结束（非
    ///   Unix/Windows 平台的空实现、spawn 后立刻 panic）时，只有这一条能判出来。
    /// - `serving` 为真——平台 serve 没有宣告停止接收。退出 accept 循环之后到任务真正结束
    ///   之间还有一段收尾（abort/drain `connection_tasks`，Unix 侧再删 socket 文件）：这段
    ///   时间里 join handle 仍未结束，但端点上已经没有人在 accept 了。
    fn is_serving(&self) -> bool {
        !self.task.is_finished() && *self.serving.borrow()
    }
}

/// [`serve_state`] 的判定结果。
enum ServeState {
    /// 有一个**确实还在 accept** 的 serve。
    Running(String),
    /// 没有在监听。`stale` 是就地取走的、**已经不在 accept** 的 serve（任务可能已经结束，
    /// 也可能还在连接收尾；`None` = 本来就没有）。
    Stopped(Option<RunningBroker>),
}

/// 判定当前是否有一个确实还在 accept 的 serve。
///
/// 判定与取走在同一个临界区内**同步**完成：不再 serve 的条目（任务已结束，**或者**平台
/// serve 已宣告停止接收、只剩连接收尾）会被立刻从 `running` 里取走。这样调用方就能把
/// "回收"这件事拆成两半——锁内只做判定与取走，`JoinHandle` 的 await 一律留在锁外。
/// `manager` 的 mutex 因此不会跨 await 持有。
fn serve_state(running: &mut Option<RunningBroker>) -> ServeState {
    match running.as_ref() {
        Some(broker) if broker.is_serving() => ServeState::Running(broker.endpoint.display()),
        Some(_) => ServeState::Stopped(running.take()),
        None => ServeState::Stopped(None),
    }
}

/// 可启停的 Broker manager。由 `AppState` 持有。
pub struct AgentBrokerManager {
    /// 启停的串行化边界：[`AgentBrokerManager::status`]、[`AgentBrokerManager::enable`]、
    /// [`AgentBrokerManager::disable`] 三者的第一步都是取它。
    ///
    /// **加锁顺序恒为 `lifecycle` → `running`**，任何路径都不得反向取。
    ///
    /// 它存在的唯一理由是 `disable` 的收尾窗口：`disable` 必须先把 `RunningBroker` 从
    /// `running` 里取走（此后 `running` 为空），再等 serve 真正退出。少了这把覆盖整个
    /// 启停过程的锁，并发的 `enable` 就会看到那个"空档"并尝试 bind——而旧 serve 还没
    /// 释放端点，于是 manager 把自己的 shutdown 窗口误报成 `ENDPOINT_BUSY`。
    ///
    /// 因此本锁**有意**跨 `listener::bind` 与 serve 收尾的 await 持有：[`running`] 锁一次
    /// 都不跨 await，串行化由这里负责。
    ///
    /// [`running`]: Self::running
    lifecycle: Mutex<()>,
    running: Mutex<Option<RunningBroker>>,
    quota: Arc<ProcessQuota>,
    app_version: String,
}

impl AgentBrokerManager {
    pub fn new(app_version: impl Into<String>) -> Self {
        Self {
            lifecycle: Mutex::new(()),
            running: Mutex::new(None),
            quota: Arc::new(ProcessQuota::default()),
            app_version: app_version.into(),
        }
    }

    /// 当前**确实在监听**的端点。
    ///
    /// 一个已经不再 accept 的 serve 不构成"正在监听"——无论是任务已经结束，还是平台 serve
    /// 已经退出 accept 循环、只剩连接收尾（见 [`RunningBroker::is_serving`]）：两种情况下
    /// 端点上都没有人在 accept 了。这类条目在这里被回收，取走在锁内同步完成，
    /// `JoinHandle` 的 await 留在锁外（`running` 锁不跨 await）。
    ///
    /// 这次 await **可能真的会等**：serve 退出 accept 循环之后还要 abort/drain 在途连接，
    /// Unix 侧再删 socket 文件，端点要到那时才真正空出来。等它是**必要**的而不是保守的——
    /// 调用方（`enable`）正是靠它保证"上一个 serve 收尾完成之前不去 bind"，否则会把自己
    /// 的收尾窗口误报成 `ENDPOINT_BUSY`。
    ///
    /// 调用方必须已持有 [`Self::lifecycle`]：本函数只做投影与回收，不参与串行化。
    async fn listening_endpoint(&self) -> Option<String> {
        let stale = {
            let mut guard = self.running.lock().await;
            match serve_state(&mut guard) {
                ServeState::Running(endpoint) => return Some(endpoint),
                ServeState::Stopped(stale) => stale,
            }
        };
        if let Some(stale) = stale {
            let _ = stale.task.await;
        }
        None
    }

    /// 内部状态投影：把**当下**的事实折成 [`BrokerStatus`]。**不**尝试连接或探测端点。
    ///
    /// 调用方必须已持有 [`Self::lifecycle`]（因此本函数不取它）。`disable` 需要在同一个
    /// 临界区里接着投影收尾之后的状态，而回头去调会再次取锁的 [`Self::status`] 只会自锁。
    async fn project_status(&self) -> BrokerStatus {
        // 已经不在 accept 的 serve 不得被投影成 `listening`——先回收（必要时等它收尾完），
        // 再按当前事实判定。
        if let Some(endpoint) = self.listening_endpoint().await {
            return BrokerStatus::Listening { endpoint };
        }
        match endpoint::resolve_endpoint() {
            Ok(_) => BrokerStatus::Disabled,
            Err(error) => BrokerStatus::Unavailable {
                reason: error.message().to_owned(),
            },
        }
    }

    /// 当前状态。**不**尝试连接或探测端点。
    ///
    /// 投影在 `lifecycle` 锁内完成：一次在途的启停操作要么整体完成、要么整体没
    /// 发生，这里读不到"条目已取走、serve 还在收尾"这种中间态。
    pub async fn status(&self) -> BrokerStatus {
        let _lifecycle = self.lifecycle.lock().await;
        self.project_status().await
    }

    /// 显式开启：创建端点并开始监听。已在监听时是幂等的。
    ///
    /// **已经不在 accept 的 serve 不算"已开启"。** 任务可能因为平台层错误或内部失败而
    /// 退出；也可能刚刚退出 accept 循环、正在收尾连接。两种情况端点都没有人在听，把这种
    /// 条目当成幂等成功会让 UI 显示 `listening` 而外部 Agent 连不上。因此这里先回收它
    /// （必要时等它收尾完成，端点真正空出来），再以**当下**的事实决定是幂等返回还是继续启用。
    ///
    /// 整个函数在 `lifecycle` 锁内运行，所以不存在"检查与 bind 之间被另一个启停操作
    /// 插队"的窗口——这正是 bind 可以不持有 `running` 锁的原因。
    pub async fn enable(&self, api: Arc<dyn BrokerAgentApi>) -> Result<BrokerStatus, BrokerError> {
        let _lifecycle = self.lifecycle.lock().await;

        // 仍在运行的 serve：幂等返回。`listening_endpoint` 顺手就地回收任何已经不在 accept
        // 的条目（并等它收尾完），因此它返回 `None` 之后 `running` 必定为空——没有别人能在
        // 这中间改动它。
        if let Some(endpoint) = self.listening_endpoint().await {
            return Ok(BrokerStatus::Listening { endpoint });
        }

        let endpoint = endpoint::resolve_endpoint()?;
        let (shutdown, shutdown_rx) = watch::channel(false);
        // serve 相位：从 `true` 起步。句柄一造出来就是"平台 serve 仍在 accept"，直到
        // `listener::serve` 自己宣告停止接收为止。
        let (serving, serving_rx) = watch::channel(true);
        // 这个 await 由 lifecycle 锁兜住：没有第二个 enable 在 bind，也没有 disable 正在
        // 等 serve 收尾，所以端点此刻必定空闲——不会把自己的收尾窗口报成 `ENDPOINT_BUSY`。
        let listener = listener::bind(&endpoint).await?;
        let bound_endpoint = listener.endpoint().clone();

        let task = tokio::spawn(listener::serve(
            listener,
            api,
            self.quota.clone(),
            self.app_version.clone(),
            shutdown_rx,
            serving,
        ));

        // running 锁只覆盖这一次同步写入，不跨任何 await。
        *self.running.lock().await = Some(RunningBroker {
            endpoint: bound_endpoint.clone(),
            shutdown,
            serving: serving_rx,
            task,
        });
        Ok(BrokerStatus::Listening {
            endpoint: bound_endpoint.display(),
        })
    }

    /// 显式关闭：立即停止监听并断开在途连接（通过 shutdown 广播）。
    ///
    /// 从取走条目到 serve 真正退出，整个过程都在 `lifecycle` 锁内：窗外看不到"端点已
    /// 空出、旧 serve 仍在监听"的中间态，因此 disable→enable 不会在这个窗口里撞上
    /// `ENDPOINT_BUSY`。
    pub async fn disable(&self) -> BrokerStatus {
        let _lifecycle = self.lifecycle.lock().await;
        let taken = self.running.lock().await.take();
        if let Some(running) = taken {
            let _ = running.shutdown.send(true);
            // 必须等待监听任务真正退出：Unix 端点的 socket 文件与活动连接都在
            // listener::serve 的收尾阶段释放。固定 sleep 既不能证明完成，也会制造
            // disable→enable 的竞态。
            let _ = running.task.await;
        }
        self.project_status().await
    }

    /// 进程窗口内已使用的外部提案创建次数（诊断用）。
    pub async fn quota_used(&self) -> u32 {
        self.quota.used()
    }
}

impl Default for AgentBrokerManager {
    fn default() -> Self {
        Self::new(env!("CARGO_PKG_VERSION"))
    }
}

#[cfg(test)]
mod tests {
    // 端点进程级独占测试需要把同步 guard 持有到异步生命周期操作完成；见
    // `listener::endpoint_test_guard`。
    #![allow(clippy::await_holding_lock)]

    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::sync::oneshot;

    #[derive(Default)]
    struct NoopApi {
        calls: AtomicU32,
    }

    #[async_trait]
    impl BrokerAgentApi for NoopApi {
        async fn context(&self) -> Result<ContextPayload, BrokerError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(json!({}))
        }

        async fn create_proposal(
            &self,
            _session_id: AgentSessionId,
            _request_id: AgentRequestId,
            _request: &AgentSettingsProposalCreateRequest,
        ) -> Result<AgentSettingsProposalDto, BrokerError> {
            Err(BrokerError::capability_unavailable())
        }
    }

    #[tokio::test]
    async fn manager_starts_disabled() {
        let manager = AgentBrokerManager::default();
        let status = manager.status().await;
        assert!(
            matches!(
                status,
                BrokerStatus::Disabled | BrokerStatus::Unavailable { .. }
            ),
            "默认必须是未开启，得到 {status:?}"
        );
        assert!(status.endpoint().is_none());
    }

    #[tokio::test]
    async fn enable_then_disable_round_trip() {
        // 端点进程级独占，同进程内的绑定用例必须串行。
        let _serialized = crate::agent_broker::listener::endpoint_test_guard();
        let manager = AgentBrokerManager::default();
        if !matches!(manager.status().await, BrokerStatus::Disabled) {
            // 极端环境（无 HOME/XDG）下端点不可解析：此时 enable 必须 fail closed。
            let error = manager
                .enable(Arc::new(NoopApi::default()))
                .await
                .unwrap_err();
            assert!(!error.message().is_empty());
            return;
        }

        let enabled = manager
            .enable(Arc::new(NoopApi::default()))
            .await
            .expect("端点可用时必须能开启");
        assert_eq!(enabled.label(), "listening");
        let endpoint = enabled.endpoint().expect("开启后必须有端点").to_owned();
        endpoint::validate_for_current_platform(&endpoint).expect("端点必须合法");

        // 幂等：重复 enable 返回同一端点。
        let again = manager.enable(Arc::new(NoopApi::default())).await.unwrap();
        assert_eq!(again.endpoint(), Some(endpoint.as_str()));

        let disabled = manager.disable().await;
        assert_eq!(disabled.label(), "disabled");
        assert!(disabled.endpoint().is_none());
    }

    #[tokio::test]
    async fn second_instance_fails_closed_with_endpoint_busy() {
        let _serialized = crate::agent_broker::listener::endpoint_test_guard();
        let first = AgentBrokerManager::default();
        if !matches!(first.status().await, BrokerStatus::Disabled) {
            return; // 端点不可解析时跳过（由上一个用例覆盖 fail closed）
        }
        let _ = first.enable(Arc::new(NoopApi::default())).await.unwrap();

        let second = AgentBrokerManager::default();
        let error = second
            .enable(Arc::new(NoopApi::default()))
            .await
            .expect_err("第二个实例不得接管端点");
        assert_eq!(error.code(), "HAVEN_BROKER_ENDPOINT_BUSY");
        assert!(!error.retryable());
        // 第二实例保持未监听，且第一实例仍在监听。
        assert!(second.status().await.endpoint().is_none());
        assert!(first.status().await.endpoint().is_some());

        let _ = first.disable().await;
    }

    /// 一个**绝不会**由 [`endpoint::resolve_endpoint`] 产出的端点：任何投影出这个端点的
    /// `listening` 都只可能来自被回收之前的陈旧条目，而不是一次真实绑定。
    fn stale_endpoint() -> BrokerEndpoint {
        BrokerEndpoint::NamedPipe(r"\\.\pipe\haven-broker-stale-test".to_owned())
    }

    /// 往 manager 里直接装一个 serve 条目（绕过 `enable`：端点能不能绑定不是这组用例要证明
    /// 的东西）。任务、相位都由调用方给出，因此投影与回收能被精确构造而不是靠 sleep 去赌。
    async fn install_serve(
        manager: &AgentBrokerManager,
        serving: watch::Receiver<bool>,
        task: JoinHandle<()>,
    ) {
        // shutdown receiver 随本函数一起丢弃，`disable` 的关闭广播因此得到 `Err`——它本来
        // 就忽略发送结果：收尾等的是 `task`，不是广播本身。
        let (shutdown, _shutdown_rx) = watch::channel(false);
        *manager.running.lock().await = Some(RunningBroker {
            endpoint: stale_endpoint(),
            shutdown,
            serving,
            task,
        });
    }

    /// 往 manager 里装一个**已经结束**的 serve 任务，相位**仍停在 `true`**。
    ///
    /// 不走 `enable`：这里唯一的前提是"JoinHandle 确实已经结束"，用 `is_finished` 自旋把它
    /// 变成事实而不是猜测。相位有意停在 `true`——它证明光看相位不够：任务已经结束同样不算
    /// 在监听，这一条只能由 `is_finished` 判出来。
    async fn install_finished_serve(manager: &AgentBrokerManager) {
        let task = tokio::spawn(async {});
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
        assert!(task.is_finished(), "前提：装进去的任务必须已经结束");

        let (_serving_tx, serving) = watch::channel(true);
        install_serve(manager, serving, task).await;
    }

    /// 假 serve 的两个开关。
    struct FakeServe {
        /// serve 相位发送端：置 `false` = "平台 serve 已退出 accept 循环，正在收尾连接"。
        serving: watch::Sender<bool>,
        /// 放行假 serve 的任务真正结束（也就是 drain 完成）。
        release: oneshot::Sender<()>,
    }

    /// [`install_finished_serve`] 的反面：装一个**不会自己结束**的 serve，相位从 `true`
    /// 起步，并把两个开关交回调用方。
    ///
    /// 用它把"serve 停止 accept 之后、任务真正结束之前"这段窗口变成可观测的事实——相位什么
    /// 时候翻、任务什么时候放行都由测试决定，窗口有多宽因此由测试决定，而不是靠 sleep 去赌。
    async fn install_controllable_serve(manager: &AgentBrokerManager) -> FakeServe {
        let (release, released) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = released.await;
        });
        let (serving, serving_rx) = watch::channel(true);
        install_serve(manager, serving_rx, task).await;
        FakeServe { serving, release }
    }

    /// 已结束的 serve 任务**不得**被投影成 `listening`，而且必须被回收。
    #[tokio::test]
    async fn finished_serve_is_not_projected_as_listening() {
        let manager = AgentBrokerManager::default();
        install_finished_serve(&manager).await;

        let status = manager.status().await;
        assert!(
            matches!(
                status,
                BrokerStatus::Disabled | BrokerStatus::Unavailable { .. }
            ),
            "已结束的 serve 不是 listening，得到 {status:?}"
        );
        assert!(status.endpoint().is_none());

        // 回收是彻底的：陈旧条目已经不在 `running` 里了。
        assert!(
            manager.running.lock().await.is_none(),
            "已结束的 serve 必须被回收，而不是继续留在 running 里"
        );
        // 再判一次同样不 listening（回收是幂等的）。
        assert!(manager.status().await.endpoint().is_none());
    }

    /// serve 已经停止 accept、任务却还在收尾连接：这段窗口**不**是 listening。
    ///
    /// 只看 `JoinHandle::is_finished` 会把它投影成 `listening`——平台的 accept 循环早已退出，
    /// 外部 Agent 这时连上不会被受理，而 `enable` 也会因此以为"已经在监听"而不去 bind。
    /// 用例不靠 sleep：相位由测试显式翻假，任务结束时机由测试精确放行。
    #[tokio::test]
    async fn stopped_accepting_serve_is_not_projected_as_listening() {
        let manager = Arc::new(AgentBrokerManager::default());
        let fake = install_controllable_serve(&manager).await;

        // 前提：相位为真的当下它**确实**被投影成 listening——否则下面的断言证明不了任何东西。
        let stale = stale_endpoint().display();
        assert_eq!(manager.status().await.endpoint(), Some(stale.as_str()));

        // 平台 serve 退出 accept 循环：相位翻假，但任务仍在跑（正在 abort/drain 连接）。
        fake.serving
            .send(false)
            .expect("manager 必须持有相位接收端");
        let task_still_running = {
            let guard = manager.running.lock().await;
            let broker = guard.as_ref().expect("条目此刻还在 running 里");
            !broker.task.is_finished()
        };
        assert!(task_still_running, "前提：假 serve 的任务此刻必须仍在跑");

        let projecting = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.status().await })
        };
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
        assert!(
            !projecting.is_finished(),
            "serve 停止 accept 之后 status 必须等它收尾完成，而不是立刻把没人 accept 的端点报成 listening"
        );

        // 放行假 serve：收尾完成，回收随之完成。
        let _ = fake.release.send(());
        let status = projecting.await.expect("status 不得 panic");
        assert!(
            status.endpoint().is_none(),
            "收尾之后不得再投影成 listening，得到 {status:?}"
        );
        assert!(
            manager.running.lock().await.is_none(),
            "收尾之后条目必须被回收，而不是继续留在 running 里"
        );
    }

    /// `enable` 不得把已结束的 serve 当成"已经在监听"而直接返回幂等成功。
    #[tokio::test]
    async fn enable_after_finished_serve_binds_instead_of_reusing_it() {
        let _serialized = crate::agent_broker::listener::endpoint_test_guard();
        let manager = AgentBrokerManager::default();
        install_finished_serve(&manager).await;

        match manager.enable(Arc::new(NoopApi::default())).await {
            Ok(status) => {
                assert_eq!(status.label(), "listening");
                let endpoint = status.endpoint().expect("开启后必须有端点");
                assert_ne!(
                    endpoint,
                    stale_endpoint().display(),
                    "必须是一次真实绑定，而不是把已结束的 serve 端点当成幂等成功"
                );
                endpoint::validate_for_current_platform(endpoint).expect("端点必须合法");
                assert_eq!(manager.disable().await.label(), "disabled");
            }
            Err(error) => {
                // 端点不可解析（或已被占用）：必须 fail closed，且不得报成"已监听"。
                assert!(!error.message().is_empty());
                assert!(manager.status().await.endpoint().is_none());
            }
        }
    }

    /// 一次启停操作在途时，`status` 必须在 `lifecycle` 上等它，而不是读到中间态。
    ///
    /// 判据是"没有完成"而不是"多久完成"：卡在 `lifecycle` 上的 future 无论被调度多少次都
    /// 不可能完成，所以让出多少次都不会让断言变得不确定；反过来，只要 `status` 不取这把
    /// 锁，它会在第一次被轮询时就跑完，断言立刻失败。因此这里既不需要 sleep，也没有竞态。
    #[tokio::test]
    async fn status_waits_for_in_flight_lifecycle_operation() {
        let manager = Arc::new(AgentBrokerManager::default());

        // 模拟一次在途的 enable/disable：它们有意跨 bind 或 serve 收尾的 await 持有这把锁。
        let lifecycle = manager.lifecycle.lock().await;

        let waiting = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.status().await })
        };
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
        assert!(
            !waiting.is_finished(),
            "启停操作在途时 status 不得越过 lifecycle 锁读到中间态"
        );

        // 放锁之后它必须能正常完成（证明刚才挡住它的确实是这把锁）。
        drop(lifecycle);
        let status = waiting.await.expect("status 不得 panic");
        assert!(
            matches!(
                status,
                BrokerStatus::Disabled | BrokerStatus::Unavailable { .. }
            ),
            "未开启过的 manager 只能是未开启或不可用，得到 {status:?}"
        );
        assert!(status.endpoint().is_none());
    }

    /// `disable`→`enable`：后者不得在"条目已取走、旧 serve 还没收尾"的窗口里自行 `bind`。
    ///
    /// 这个窗口正是一次误报 `ENDPOINT_BUSY` 的全部成因——`running` 已经空了，但端点还被旧
    /// serve 占着。用例不用 sleep 去赌窗口：假 serve 的退出时机由测试精确放行，于是
    /// "enable 被挡住"与"disable 收尾后 enable 才继续"都是确定性的。
    #[tokio::test]
    async fn disable_blocks_enable_until_shutdown_finishes() {
        let _serialized = crate::agent_broker::listener::endpoint_test_guard();
        let manager = Arc::new(AgentBrokerManager::default());
        if !matches!(manager.status().await, BrokerStatus::Disabled) {
            return; // 端点不可解析：enable 必然 fail closed，由其它用例覆盖
        }

        let fake = install_controllable_serve(&manager).await;

        let disabling = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.disable().await })
        };
        // 等 disable 真的进入收尾窗口：条目已经被取走，它正在等 serve 结束。这里等的是一个
        // 可观测的状态变化，不是一段时长。
        loop {
            let entry_still_there = manager.running.lock().await.is_some();
            if !entry_still_there {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            !disabling.is_finished(),
            "假 serve 未放行前 disable 不该结束，否则窗口没有被真正制造出来"
        );

        let enabling = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.enable(Arc::new(NoopApi::default())).await })
        };
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
        assert!(
            !enabling.is_finished(),
            "disable 收尾期间 enable 不得自行 bind——那会把 manager 自己的 shutdown 窗口误报成 ENDPOINT_BUSY"
        );

        // 放行假 serve：相位先翻（平台 serve 已停止 accept），任务随后结束，disable 收尾完成。
        let _ = fake.serving.send_replace(false);
        let _ = fake.release.send(());
        assert_eq!(
            disabling.await.expect("disable 不得 panic").label(),
            "disabled"
        );

        // 放行之后 enable 要么真的绑上端点，要么因端点被**别的进程**占用而 fail closed
        // （同机另有一个 Haven 在跑时）。本用例证明的是"窗口内它不会自行 bind"，两种结果
        // 都不削弱这一点。
        match enabling.await.expect("enable 不得 panic") {
            Ok(status) => {
                assert_eq!(status.label(), "listening");
                let endpoint = status.endpoint().expect("开启后必须有端点");
                assert_ne!(
                    endpoint,
                    stale_endpoint().display(),
                    "必须是一次真实绑定，而不是把假 serve 的端点当成幂等成功"
                );
                endpoint::validate_for_current_platform(endpoint).expect("端点必须合法");
                assert_eq!(manager.disable().await.label(), "disabled");
            }
            Err(error) => {
                assert_eq!(error.code(), "HAVEN_BROKER_ENDPOINT_BUSY");
                assert!(manager.status().await.endpoint().is_none());
            }
        }
    }
}
