//! SearchSourceService：`search_source_start` / `search_source_cancel`
//! （契约 §36.3 / CONTRACT-V02-SEARCH-CHANNEL-001）。
//!
//! 规则：
//! - `start` 只登记操作 + 立即返回；分发在独立后台任务跑（与 ScanService 同架构）。
//! - 同一归一化 query 已有运行中任务 → 返回既有 operationId/taskId +
//!   `alreadyRunning=true`；不同 query 不合并，旧结果由前端按 operationId 丢弃。
//! - 本地结果不等待慢 Source；单 Source 失败发 `warning` 后继续；
//!   Terminal Event 只能出现一次（completed | cancelled | failed）。
//! - `cancel` 幂等：运行中 → 设置取消标志；已终态 → alreadyTerminal；
//!   未知 operationId → `RESOURCE_NOT_FOUND`。
//! - 已启用来源无注册参与者时跳过该来源（组合缺口，不是来源失败）；固定公开
//!   metadata Provider 不需要用户端点，CMS10/M3U/OPDS 等资源来源仍需端点。
//! - 每个**真实**搜索观测（成功或失败）都写入来源注册表的健康事实：被跳过的来源
//!   （无参与者、分类不匹配、未配置端点）与取消的查询都不是探测，不写健康；
//!   更早开始的检查即使更晚完成，也不会回退更晚开始的结果（见 `source_registry::health`）。
//!
//! 依赖方向：application 层只依赖 domain 契约 + wire DTO；
//! `SearchSourceParticipant` / `SearchEventSink` 端口由后续批次与 src-tauri 实现。

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::task::JoinSet;

use haven_common::{AppError, ErrorKind, UtcMillis};

use crate::mapper::utc_millis_to_rfc3339;
use crate::services::source_registry::{
    SourceHealthRecorder, SourceRegistryService, SourceSearchObservation,
};
use crate::wire::{
    SearchSourceEvent, SearchSourceEventData, SearchSourceEventKind, SearchSourceStartRequest,
    SearchStartResultDto, WorkCardDto,
};

/// 查询长度上限（契约 §36.3 输入校验）。
pub const MAX_QUERY_LEN: usize = 200;
/// 单来源结果上限（契约 §36.3 输入校验）。
pub const MAX_LIMIT_PER_SOURCE: u32 = 50;
/// 未指定时的默认单来源上限。
pub const DEFAULT_LIMIT_PER_SOURCE: u32 = 20;

/// 参与渐进式搜索的来源端口。由固定公开 Metadata/CMS10/M3U/OPDS 适配器实现；
/// `search` 在分页边界轮询 `is_cancelled` 协作退出。
#[async_trait::async_trait]
pub trait SearchSourceParticipant: Send + Sync {
    /// 来源 ID（必须来自内置目录；未注册参与者按组合缺口跳过）。
    fn source_id(&self) -> &str;
    /// 前缀路由（V2-H 收尾批次）：返回 Some(前缀) 时该参与者承接所有以此前缀
    /// 开头的 sourceId（如自定义源 `custom_` 家族共享一个动态参与者实例）。
    /// 默认 None = 仅精确匹配自身 source_id。
    fn id_prefix(&self) -> Option<&str> {
        None
    }
    /// 分类门控（V2-H2）：请求分类与本来源不匹配时跳过分发（组合缺口，非告警）。
    /// 默认全兼容，保持既有参与者不变。
    fn supports_category(&self, _category: Option<crate::wire::QueryCategory>) -> bool {
        true
    }
    /// 执行单来源搜索。返回候选卡片投影（去重键见契约 §36.1）。
    /// 取消检查器要求 Send+Sync：引用需跨 await 存活于 Send future 内。
    async fn search(
        &self,
        query: &str,
        limit: u32,
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<WorkCardDto>, AppError>;

    /// V2-H 收尾批次：按 dispatched sourceId 分发（前缀路由参与者按实际 sourceId 寻址）。
    /// 默认实现委托给 `search`（要求 dispatched 已匹配自身 id/前缀）。
    async fn search_for(
        &self,
        dispatched_id: &str,
        query: &str,
        limit: u32,
        is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<WorkCardDto>, AppError> {
        let matches = dispatched_id == self.source_id()
            || self
                .id_prefix()
                .is_some_and(|prefix| dispatched_id.starts_with(prefix));
        if matches {
            self.search(query, limit, is_cancelled).await
        } else {
            Ok(Vec::new())
        }
    }
}

/// 搜索事件 Sink 端口：src-tauri 实现为 Tauri Channel 发送。
pub trait SearchEventSink: Send + Sync {
    fn emit_search_event(&self, event: SearchSourceEvent);
}

/// `search_source_cancel` 结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchCancelOutcome {
    /// 任务正在运行，已设置取消标志。
    Cancelled,
    /// 任务已结束（completed/cancelled/failed 任一），返回真实终态事实。
    AlreadyTerminal,
}

/// 运行中的搜索操作。
struct RunningOp {
    operation_id: String,
    task_id: String,
    cancel: Arc<AtomicBool>,
    /// 搜索结果句柄缓存（按 source_result 到达顺序；供导入定位，容量受限）。
    ///
    /// 这里必须缓存每一张展示卡片，而不只是可导入卡片：前端的
    /// `operationId + index` 索引对应的是搜索结果的展示顺序。metadata-only
    /// 卡片仍会在 `SourceImportService` 入口被明确拒绝，但不能从缓存中省略，
    /// 否则后面的可导入卡片会发生索引漂移，用户点击 A 实际导入 B 或收到
    /// `RESOURCE_NOT_FOUND`。
    candidates: Arc<Mutex<Vec<String>>>,
}

/// 操作监督者：运行表按归一化 query 索引（幂等合并）；终态记录按 operationId 索引。
struct Supervisor {
    running_by_query: Mutex<HashMap<String, Arc<RunningOp>>>,
    terminal: Mutex<HashMap<String, TerminalRecord>>,
}

/// 已结束操作的终态记录（cancel 返回真实终态；候选缓存供导入）。
struct TerminalRecord {
    inserted_at: std::time::Instant,
    candidates: Vec<String>,
}

const TERMINAL_HISTORY_CAP: usize = 256;

impl Supervisor {
    fn new() -> Self {
        Self {
            running_by_query: Mutex::new(HashMap::new()),
            terminal: Mutex::new(HashMap::new()),
        }
    }

