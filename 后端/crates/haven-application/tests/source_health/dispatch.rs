//! 真实搜索分发 → 来源健康事实（公开装配面）。
//!
//! 这些用例原本直接驱动私有的 `run_dispatch` + `TaskEmitter` + `HealthContext`，只能在
//! lib-test 实例里跑；而 lib-test 实例无法注入真实 `SqliteSettingsUoW`（E0277：两个
//! crate 实例的 `SettingsUoW` 不是同一个 item）。这里改从**公开**入口驱动同一条分发
//! 路径：`SearchSourceService::start` 在后台跑 `run_dispatch`，事件经注入的
//! `SearchEventSink` 出来，参与者的先后由 `Notify` 决定，取消走公开的
//! `SearchSourceService::cancel`。除看门狗超时外没有任何 sleep。
//!
//! 注意「迟到结果」这一条的语义在这里比原来更强：原来是先停用再把描述符硬塞进分发
//! 队列；现在来源真的在飞行中被停用，结果随后才回来——这正是「迟到」的定义。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use haven_application::services::search_source::{
    SearchCancelOutcome, SearchEventSink, SearchSourceParticipant, SearchSourceService,
};
use haven_application::services::source_registry::SourceRegistryService;
use haven_application::wire::{
    ContentCategory, MediaTypeDto, SearchSourceEvent, SearchSourceEventKind,
    SearchSourceStartRequest, SourceHealthDto, SourceRegistrySetRequest, SourceUpdateRequest,
    WorkCardDto,
};
use haven_common::{AppError, ErrorKind};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::fixtures::{descriptor_of, registry_with_health};

/// 看门狗：分发与健康写入都必须在此之前收敛到终态（唯一的时间上界）。
const WATCHDOG: Duration = Duration::from_secs(10);
/// 单来源上限（调度只关心它是否合法）。
const LIMIT: u32 = 5;

// ---------- 事件下沉与分发夹具 ----------

/// 事件下沉：用无界通道把事件交给测试，终态由 `await` 决定而不是 sleep。
struct ChannelSink {
    tx: UnboundedSender<SearchSourceEvent>,
}

impl SearchEventSink for ChannelSink {
    fn emit_search_event(&self, event: SearchSourceEvent) {
        // 接收端在整个用例期间存活；发送失败只可能发生在用例已经结束之后。
        let _ = self.tx.send(event);
    }
}

/// 用给定参与者组装公开服务，并返回事件接收端。
fn service_with(
    registry: &SourceRegistryService,
    participants: Vec<Arc<dyn SearchSourceParticipant>>,
) -> (SearchSourceService, UnboundedReceiver<SearchSourceEvent>) {
    let (tx, rx) = unbounded_channel();
    let service =
        SearchSourceService::new(registry.clone(), participants, Arc::new(ChannelSink { tx }));
    (service, rx)
}

/// `start` 必须成功登记；返回 operationId（取消与后续断言都要用它）。
async fn start_search(service: &SearchSourceService, query: &str) -> String {
    service
        .start(SearchSourceStartRequest {
            query: query.to_owned(),
            category: None,
            limit_per_source: Some(LIMIT),
        })
        .await
        .expect("分发登记必须成功")
        .operation_id
}

/// 等到终态事件（completed | cancelled | failed）并返回它的种类。
///
/// 健康写入发生在 `run_dispatch` 内部、早于终态发出，所以看到终态时观测已经落地。
async fn await_terminal(rx: &mut UnboundedReceiver<SearchSourceEvent>) -> SearchSourceEventKind {
    tokio::time::timeout(WATCHDOG, async {
        while let Some(event) = rx.recv().await {
            if matches!(
                event.kind,
                SearchSourceEventKind::Completed
                    | SearchSourceEventKind::Cancelled
                    | SearchSourceEventKind::Failed
            ) {
                return event.kind;
            }
        }
        panic!("事件通道在终态之前关闭");
    })
    .await
    .expect("分发必须在看门狗超时前收敛到终态")
}