    /// 幂等登记：同 query 已有运行任务 → AlreadyRunning；否则插入并返回新身份。
    fn try_register(&self, query_key: &str) -> RegisterResult {
        let mut running = self
            .running_by_query
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(op) = running.get(query_key) {
            return RegisterResult::AlreadyRunning(op.operation_id.clone(), op.task_id.clone());
        }
        let op = Arc::new(RunningOp {
            operation_id: new_id("op-search"),
            task_id: new_id("task-search"),
            cancel: Arc::new(AtomicBool::new(false)),
            candidates: Arc::new(Mutex::new(Vec::new())),
        });
        running.insert(query_key.to_owned(), op.clone());
        RegisterResult::Registered(
            op.operation_id.clone(),
            op.task_id.clone(),
            op.cancel.clone(),
            op.candidates.clone(),
        )
    }

    /// 登记后、后台任务启动前的失败清理（仅移除本操作占位）。
    fn abort(&self, query_key: &str, operation_id: &str) {
        let mut running = self
            .running_by_query
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(op) = running.get(query_key) {
            if op.operation_id == operation_id {
                running.remove(query_key);
            }
        }
    }

    fn remove(&self, query_key: &str, operation_id: &str, candidates: Vec<String>) {
        let mut running = self
            .running_by_query
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // 条件删除：仅当仍是本操作占位时移除（防误删并发同键新任务——理论上
        // try_register 的互斥使该窗口不存在，防御性保留）。
        if let Some(op) = running.get(query_key) {
            if op.operation_id == operation_id {
                running.remove(query_key);
            }
        }
        drop(running);
        let mut terminal = self.terminal.lock().unwrap_or_else(|e| e.into_inner());
        if terminal.len() >= TERMINAL_HISTORY_CAP {
            if let Some(oldest) = terminal
                .iter()
                .min_by_key(|(_, at)| at.inserted_at)
                .map(|(k, _)| k.clone())
            {
                terminal.remove(&oldest);
            }
        }
        terminal.insert(
            operation_id.to_owned(),
            TerminalRecord {
                inserted_at: std::time::Instant::now(),
                candidates,
            },
        );
    }

    /// 按序号取候选外部 ID（运行中或已终态均可；导入用）。
    pub fn candidate_external_id(&self, operation_id: &str, index: u32) -> Option<String> {
        let running = self
            .running_by_query
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for op in running.values() {
            if op.operation_id == operation_id {
                let candidates = op.candidates.lock().unwrap_or_else(|e| e.into_inner());
                return candidates.get(index as usize).cloned();
            }
        }
        drop(running);
        let terminal = self.terminal.lock().unwrap_or_else(|e| e.into_inner());
        terminal
            .get(operation_id)
            .and_then(|record| record.candidates.get(index as usize))
            .cloned()
    }

    /// cancel by operationId：运行中 → 设置取消标志；已终态 → AlreadyTerminal；
    /// 未知 → None。
    fn cancel(&self, operation_id: &str) -> Option<SearchCancelOutcome> {
        let running = self
            .running_by_query
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for op in running.values() {
            if op.operation_id == operation_id {
                op.cancel.store(true, Ordering::Release);
                return Some(SearchCancelOutcome::Cancelled);
            }
        }
        drop(running);
        let terminal = self.terminal.lock().unwrap_or_else(|e| e.into_inner());
        if terminal.contains_key(operation_id) {
            Some(SearchCancelOutcome::AlreadyTerminal)
        } else {
            None
        }
    }
}

enum RegisterResult {
    Registered(String, String, Arc<AtomicBool>, Arc<Mutex<Vec<String>>>),
    AlreadyRunning(String, String),
}

/// 后台事件发射器：sequence 分配与投递在同一 Mutex 内，保证同 operation 内严格递增。
struct TaskEmitter {
    operation_id: String,
    sink: Arc<dyn SearchEventSink>,
    emit_lock: Mutex<u32>,
    cancel: Arc<AtomicBool>,
}

impl TaskEmitter {
    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    fn emit(
        &self,
        kind: SearchSourceEventKind,
        source_id: Option<String>,
        works: Vec<WorkCardDto>,
        code: Option<String>,
        message: Option<String>,
    ) {
        let mut seq = self.emit_lock.lock().unwrap_or_else(|e| e.into_inner());
        *seq += 1;
        let event = SearchSourceEvent {
            operation_id: self.operation_id.clone(),
            sequence: *seq,
            at: utc_millis_to_rfc3339(UtcMillis::now()),
            kind,
            data: SearchSourceEventData {
                source_id,
                works,
                code,
                message,
            },
        };
        self.sink.emit_search_event(event);
    }
}

/// `search_source_start` / `search_source_cancel` 服务。
#[derive(Clone)]
pub struct SearchSourceService {
    registry: SourceRegistryService,
    participants: Vec<Arc<dyn SearchSourceParticipant>>,
    sink: Arc<dyn SearchEventSink>,
    supervisor: Arc<Supervisor>,
    /// 真实搜索观测的健康落地端口（无写入通道时注册表侧是无操作）。
    health: Option<Arc<dyn SourceHealthRecorder>>,
}

impl SearchSourceService {
    pub fn new(
        registry: SourceRegistryService,
        participants: Vec<Arc<dyn SearchSourceParticipant>>,
        sink: Arc<dyn SearchEventSink>,
    ) -> Self {
        let health: Arc<dyn SourceHealthRecorder> = Arc::new(registry.clone());
        Self {
            registry,
            participants,
            sink,
            supervisor: Arc::new(Supervisor::new()),
            health: Some(health),
        }
    }

    /// `search_source_start`：校验 → 幂等登记 → 立即返回；分发在后台跑。
    pub async fn start(
        &self,
        request: SearchSourceStartRequest,
    ) -> Result<SearchStartResultDto, AppError> {
        let query = request.query.trim();
        if query.is_empty() {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "搜索词不能为空",
                false,
            ));
        }
        if query.chars().count() > MAX_QUERY_LEN {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "搜索词过长（不超过 200 字符）",
                false,
            ));
        }
        if let Some(limit) = request.limit_per_source {
            if limit == 0 || limit > MAX_LIMIT_PER_SOURCE {
                return Err(AppError::new(
                    "INVALID_ARGUMENT",
                    ErrorKind::Validation,
                    "单来源数量超出允许范围",
                    false,
                ));
            }
        }

        // 归一化合并键：trim 后的 query + 分类 + 上限（不同参数不合并）。
        let query_key = format!(
            "{query}\u{1}{:?}\u{1}{}",
            request.category,
            request.limit_per_source.unwrap_or(DEFAULT_LIMIT_PER_SOURCE)
        );

        let (operation_id, task_id, already_running, cancel, candidates) =
            match self.supervisor.try_register(&query_key) {
                RegisterResult::AlreadyRunning(op, task) => (op, task, true, None, None),
                RegisterResult::Registered(op, task, cancel, candidates) => {
                    (op, task, false, Some(cancel), Some(candidates))
                }
            };

        if already_running {
            return Ok(SearchStartResultDto {
                operation_id,
                task_id,
                already_running: true,
            });
        }
        let cancel = cancel.expect("新登记必须返回取消标志");
        let candidates = candidates.expect("新登记必须返回候选缓存");

        // 启用时点的分发快照（含健康/端点/配置版本，供并行优先级排序；搜索期间变更不并入本次任务）。
        // 注册表读取失败 → 清理登记并以稳定错误响应（不产生 Channel 会话）。
        let snapshot = match self.registry.enabled_sources_snapshot().await {
            Ok(v) => v,
            Err(err) => {
                self.supervisor.abort(&query_key, &operation_id);
                return Err(err);
            }
        };

        let emitter = Arc::new(TaskEmitter {
            operation_id: operation_id.clone(),
            sink: self.sink.clone(),
            emit_lock: Mutex::new(0),
            cancel: cancel.clone(),
        });

        // started 强制先发（订阅端先于任何结果看到会话开始）。
        emitter.emit(SearchSourceEventKind::Started, None, Vec::new(), None, None);

        let participants = self.participants.clone();
        let supervisor = self.supervisor.clone();
        let dispatch_query = DispatchQuery {
            // 合并键、事件与参与者都使用同一个 trim 过的搜索词。
            text: query.to_owned(),
            category: request.category,
            limit: request.limit_per_source.unwrap_or(DEFAULT_LIMIT_PER_SOURCE),
        };
        let op_for_spawn = operation_id.clone();
        let enabled_sources = snapshot.sources;
        let health = HealthContext {
            revisions: snapshot.revisions,
            recorder: self.health.clone(),
        };

        // 双层 spawn（与 ScanService 同模式）：内层 panic 由 JoinHandle 捕获，
        // 外层保证终态事件 + 终态登记统一执行。
        let supervisor_query_key = query_key.clone();
        tokio::spawn(async move {
            let inner_emitter = emitter.clone();
            let inner_candidates = candidates.clone();
            let inner_health = health.clone();
            let inner = tokio::spawn(async move {
                run_dispatch(
                    participants,
                    dispatch_query,
                    enabled_sources,
                    inner_emitter,
                    inner_candidates,
                    inner_health,
                )
                .await
            });
            let terminal_kind = match inner.await {
                Ok(Some(kind)) => kind,
                Ok(None) => SearchSourceEventKind::Completed,
                Err(_) => SearchSourceEventKind::Failed,
            };
            match terminal_kind {
                SearchSourceEventKind::Failed => {
                    emitter.emit(
                        SearchSourceEventKind::Failed,
                        None,
                        Vec::new(),
                        Some("INTERNAL_ERROR".to_owned()),
                        None,
                    );
                }
                kind => emitter.emit(kind, None, Vec::new(), None, None),
            }
            let final_candidates = candidates.lock().unwrap_or_else(|e| e.into_inner()).clone();
            supervisor.remove(&supervisor_query_key, &op_for_spawn, final_candidates);
        });

        Ok(SearchStartResultDto {
            operation_id,
            task_id,
            already_running: false,
        })
    }

    /// `search_source_cancel`：幂等；未知 operationId → RESOURCE_NOT_FOUND。
    pub fn cancel(&self, operation_id: &str) -> Result<SearchCancelOutcome, AppError> {
        match self.supervisor.cancel(operation_id) {
            Some(outcome) => Ok(outcome),
            None => Err(AppError::new(
                "RESOURCE_NOT_FOUND",
                ErrorKind::NotFound,
                "搜索操作不存在",
                false,
            )),
        }
    }

    /// 按操作 + 序号取候选外部 ID（导入定位；未知返回 None）。
    pub fn candidate_external_id(&self, operation_id: &str, index: u32) -> Option<String> {
        self.supervisor.candidate_external_id(operation_id, index)
    }
}

// ---------- 并行调度（V2-H 后续：全开均衡） ----------

const MAX_CONCURRENCY: usize = 5;
const SOURCE_TIMEOUT: Duration = Duration::from_secs(5);
const CANCEL_POLL: Duration = Duration::from_millis(25);

#[allow(dead_code)]
struct ScheduledSource {
    source_id: String,
    participant: Arc<dyn SearchSourceParticipant>,
    priority: u64,
    descriptor: crate::wire::SourceDescriptorDto,
}

/// 一次来源调用的探测事实：开始时间、真实耗时，以及它是否真的发出了请求。
struct SourceProbe {
    /// 分发时刻即「检查开始」时刻：观测按它排序，迟到的旧检查不得回退新结果。
    started_at: UtcMillis,
    latency_ms: u64,
    /// 分发时是否已经取消：取消后参与者返回的空结果不是一次真实探测，不能记成成功。
    probed: bool,
}

enum SourceOutcomeKind {
    Result {
        source_id: String,
        works: Vec<WorkCardDto>,
    },
    Warning {
        source_id: String,
        code: String,
        message: String,
    },
}

struct SourceOutcome {
    kind: SourceOutcomeKind,
    probe: SourceProbe,
}

/// 分发期的健康观测上下文：分发时读到的来源配置版本 + 观测落地端口。
///
/// 版本号随观测进入健康写入事务：分发之后发生的端点/凭据变更（含 A→B→A）会让这次
/// 观测作废，而不是把新端点标成健康。
#[derive(Clone)]
struct HealthContext {
    revisions: BTreeMap<String, String>,
    recorder: Option<Arc<dyn SourceHealthRecorder>>,
}