fn card(work_id: &str, title: &str) -> WorkCardDto {
    WorkCardDto {
        work_id: work_id.to_owned(),
        title: title.to_owned(),
        original_title: None,
        description: None,
        categories: vec![ContentCategory::Comic],
        available_media_types: vec![MediaTypeDto::Comic],
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

// ---------- 参与者 ----------

/// 立即返回空结果：空的成功响应同样是一次真实成功的探测。
struct ImmediateParticipant {
    id: String,
    calls: Arc<AtomicUsize>,
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

/// 总是失败的真实参与者（错误算失败）。
struct FailingParticipant {
    id: String,
}

#[async_trait::async_trait]
impl SearchSourceParticipant for FailingParticipant {
    fn source_id(&self) -> &str {
        &self.id
    }

    async fn search(
        &self,
        _query: &str,
        _limit: u32,
        _is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<WorkCardDto>, AppError> {
        Err(AppError::new(
            "SOURCE_UNAVAILABLE",
            ErrorKind::Network,
            "测试来源不可达",
            true,
        ))
    }
}

/// 由测试显式放行的参与者：成功/失败由构造参数决定，结果返回与取消的先后由
/// `Notify` 决定，不靠 sleep 抢时序。
struct GatedParticipant {
    id: String,
    started: Arc<Notify>,
    release: Arc<Notify>,
    succeeded: bool,
}

#[async_trait::async_trait]
impl SearchSourceParticipant for GatedParticipant {
    fn source_id(&self) -> &str {
        &self.id
    }

    async fn search(
        &self,
        _query: &str,
        _limit: u32,
        _is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<WorkCardDto>, AppError> {
        // 这次调用真的开始了（探测已发生），测试据此在**确定**的时刻取消或放行。
        self.started.notify_one();
        self.release.notified().await;
        if self.succeeded {
            Ok(Vec::new())
        } else {
            Err(AppError::new(
                "SOURCE_UNAVAILABLE",
                ErrorKind::Network,
                "测试来源不可达",
                true,
            ))
        }
    }
}

/// 搜索进行中修改自己端点地址的参与者：模拟「用户在请求飞行中编辑来源」。
///
/// 自定义源改地址走 `update_custom_source`（`set_endpoint` 只接受内置目录里的 sourceId）；
/// 这条路径同样会推进该来源的配置版本。
struct EndpointEditingParticipant {
    registry: SourceRegistryService,
    source_id: String,
}

#[async_trait::async_trait]
impl SearchSourceParticipant for EndpointEditingParticipant {
    fn source_id(&self) -> &str {
        &self.source_id
    }

    async fn search(
        &self,
        _query: &str,
        _limit: u32,
        _is_cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Result<Vec<WorkCardDto>, AppError> {
        self.registry
            .update_custom_source(SourceUpdateRequest {
                source_id: self.source_id.clone(),
                display_name: None,
                endpoint: Some("https://two.example.invalid/opds".to_owned()),
            })
            .await
            .expect("飞行中改地址必须成功");
        Ok(vec![card("content-candidate-opds-1", "改址后的结果")])
    }
}

// ---------- 用例 ----------

/// 空成功响应也是一次真实成功的探测：健康必须反映它，而不是停在出厂 unknown。
#[tokio::test]
async fn successful_source_search_records_ok_health() {
    let (registry, _repos) = registry_with_health();
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, mut rx) = service_with(
        &registry,
        vec![Arc::new(ImmediateParticipant {
            id: "mangadex".to_owned(),
            calls: calls.clone(),
        })],
    );

    start_search(&service, "健康-成功").await;
    assert_eq!(
        await_terminal(&mut rx).await,
        SearchSourceEventKind::Completed
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let observed = descriptor_of(&registry, "mangadex").await;
    assert_eq!(observed.health, SourceHealthDto::Ok);
    assert_eq!(observed.success_rate, Some(1.0));
    assert!(observed.last_checked.is_some());
    assert!(observed.latency_ms.is_some());
    assert!(observed.enabled, "健康结果永远不能改写启用状态");
}

/// 错误与超时算失败：连续失败降到 Degraded，再有失败即 Down；成功率随之下降。
#[tokio::test]
async fn failed_source_searches_record_real_failures_until_down() {
    let (registry, _repos) = registry_with_health();
    let (service, mut rx) = service_with(
        &registry,
        vec![Arc::new(FailingParticipant {
            id: "mangadex".to_owned(),
        })],
    );

    for (index, expected) in [
        SourceHealthDto::Degraded,
        SourceHealthDto::Degraded,
        SourceHealthDto::Down,
    ]
    .into_iter()
    .enumerate()
    {
        // 每轮用不同的 query：同一 query 的运行中任务会被幂等合并，那不是这里有意的行为。
        start_search(&service, &format!("健康-失败-{index}")).await;
        assert_eq!(
            await_terminal(&mut rx).await,
            SearchSourceEventKind::Completed,
            "单个来源失败不是整次搜索失败"
        );
        assert_eq!(descriptor_of(&registry, "mangadex").await.health, expected);
    }

    let observed = descriptor_of(&registry, "mangadex").await;
    assert_eq!(observed.success_rate, Some(0.0));
    assert!(observed.enabled, "来源失败同样不得关掉用户的选择");
}

/// 取消不是探测：既不记成功也不记失败；随后一次真实搜索仍然照常落地。
#[tokio::test]
async fn cancelled_source_search_records_no_health() {
    let (registry, _repos) = registry_with_health();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (service, mut rx) = service_with(
        &registry,
        vec![Arc::new(GatedParticipant {
            id: "mangadex".to_owned(),
            started: started.clone(),
            release: release.clone(),
            succeeded: false,
        })],
    );

    let operation_id = start_search(&service, "健康-取消").await;
    // 这次调用已经开始（探测确实发生了），此刻才取消：先后由事件决定，不看时钟。
    tokio::time::timeout(WATCHDOG, started.notified())
        .await
        .expect("参与者必须已经开始这次分发");
    assert_eq!(
        service.cancel(&operation_id).unwrap(),
        SearchCancelOutcome::Cancelled
    );
    assert_eq!(
        await_terminal(&mut rx).await,
        SearchSourceEventKind::Cancelled,
        "取消必须收敛为 cancelled 终态"
    );

    let observed = descriptor_of(&registry, "mangadex").await;
    assert_eq!(
        observed.health,
        SourceHealthDto::Unknown,
        "取消的查询不得留下失败（或成功）观测"
    );
    assert_eq!(observed.success_rate, None);
    assert_eq!(observed.last_checked, None);

    // 同一来源随后的一次真实搜索照常落地（证明上一步的空缺不是装配问题）。
    let (service, mut rx) = service_with(
        &registry,
        vec![Arc::new(ImmediateParticipant {
            id: "mangadex".to_owned(),
            calls: Arc::new(AtomicUsize::new(0)),
        })],
    );
    start_search(&service, "健康-取消之后").await;
    assert_eq!(
        await_terminal(&mut rx).await,
        SearchSourceEventKind::Completed
    );
    assert_eq!(
        descriptor_of(&registry, "mangadex").await.health,
        SourceHealthDto::Ok
    );
}

/// 请求飞行中编辑端点：迟到的观测不得把**新**端点标成健康，也不得改回配置。
#[tokio::test]
async fn endpoint_edit_during_search_does_not_mark_the_new_endpoint_healthy() {
    let (registry, _repos) = registry_with_health();
    let added = registry
        .add_custom_source("我的书源", "https://one.example.invalid/opds")
        .await
        .unwrap();
    registry
        .set_custom_source_enabled(&added.source_id, true)
        .await
        .unwrap();
    let snapshot = registry.enabled_sources_snapshot().await.unwrap();
    assert!(
        snapshot.revisions.contains_key(&added.source_id),
        "自定义来源的配置版本必须在分发快照里"
    );

    let (service, mut rx) = service_with(
        &registry,
        vec![Arc::new(EndpointEditingParticipant {
            registry: registry.clone(),
            source_id: added.source_id.clone(),
        })],
    );
    start_search(&service, "健康-改址").await;
    assert_eq!(
        await_terminal(&mut rx).await,
        SearchSourceEventKind::Completed
    );

    let observed = descriptor_of(&registry, &added.source_id).await;
    assert_eq!(
        observed.health,
        SourceHealthDto::Unknown,
        "改地址后旧观测描述的不是当前来源"
    );
    assert_eq!(observed.success_rate, None);
    assert_eq!(
        registry
            .endpoint(&added.source_id)
            .await
            .unwrap()
            .as_deref(),
        Some("https://two.example.invalid/opds"),
        "健康写入不得覆盖用户刚写入的端点"
    );
}

/// 迟到的结果绝不允许把已经停用的来源重新打开。
///
/// 分发时来源仍然启用（请求已经飞出去），飞行中用户把它停用，结果随后才回来：
/// 这仍是一次针对当前出站绑定的真实观测（停用不改变端点与配置版本），因此健康照常
/// 落地——但启用状态必须保持用户的选择，且下一次分发不再包含它。
#[tokio::test]
async fn late_source_result_never_reenables_a_disabled_source() {
    let (registry, _repos) = registry_with_health();
    // 先让出厂播种落地（首次读取才播种默认启用来源），再停用才是真实的状态翻转。
    let _ = registry.list().await.unwrap();

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (service, mut rx) = service_with(
        &registry,
        vec![Arc::new(GatedParticipant {
            id: "mangadex".to_owned(),
            started: started.clone(),
            release: release.clone(),
            succeeded: true,
        })],
    );

    start_search(&service, "健康-迟到").await;
    tokio::time::timeout(WATCHDOG, started.notified())
        .await
        .expect("参与者必须已经开始这次分发");

    // 请求飞行中用户停用该来源；结果在停用之后才回来。
    registry
        .set(SourceRegistrySetRequest {
            source_id: "mangadex".into(),
            enabled: false,
        })
        .await
        .unwrap();
    release.notify_one();
    assert_eq!(
        await_terminal(&mut rx).await,
        SearchSourceEventKind::Completed
    );

    let observed = descriptor_of(&registry, "mangadex").await;
    assert!(!observed.enabled, "迟到的健康结果不得改写启用状态");
    assert_eq!(
        observed.health,
        SourceHealthDto::Ok,
        "停用不改写出站绑定，这次观测仍是针对当前来源的"
    );

    assert!(
        !registry
            .enabled_sources_snapshot()
            .await
            .unwrap()
            .sources
            .iter()
            .any(|source| source.source_id == "mangadex"),
        "停用后不再进入分发快照"
    );
}