impl HealthContext {
    /// 构造一次真实搜索观测。
    ///
    /// 取消的查询与分发时已取消的调用都不是探测：既不记成功也不记失败。
    fn observation(
        &self,
        source_id: &str,
        probe: &SourceProbe,
        succeeded: bool,
        emitter: &TaskEmitter,
    ) -> Option<SourceSearchObservation> {
        if !probe.probed || emitter.is_cancelled() {
            return None;
        }
        Some(SourceSearchObservation {
            source_id: source_id.to_owned(),
            source_revision: self.revisions.get(source_id).cloned(),
            started_at: probe.started_at,
            succeeded,
            latency_ms: probe.latency_ms,
        })
    }

    /// 一轮搜索只发起一次健康写入，避免每个来源分别排队争用数据库写锁。
    async fn record_all(&self, observations: Vec<SourceSearchObservation>, emitter: &TaskEmitter) {
        if observations.is_empty() || emitter.is_cancelled() {
            return;
        }
        let Some(recorder) = &self.recorder else {
            return;
        };
        // 健康是诊断事实：写入失败不得把一次已经完成的搜索变成失败。
        let _ = recorder.record_search_observations(observations).await;
    }
}

fn resolve_participant(
    source_id: &str,
    participants: &[Arc<dyn SearchSourceParticipant>],
) -> Option<Arc<dyn SearchSourceParticipant>> {
    if let Some(p) = participants.iter().find(|p| p.source_id() == source_id) {
        return Some(Arc::clone(p));
    }
    participants
        .iter()
        .filter_map(|p| {
            let prefix = p.id_prefix()?;
            source_id
                .starts_with(prefix)
                .then_some((prefix.len(), Arc::clone(p)))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, p)| p)
}

fn priority_of(source: &crate::wire::SourceDescriptorDto) -> u64 {
    use crate::wire::SourceHealthDto;
    let latency = source.latency_ms.unwrap_or(1000).clamp(50, 5000);
    let success_bp: u64 = match source.success_rate {
        Some(v) => ((v.clamp(0.05, 1.0) * 10_000.0) as u64).max(500),
        None => match source.health {
            SourceHealthDto::Ok => 9000,
            SourceHealthDto::Unknown => 7000,
            SourceHealthDto::Degraded => 4500,
            SourceHealthDto::Down => 2000,
        },
    };
    let health_factor_bp = match source.health {
        SourceHealthDto::Ok => 10_000,
        SourceHealthDto::Unknown => 12_500,
        SourceHealthDto::Degraded => 20_000,
        SourceHealthDto::Down => 40_000,
    };
    latency
        .saturating_mul(health_factor_bp)
        .saturating_mul(10_000)
        / success_bp.max(500)
}

/// 一次分发的查询输入。
///
/// 分发循环里的每个参与者调用都只依赖这三个值；把它们收成一份值对象，让
/// `run_dispatch` 的参数表不再随调度选项增长。字段语义与之前的位置参数完全一致：
/// `text` 是已 trim 的搜索词（合并键、事件与参与者共用同一个字符串），`category` 是分类
/// 门控用的原始请求值（`None` 表示不限制），`limit` 是已经取过 [`DEFAULT_LIMIT_PER_SOURCE`]
/// 默认值的上限。
struct DispatchQuery {
    text: String,
    category: Option<crate::wire::QueryCategory>,
    limit: u32,
}

/// 分发循环（并行均衡版）。返回值语义同前：`Some(kind)` 为取消/失败提前收束，`None` 为全部完成。
async fn run_dispatch(
    participants: Vec<Arc<dyn SearchSourceParticipant>>,
    query: DispatchQuery,
    enabled_sources: Vec<crate::wire::SourceDescriptorDto>,
    emitter: Arc<TaskEmitter>,
    candidates: Arc<Mutex<Vec<String>>>,
    health: HealthContext,
) -> Option<SearchSourceEventKind> {
    // 1. 组装调度队列：解析 participant、分类门控、端点门控
    let mut jobs: Vec<ScheduledSource> = Vec::new();
    for source in enabled_sources {
        let Some(participant) = resolve_participant(&source.source_id, &participants) else {
            // 组合缺口：启用但无参与者，跳过不告警
            continue;
        };
        if !participant.supports_category(query.category) {
            continue;
        }
        if source_requires_endpoint(&source) && !source.endpoint_configured {
            emitter.emit(
                SearchSourceEventKind::Warning,
                Some(source.source_id.clone()),
                Vec::new(),
                Some("SOURCE_NOT_CONFIGURED".to_owned()),
                Some(source_not_configured_message(&source.source_id).to_owned()),
            );
            continue;
        }
        let priority = priority_of(&source);
        jobs.push(ScheduledSource {
            source_id: source.source_id.clone(),
            participant,
            priority,
            descriptor: source,
        });
    }

    jobs.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.source_id.cmp(&b.source_id))
    });

    let mut pending: VecDeque<ScheduledSource> = jobs.into();
    let mut running: JoinSet<SourceOutcome> = JoinSet::new();
    // JoinError 不携带业务身份；保留任务与来源的对应关系才能报告具体失败来源。
    let mut running_sources = HashMap::new();
    let mut health_observations = Vec::new();

    let spawn_next = |running: &mut JoinSet<SourceOutcome>,
                      running_sources: &mut HashMap<
        tokio::task::Id,
        (String, UtcMillis, std::time::Instant),
    >,
                      pending: &mut VecDeque<ScheduledSource>,
                      query: &DispatchQuery,
                      cancel: &Arc<AtomicBool>| {
        while running.len() < MAX_CONCURRENCY {
            let Some(job) = pending.pop_front() else {
                break;
            };
            let limit = query.limit;
            let query = query.text.clone();
            let cancel = Arc::clone(cancel);
            let participant = job.participant.clone();
            let source_id = job.source_id.clone();
            let failure_context = (
                source_id.clone(),
                UtcMillis::now(),
                std::time::Instant::now(),
            );
            let task = running.spawn(async move {
                let is_cancelled = || cancel.load(Ordering::Acquire);
                let probe_started_at = UtcMillis::now();
                let probed = !is_cancelled();
                let started = std::time::Instant::now();
                let call = participant.search_for(&source_id, &query, limit, &is_cancelled);
                let (kind, latency_ms) = match tokio::time::timeout(SOURCE_TIMEOUT, call).await {
                    Ok(Ok(works)) => (
                        SourceOutcomeKind::Result {
                            source_id: source_id.clone(),
                            works,
                        },
                        started.elapsed().as_millis() as u64,
                    ),
                    Ok(Err(err)) => (
                        SourceOutcomeKind::Warning {
                            source_id: source_id.clone(),
                            code: err.code().as_str().to_owned(),
                            message: err.user_message().to_owned(),
                        },
                        started.elapsed().as_millis() as u64,
                    ),
                    Err(_) => (
                        SourceOutcomeKind::Warning {
                            source_id: source_id.clone(),
                            code: "SOURCE_UNAVAILABLE".to_owned(),
                            message: "该搜索源响应超时。".to_owned(),
                        },
                        SOURCE_TIMEOUT.as_millis() as u64,
                    ),
                };
                SourceOutcome {
                    kind,
                    probe: SourceProbe {
                        started_at: probe_started_at,
                        latency_ms,
                        probed,
                    },
                }
            });
            running_sources.insert(task.id(), failure_context);
        }
    };

    // 初始填充
    {
        let cancel = emitter.cancel.clone();
        spawn_next(
            &mut running,
            &mut running_sources,
            &mut pending,
            &query,
            &cancel,
        );
    }

    enum End {
        Completed,
        Cancelled,
    }

    let end = loop {
        if emitter.is_cancelled() {
            running.abort_all();
            while running.join_next().await.is_some() {}
            break End::Cancelled;
        }

        if running.is_empty() {
            if pending.is_empty() {
                break End::Completed;
            }
            let cancel = emitter.cancel.clone();
            spawn_next(
                &mut running,
                &mut running_sources,
                &mut pending,
                &query,
                &cancel,
            );
            continue;
        }

        let joined = tokio::time::timeout(CANCEL_POLL, running.join_next_with_id()).await;
        let Some(joined) = (match joined {
            Err(_) => continue,
            Ok(v) => v,
        }) else {
            if pending.is_empty() {
                break End::Completed;
            }
            let cancel = emitter.cancel.clone();
            spawn_next(
                &mut running,
                &mut running_sources,
                &mut pending,
                &query,
                &cancel,
            );
            continue;
        };

        if emitter.is_cancelled() {
            running.abort_all();
            while running.join_next().await.is_some() {}
            break End::Cancelled;
        }

        match joined {
            Ok((task_id, SourceOutcome { kind, probe })) => {
                running_sources.remove(&task_id);
                // 先补位，避免 sink 慢时浪费并发
                {
                    let cancel = emitter.cancel.clone();
                    spawn_next(
                        &mut running,
                        &mut running_sources,
                        &mut pending,
                        &query,
                        &cancel,
                    );
                }
                match kind {
                    SourceOutcomeKind::Result { source_id, works } => {
                        {
                            // `SearchSourceEventData.works` 与此缓存共享同一批次、同一
                            // 到达顺序。缓存所有展示句柄，确保前端使用的 index 不会因
                            // metadata-only 结果被跳过而错位；导入能力仍由
                            // `SourceImportService::import_candidate` 最终裁决。
                            cache_displayed_candidates(&candidates, &works);
                        }
                        emitter.emit(
                            SearchSourceEventKind::SourceResult,
                            Some(source_id.clone()),
                            works,
                            None,
                            None,
                        );
                        // 空成功响应也是一次真实成功的探测。
                        if let Some(observation) =
                            health.observation(&source_id, &probe, true, &emitter)
                        {
                            health_observations.push(observation);
                        }
                    }
                    SourceOutcomeKind::Warning {
                        source_id,
                        code,
                        message,
                    } => {
                        emitter.emit(
                            SearchSourceEventKind::Warning,
                            Some(source_id.clone()),
                            Vec::new(),
                            Some(code),
                            Some(message),
                        );
                        if let Some(observation) =
                            health.observation(&source_id, &probe, false, &emitter)
                        {
                            health_observations.push(observation);
                        }
                    }
                }
            }
            Err(join_err) => {
                if let Some((source_id, started_at, started)) =
                    running_sources.remove(&join_err.id())
                {
                    emitter.emit(
                        SearchSourceEventKind::Warning,
                        Some(source_id.clone()),
                        Vec::new(),
                        Some("SOURCE_UNAVAILABLE".to_owned()),
                        Some("该搜索源处理结果时发生异常，请稍后重试。".to_owned()),
                    );
                    let probe = SourceProbe {
                        started_at,
                        latency_ms: started.elapsed().as_millis() as u64,
                        probed: true,
                    };
                    if let Some(observation) =
                        health.observation(&source_id, &probe, false, &emitter)
                    {
                        health_observations.push(observation);
                    }
                }
            }
        }
    };

    match end {
        End::Completed => {
            health.record_all(health_observations, &emitter).await;
            None
        }
        End::Cancelled => Some(SearchSourceEventKind::Cancelled),
    }
}

/// Keep the import lookup index identical to the order in which cards are
/// delivered in `SearchSourceEventData.works`.
///
/// Search results intentionally include metadata-only cards. They are useful
/// to the user, but `SourceImportService` will reject their handles with a
/// stable unsupported error. Omitting them here would make the UI's display
/// index refer to a different array and could import the wrong work.
fn cache_displayed_candidates(candidates: &Arc<Mutex<Vec<String>>>, works: &[WorkCardDto]) {
    let mut cache = candidates.lock().unwrap_or_else(|e| e.into_inner());
    cache.extend(works.iter().map(|card| card.work_id.clone()));
}

/// 只有需要访问用户配置端点的聚合来源才要求 `endpointConfigured`。
/// 能力（search/onlineRead/offlineDownload）描述可执行动作，不能用来推断
/// 端点归属：MangaDex、arXiv 等固定 Provider 同样可能支持在线读取或下载，
/// 但它们的地址由后端拥有，不应因为 Wire 中没有端点而被跳过。
fn source_requires_endpoint(source: &crate::wire::SourceDescriptorDto) -> bool {
    matches!(source.source_id.as_str(), "cms10" | "m3u") || source.source_id.starts_with("custom_")
}

fn source_not_configured_message(source_id: &str) -> &'static str {
    match source_id {
        "cms10" => "CMS10 需要先到“设置 → 来源”配置采集接口地址。",
        "m3u" => "M3U 需要先到“设置 → 来源”配置播放列表地址。",
        _ => "该搜索源尚未完成配置。",
    }
}

/// 生成 opaque id（时间戳 + 纳秒后缀，保证唯一性）。
fn new_id(prefix: &str) -> String {
    format!(
        "{}-{:016x}-{:x}",
        prefix,
        UtcMillis::now().0 as u64,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn query_limit_counts_unicode_characters_instead_of_bytes() {
        let db = Arc::new(haven_infrastructure::Db::open_in_memory().unwrap());
        let repos = Arc::new(haven_infrastructure::db::repos::SqliteRepositories::new(db));
        let service = SearchSourceService::new(
            SourceRegistryService::new(repos),
            vec![],
            Arc::new(CaptureSink {
                events: Mutex::new(Vec::new()),
            }),
        );
        for query in ["书".repeat(200), "📚".repeat(200), "a".repeat(200)] {
            assert!(
                service
                    .start(SearchSourceStartRequest {
                        query,
                        category: None,
                        limit_per_source: None
                    })
                    .await
                    .is_ok()
            );
        }
        for query in ["书".repeat(201), "📚".repeat(201), "a".repeat(201)] {
            let error = service
                .start(SearchSourceStartRequest {
                    query,
                    category: None,
                    limit_per_source: None,
                })
                .await
                .unwrap_err();
            assert_eq!(error.code().as_str(), "INVALID_ARGUMENT");
            assert!(error.user_message().contains("200"));
        }
    }

    #[tokio::test]
    async fn a_panicking_participant_emits_a_source_warning_and_dispatch_continues() {
        struct PanickingParticipant;
        #[async_trait::async_trait]
        impl SearchSourceParticipant for PanickingParticipant {
            fn source_id(&self) -> &str {
                "tvmaze"
            }
            async fn search(
                &self,
                _: &str,
                _: u32,
                _: &(dyn Fn() -> bool + Send + Sync),
            ) -> Result<Vec<WorkCardDto>, AppError> {
                panic!("synthetic provider panic");
            }
        }
        let db = Arc::new(haven_infrastructure::Db::open_in_memory().unwrap());
        let repos = Arc::new(haven_infrastructure::db::repos::SqliteRepositories::new(db));
        let enabled = SourceRegistryService::new(repos)
            .list()
            .await
            .unwrap()
            .sources
            .into_iter()
            .filter(|s| s.source_id == "tvmaze" || s.source_id == "bangumi")
            .collect();
        let sink = Arc::new(CaptureSink {
            events: Mutex::new(Vec::new()),
        });
        let emitter = Arc::new(TaskEmitter {
            operation_id: "op-panic".into(),
            sink: sink.clone(),
            emit_lock: Mutex::new(0),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        let healthy_calls = Arc::new(AtomicUsize::new(0));
        let participants: Vec<Arc<dyn SearchSourceParticipant>> = vec![
            Arc::new(PanickingParticipant),
            Arc::new(ImmediateParticipant {
                id: "bangumi".into(),
                calls: healthy_calls.clone(),
            }),
        ];
        let terminal = run_dispatch(
            participants,
            DispatchQuery {
                text: "book".into(),
                category: None,
                limit: 5,
            },
            enabled,
            emitter,
            Arc::new(Mutex::new(Vec::new())),
            no_health(),
        )
        .await;
        assert_eq!(terminal, None);
        assert_eq!(healthy_calls.load(Ordering::SeqCst), 1);
        let events = sink.events.lock().unwrap();
        let warnings: Vec<_> = events
            .iter()
            .filter(|e| e.kind == SearchSourceEventKind::Warning)
            .collect();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].data.source_id.as_deref(), Some("tvmaze"));
        assert_eq!(warnings[0].data.code.as_deref(), Some("SOURCE_UNAVAILABLE"));
        assert!(
            events
                .iter()
                .any(|e| e.kind == SearchSourceEventKind::SourceResult
                    && e.data.source_id.as_deref() == Some("bangumi"))
        );
    }

    struct FakeParticipant {
        id: String,
        delay: Duration,
        current: Arc<AtomicUsize>,
        max: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl SearchSourceParticipant for FakeParticipant {
        fn source_id(&self) -> &str {
            &self.id
        }

        async fn search(
            &self,
            _query: &str,
            _limit: u32,
            is_cancelled: &(dyn Fn() -> bool + Send + Sync),
        ) -> Result<Vec<WorkCardDto>, AppError> {
            let cur = self.current.fetch_add(1, Ordering::SeqCst) + 1;
            // record max
            loop {
                let max = self.max.load(Ordering::SeqCst);
                if cur <= max
                    || self
                        .max
                        .compare_exchange(max, cur, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    break;
                }
            }
            // cooperative cancel check during sleep
            let mut elapsed = Duration::from_millis(0);
            while elapsed < self.delay {
                if is_cancelled() {
                    self.current.fetch_sub(1, Ordering::SeqCst);
                    return Ok(Vec::new());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
                elapsed += Duration::from_millis(10);
            }
            self.current.fetch_sub(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
    }

    /// 事件捕获下沉。健康用例（`health_tests`）复用同一份实现，避免两套事件断言分叉。
    pub(super) struct CaptureSink {
        pub(super) events: Mutex<Vec<SearchSourceEvent>>,
    }

    impl SearchEventSink for CaptureSink {
        fn emit_search_event(&self, event: SearchSourceEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    pub(super) fn card(work_id: &str, title: &str) -> WorkCardDto {
        WorkCardDto {
            work_id: work_id.to_owned(),
            title: title.to_owned(),
            original_title: None,
            description: None,
            categories: vec![crate::wire::ContentCategory::Comic],
            available_media_types: vec![crate::wire::MediaTypeDto::Comic],
            poster_uri: None,
            backdrop_uri: None,
            release_year: None,
            rating_value: None,
            rating_scale: None,
            favorite: false,
            progress: None,
            primary_action: None,
            external_ids: Vec::new(),
        }
    }

    #[test]
    fn candidate_cache_preserves_display_index_for_mixed_results() {
        let candidates = Arc::new(Mutex::new(Vec::new()));
        let works = vec![
            card("metadata-candidate-bangumi-1", "元数据结果"),
            card("content-candidate-mangadex-2", "目标漫画"),
        ];

        cache_displayed_candidates(&candidates, &works);

        let cached = candidates.lock().unwrap().clone();
        assert_eq!(
            cached,
            vec![
                "metadata-candidate-bangumi-1",
                "content-candidate-mangadex-2",
            ]
        );
        assert_eq!(
            cached.get(1).map(String::as_str),
            Some("content-candidate-mangadex-2")
        );
    }

    #[tokio::test]
    async fn searches_sources_concurrently() {
        let current = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let mut participants: Vec<Arc<dyn SearchSourceParticipant>> = Vec::new();
        for i in 0..4 {
            participants.push(Arc::new(FakeParticipant {
                id: format!("src{i}"),
                delay: Duration::from_millis(200),
                current: current.clone(),
                max: max.clone(),
            }));
        }
        // Build enabled descriptors
        let enabled: Vec<crate::wire::SourceDescriptorDto> = (0..4)
            .map(|i| crate::wire::SourceDescriptorDto {
                source_id: format!("src{i}"),
                display_name: format!("src{i}"),
                kinds: vec![crate::wire::SourceKindDto::Search],
                categories: vec![crate::wire::SourceCategoryDto::Video],
                mode: crate::wire::SourceModeDto::Single,
                notes: "测试来源".to_owned(),
                enabled: true,
                health: crate::wire::SourceHealthDto::Ok,
                endpoint_configured: true,
                credential_configured: false,
                last_checked: None,
                latency_ms: Some(200),
                success_rate: Some(0.9),
            })
            .collect();

        let sink = Arc::new(CaptureSink {
            events: Mutex::new(Vec::new()),
        });
        let emitter = Arc::new(TaskEmitter {
            operation_id: "op-test".to_owned(),
            sink: sink.clone(),
            emit_lock: Mutex::new(0),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        let candidates = Arc::new(Mutex::new(Vec::new()));
        let started = std::time::Instant::now();
        let result = run_dispatch(
            participants,
            DispatchQuery {
                text: "q".to_owned(),
                category: None,
                limit: 5,
            },
            enabled,
            emitter.clone(),
            candidates,
            no_health(),
        )
        .await;
        let elapsed = started.elapsed();
        // Parallel with 5 concurrency should be ~200ms, well below 600ms; serial would be ~800ms
        assert!(
            elapsed < Duration::from_millis(600),
            "should be concurrent, elapsed={elapsed:?}"
        );
        assert!(max.load(Ordering::SeqCst) <= 5);
        assert!(max.load(Ordering::SeqCst) > 1);
        assert_eq!(result, None, "should complete");
        let events = sink.events.lock().unwrap();
        // All SourceResult, no Warning, no terminal (terminal emitted by outer start)
        assert!(
            events
                .iter()
                .all(|e| e.kind == SearchSourceEventKind::SourceResult)
        );
        assert_eq!(events.len(), 4);
        // sequence strictly increasing
        for w in events.windows(2) {
            assert_eq!(w[1].sequence, w[0].sequence + 1);
        }
    }

    #[tokio::test]
    async fn starting_search_on_an_empty_database_keeps_cms10_opt_in() {
        let db = Arc::new(haven_infrastructure::Db::open_in_memory().unwrap());
        let repos = Arc::new(haven_infrastructure::db::repos::SqliteRepositories::new(db));
        let registry = SourceRegistryService::new(repos);
        let calls = Arc::new(AtomicUsize::new(0));
        let participant = Arc::new(ImmediateParticipant {
            id: "cms10".to_owned(),
            calls: calls.clone(),
        });
        let sink = Arc::new(CaptureSink {
            events: Mutex::new(Vec::new()),
        });
        let service = SearchSourceService::new(registry.clone(), vec![participant], sink);

        service
            .start(SearchSourceStartRequest {
                query: "火影忍者".to_owned(),
                category: Some(crate::wire::QueryCategory::Video),
                limit_per_source: Some(1),
            })
            .await
            .unwrap();

        let seeded = registry.list().await.unwrap();
        let cms10 = seeded
            .sources
            .iter()
            .find(|source| source.source_id == "cms10")
            .expect("空库搜索必须包含 CMS10 来源");
        assert!(!cms10.enabled);
        assert!(!cms10.endpoint_configured);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "未配置的 CMS10 不应被调度");
    }

    #[test]
    fn endpoint_gate_only_applies_to_configured_aggregate_sources() {
        let metadata = crate::wire::SourceDescriptorDto {
            source_id: "tvmaze".into(),
            display_name: "TVMaze".into(),
            kinds: vec![crate::wire::SourceKindDto::Search],
            categories: vec![crate::wire::SourceCategoryDto::Video],
            mode: crate::wire::SourceModeDto::Single,
            notes: "公开 API".into(),
            enabled: true,
            health: crate::wire::SourceHealthDto::Unknown,
            endpoint_configured: false,
            credential_configured: false,
            last_checked: None,
            latency_ms: None,
            success_rate: None,
        };
        assert!(!source_requires_endpoint(&metadata));
        let stream = crate::wire::SourceDescriptorDto {
            source_id: "cms10".into(),
            kinds: vec![crate::wire::SourceKindDto::OnlineRead],
            ..metadata
        };
        assert!(source_requires_endpoint(&stream));
    }

    #[test]
    fn missing_builtin_endpoints_explain_how_to_configure_them() {
        assert_eq!(
            source_not_configured_message("cms10"),
            "CMS10 需要先到“设置 → 来源”配置采集接口地址。"
        );
        assert_eq!(
            source_not_configured_message("m3u"),
            "M3U 需要先到“设置 → 来源”配置播放列表地址。"
        );
        assert_eq!(
            source_not_configured_message("custom_0123456789ab"),
            "该搜索源尚未完成配置。"
        );
    }

    #[test]
    fn endpoint_gate_covers_user_registered_feed_sources() {
        // 用户登记的 RSS/Atom 订阅源属于 `custom_` 家族：没有端点就不能搜索。
        let feed = crate::wire::SourceDescriptorDto {
            source_id: "custom_feed_0123456789ab".into(),
            display_name: "我的订阅".into(),
            kinds: vec![
                crate::wire::SourceKindDto::Search,
                crate::wire::SourceKindDto::OnlineRead,
            ],
            categories: vec![crate::wire::SourceCategoryDto::Periodical],
            mode: crate::wire::SourceModeDto::Single,
            notes: "用户登记的订阅源".into(),
            enabled: true,
            health: crate::wire::SourceHealthDto::Unknown,
            endpoint_configured: false,
            credential_configured: false,
            last_checked: None,
            latency_ms: None,
            success_rate: None,
        };
        assert!(source_requires_endpoint(&feed));
        let custom_opds = crate::wire::SourceDescriptorDto {
            source_id: "custom_0123456789ab".into(),
            ..feed
        };
        assert!(source_requires_endpoint(&custom_opds));
    }

    /// 分发回归：`custom_feed_` 同时满足 `custom_` 与 `custom_feed_` 两个前缀。
    /// 最长前缀路由必须把订阅源交给订阅源参与者，而不是自定义 OPDS 参与者。
    #[test]
    fn feed_family_dispatches_by_longest_prefix_not_to_custom_opds() {
        struct PrefixParticipant {
            id: &'static str,
            prefix: &'static str,
        }

        #[async_trait::async_trait]
        impl SearchSourceParticipant for PrefixParticipant {
            fn source_id(&self) -> &str {
                self.id
            }

            fn id_prefix(&self) -> Option<&str> {
                Some(self.prefix)
            }

            async fn search(
                &self,
                _query: &str,
                _limit: u32,
                _is_cancelled: &(dyn Fn() -> bool + Send + Sync),
            ) -> Result<Vec<WorkCardDto>, AppError> {
                Ok(Vec::new())
            }
        }

        let participants: Vec<Arc<dyn SearchSourceParticipant>> = vec![
            Arc::new(PrefixParticipant {
                id: "custom_",
                prefix: "custom_",
            }),
            Arc::new(PrefixParticipant {
                id: "custom_feed_",
                prefix: "custom_feed_",
            }),
        ];

        let feed = resolve_participant("custom_feed_0123456789ab", &participants)
            .expect("订阅源必须有分发目标");
        assert_eq!(
            feed.id_prefix(),
            Some("custom_feed_"),
            "订阅源不得被自定义 OPDS 前缀截走"
        );
        let opds = resolve_participant("custom_0123456789ab", &participants)
            .expect("自定义 OPDS 书源必须有分发目标");
        assert_eq!(opds.id_prefix(), Some("custom_"));
        assert!(resolve_participant("mangadex", &participants).is_none());
    }

    #[tokio::test]
    async fn cancel_yields_single_terminal() {
        let current = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let participants: Vec<Arc<dyn SearchSourceParticipant>> = vec![
            Arc::new(FakeParticipant {
                id: "fast".to_owned(),
                delay: Duration::from_millis(50),
                current: current.clone(),
                max: max.clone(),
            }),
            Arc::new(FakeParticipant {
                id: "slow".to_owned(),
                delay: Duration::from_secs(10),
                current: current.clone(),
                max: max.clone(),
            }),
        ];
        let enabled: Vec<crate::wire::SourceDescriptorDto> = vec![
            crate::wire::SourceDescriptorDto {
                source_id: "fast".to_owned(),
                display_name: "fast".to_owned(),
                kinds: vec![crate::wire::SourceKindDto::Search],
                categories: vec![crate::wire::SourceCategoryDto::Video],
                mode: crate::wire::SourceModeDto::Single,
                notes: "测试来源".to_owned(),
                enabled: true,
                health: crate::wire::SourceHealthDto::Ok,
                endpoint_configured: true,
                credential_configured: false,
                last_checked: None,
                latency_ms: Some(50),
                success_rate: Some(0.9),
            },
            crate::wire::SourceDescriptorDto {
                source_id: "slow".to_owned(),
                display_name: "slow".to_owned(),
                kinds: vec![crate::wire::SourceKindDto::Search],
                categories: vec![crate::wire::SourceCategoryDto::Video],
                mode: crate::wire::SourceModeDto::Single,
                notes: "测试来源".to_owned(),
                enabled: true,
                health: crate::wire::SourceHealthDto::Ok,
                endpoint_configured: true,
                credential_configured: false,
                last_checked: None,
                latency_ms: Some(5000),
                success_rate: Some(0.9),
            },
        ];
        let sink = Arc::new(CaptureSink {
            events: Mutex::new(Vec::new()),
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let emitter = Arc::new(TaskEmitter {
            operation_id: "op-cancel".to_owned(),
            sink: sink.clone(),
            emit_lock: Mutex::new(0),
            cancel: cancel.clone(),
        });
        let candidates = Arc::new(Mutex::new(Vec::new()));
        let participants_clone = participants.clone();
        let enabled_clone = enabled.clone();
        let emitter_clone = emitter.clone();
        let candidates_clone = candidates.clone();
        let handle = tokio::spawn(async move {
            run_dispatch(
                participants_clone,
                DispatchQuery {
                    text: "q".to_owned(),
                    category: None,
                    limit: 5,
                },
                enabled_clone,
                emitter_clone,
                candidates_clone,
                no_health(),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.store(true, Ordering::Release);
        let result = handle.await.unwrap();
        assert_eq!(result, Some(SearchSourceEventKind::Cancelled));
        // run_dispatch returns Cancelled but does not emit it; outer start will emit single terminal
        let events = sink.events.lock().unwrap();
        // No terminal emitted by run_dispatch itself
        assert!(events.iter().all(|e| !matches!(
            e.kind,
            SearchSourceEventKind::Completed | SearchSourceEventKind::Cancelled
        )));
        for w in events.windows(2) {
            assert_eq!(w[1].sequence, w[0].sequence + 1);
        }
    }

    /// 立即返回空结果的参与者：健康用例用它证明「空成功响应也是一次真实成功」。
    pub(super) struct ImmediateParticipant {
        pub(super) id: String,
        pub(super) calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl SearchSourceParticipant for ImmediateParticipant {
        fn source_id(&self) -> &str {
            &self.id
        }

        async fn search(
            &self,
            _query: &str,
            _limit: u32,
            _is_cancelled: &(dyn Fn() -> bool + Send + Sync),
        ) -> Result<Vec<WorkCardDto>, AppError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }
    }

    /// 不写健康事实的分发上下文（只验证调度与事件行为的用例使用）。
    /// 真实搜索观测 → 健康事实的用例在 `health_tests` 里自带完整夹具。
    pub(super) fn no_health() -> HealthContext {
        HealthContext {
            revisions: BTreeMap::new(),
            recorder: None,
        }
    }
}
