//! CastService：投屏编排（v02-cast-001 双栈）。
//! - 发现：透传 Port（DLNA SSDP + Chromecast mDNS 合并）。
//! - 播放：复用 StreamService::prepare 解析 upstream_url → 注册 CastGrant → 控制面推流。
//! - 状态/停止：透传 Port，幂等。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use haven_common::{AppError, ErrorKind};

use crate::services::stream::StreamService;
use crate::wire::{
    CastDeviceDto, CastDiscoverRequest, CastDiscoverResult, CastPlayRequest, CastPlayResult,
    CastStatusDto, CastStatusRequest, CastStopRequest, CastStopResult, CastTransportStateDto,
    SessionEngineDto,
};

#[async_trait]
pub trait CastDiscoveryPort: Send + Sync {
    async fn discover(&self, timeout_ms: u32) -> Result<Vec<CastDeviceDto>, AppError>;
}

#[async_trait]
pub trait CastControlPort: Send + Sync {
    async fn play(&self, device_id: &str, lan_url: &str, title: &str) -> Result<(), AppError>;
    async fn stop(&self, device_id: &str) -> Result<(), AppError>;
    async fn status(&self, device_id: &str) -> Result<CastTransportStateDto, AppError>;
    async fn position(&self, device_id: &str) -> Result<(Option<u64>, Option<u64>), AppError>;
}

#[async_trait]
pub trait CastMediaPort: Send + Sync {
    fn register(&self, grant: String, upstream_url: String, title: String) -> String;
    fn revoke(&self, grant: &str) -> bool;
    fn lan_url(&self, grant: &str) -> Option<String>;
}

#[derive(Clone)]
pub struct CastGrantRegistry {
    inner: Arc<Mutex<HashMap<String, CastGrant>>>,
    base_url: String,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
struct CastGrant {
    grant: String,
    upstream_url: String,
    device_id: String,
}

impl CastGrantRegistry {
    pub fn new(base_url: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            base_url,
        }
    }

    pub fn register(&self, grant: String, upstream_url: String, device_id: String) -> String {
        let lan = format!("{}/cast/{}", self.base_url.trim_end_matches('/'), grant);
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.insert(
            grant.clone(),
            CastGrant {
                grant,
                upstream_url,
                device_id,
            },
        );
        lan
    }

    pub fn revoke(&self, grant: &str) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.remove(grant).is_some()
    }

    pub fn lan_url(&self, grant: &str) -> Option<String> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.get(grant)
            .map(|v| format!("{}/cast/{}", self.base_url.trim_end_matches('/'), v.grant))
    }
}

/// 单台设备的控制状态（C1）。
///
/// 前端只有实例代数，无法跨实例保护投屏；设备所有权只能由后端维护。规则：
/// - `intent`：最近一次发起的 play 的单调序号。新 play 在**任何异步等待之前**
///   递增它，立刻作废更早的慢速 prepare；
/// - `owner`：设备上物理上正在播放的 grant，只在控制面 Play 结论明确后更新。
///   它与 `intent` 分离：更新的 play 仍在准备或准备失败时，旧 owner 仍是设备
///   上的实际内容，必须仍能被它自己的 stop 停止；
/// - `lock`：每台设备稳定的一把 tokio 锁（`DeviceState` 本身从不被 Arc 共享，
///   锁原地存在），play 与 stop 都 `lock_owned().await` 串行化，覆盖控制面
///   调用本身；不同设备各自一把锁，互不阻塞；
/// - std Mutex 只在同步临界区内读写上述字段，绝不跨 await 持有。
struct DeviceState {
    intent: u64,
    owner: Option<String>,
    lock: Arc<tokio::sync::Mutex<()>>,
}

impl Default for DeviceState {
    fn default() -> Self {
        Self {
            intent: 0,
            owner: None,
            lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
}

#[derive(Default)]
struct DeviceControls {
    intent_seq: u64,
    devices: HashMap<String, DeviceState>,
}

impl DeviceControls {
    /// 登记一次新的 play 意图，作废该设备上更早的（以及随后开始的）play。
    /// 不触碰 `owner`：物理播放者只在控制面给出明确结论后才改变。
    fn begin_play(&mut self, device_id: &str) -> u64 {
        self.intent_seq = self.intent_seq.wrapping_add(1);
        let intent = self.intent_seq;
        self.devices.entry(device_id.to_owned()).or_default().intent = intent;
        intent
    }

    fn get(&self, device_id: &str) -> Option<&DeviceState> {
        self.devices.get(device_id)
    }
}

#[derive(Clone)]
pub struct CastService {
    discovery: Arc<dyn CastDiscoveryPort>,
    control: Arc<dyn CastControlPort>,
    media: Arc<dyn CastMediaPort>,
    stream: StreamService,
    grant_registry: Arc<CastGrantRegistry>,
    /// 设备控制状态跨 clone / 服务调用共享；std Mutex 只做同步状态读写，绝不跨 await 持有。
    device_controls: Arc<Mutex<DeviceControls>>,
}

impl CastService {
    pub fn new(
        discovery: Arc<dyn CastDiscoveryPort>,
        control: Arc<dyn CastControlPort>,
        media: Arc<dyn CastMediaPort>,
        stream: StreamService,
        grant_registry: Arc<CastGrantRegistry>,
    ) -> Self {
        Self {
            discovery,
            control,
            media,
            stream,
            grant_registry,
            device_controls: Arc::new(Mutex::new(DeviceControls::default())),
        }
    }

    pub async fn discover(&self, req: CastDiscoverRequest) -> Result<CastDiscoverResult, AppError> {
        let timeout_ms = req.timeout_ms.unwrap_or(5000);
        if timeout_ms == 0 || timeout_ms > 10_000 {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "timeout_ms 需在 1..10000",
                false,
            ));
        }
        let devices = self.discovery.discover(timeout_ms).await?;
        Ok(CastDiscoverResult {
            schema_version: 1,
            devices,
        })
    }

    pub async fn play(&self, req: CastPlayRequest) -> Result<CastPlayResult, AppError> {
        if req.media_item_id.trim().is_empty() || req.device_id.trim().is_empty() {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "参数缺失",
                false,
            ));
        }
        if req.engine != SessionEngineDto::Playback {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                ErrorKind::Validation,
                "仅支持 playback 引擎投屏",
                false,
            ));
        }
        // C1：在**任何异步等待之前**同步登记该设备状态并递增意图，慢速 prepare
        // 不得覆盖更新的 play；随后取稳定锁句柄（不跨 await 持有 std Mutex）。
        let (intent, device_lock) = {
            let mut m = self.lock_controls();
            let intent = m.begin_play(&req.device_id);
            let lock = m.get(&req.device_id).map(|d| Arc::clone(&d.lock));
            (intent, lock)
        };
        let Some(device_lock) = device_lock else {
            return Err(superseded_error());
        };
        // 复用 StreamService 校验与上游解析（远端流条目校验 is_hls 等）。
        let facts = self
            .stream
            .prepare(crate::wire::SessionOpenRequest {
                media_item_id: req.media_item_id.clone(),
                engine: req.engine,
            })
            .await?;
        // 取得该设备稳定的控制锁（等待而非拒绝：更新的 B 排在当前操作之后，
        // 而不是让过期的 A 推流成功后 B 失败）。
        let guard = device_lock.lock_owned().await;
        // 等锁期间（或 prepare 期间）可能已有更新的 play 登记意图：此时直接作废本次，
        // 不注册 grant、不触碰设备。
        if self.device_intent(&req.device_id) != Some(intent) {
            drop(guard);
            return Err(superseded_error());
        }
        // 持有设备锁注册 grant：此后（含取消）都由 RAII 守卫回滚，不留悬挂授权。
        let grant = uuid::Uuid::new_v4().to_string();
        let rollback = GrantRollback::new(self, grant.clone());
        let title = facts.media_item_id.clone();
        // A 路线：远端 HLS 直通上游（无需 LAN 代理；本地文件批次再切回 media.register 的 /cast 代理）
        let lan_url = if facts.is_hls {
            // 编码中文路径后直通，TV 可直拉
            let encoded = encode_url_for_cast(&facts.upstream_url);
            self.media
                .register(grant.clone(), encoded.clone(), title.clone());
            self.grant_registry
                .register(grant.clone(), encoded.clone(), req.device_id.clone());
            encoded
        } else {
            let url = self
                .media
                .register(grant.clone(), facts.upstream_url.clone(), title.clone());
            self.grant_registry.register(
                grant.clone(),
                facts.upstream_url.clone(),
                req.device_id.clone(),
            );
            url
        };
        // 持锁推流。等待期间到达的更新 play 只递增 intent、不改 owner：本 play
        // 若物理成功仍须登记为 owner，其后续 stop 只能串行在更新 play 之前（安全：
        // 那时更新 play 尚未开始）或在其建立所有权之后被识别为过期（不发 Stop）。
        // 不因更新 play 排队而拒绝本次成功，以免真正生效的播放无人可停。
        match self.control.play(&req.device_id, &lan_url, &title).await {
            Ok(()) => {
                {
                    let mut m = self.lock_controls();
                    m.devices.entry(req.device_id.clone()).or_default().owner = Some(grant.clone());
                }
                drop(guard);
                rollback.disarm();
                Ok(CastPlayResult {
                    schema_version: 1,
                    cast_session_id: grant,
                    lan_url,
                    device_name: req.device_id.clone(),
                })
            }
            Err(e) => {
                // 正常 Err：控制面明确未开始播放，owner 不变（旧物理 owner 照常可停，
                // 更新的 play 也可重试）。物理控制失败若发生在"已发送但无响应"的
                // 模糊区间，底层错误是否可重试由 Port 的 retryable 表达；此处不把
                // 失败当作已清理设备。
                drop(guard);
                drop(rollback);
                Err(e)
            }
        }
    }

    fn lock_controls(&self) -> std::sync::MutexGuard<'_, DeviceControls> {
        self.device_controls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// 取指定设备的控制锁句柄（锁本身稳定，克隆 Arc 后即可在 await 前释放 std Mutex）。
    fn device_lock(&self, device_id: &str) -> Option<Arc<tokio::sync::Mutex<()>>> {
        self.lock_controls()
            .get(device_id)
            .map(|d| Arc::clone(&d.lock))
    }

    fn device_intent(&self, device_id: &str) -> Option<u64> {
        self.lock_controls().get(device_id).map(|d| d.intent)
    }

    /// 撤销本 play 的 grant（设备端未确认播放，无需 Stop）。
    fn rollback_play(&self, grant: &str) {
        self.media.revoke(grant);
        self.grant_registry.revoke(grant);
    }

    pub async fn status(&self, req: CastStatusRequest) -> Result<CastStatusDto, AppError> {
        // cast_session_id 即 grant；需从 registry 取 device_id
        let device_id = {
            let g = self
                .grant_registry
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            g.get(&req.cast_session_id)
                .map(|v| v.device_id.clone())
                .ok_or_else(|| {
                    AppError::new(
                        "RESOURCE_NOT_FOUND",
                        ErrorKind::NotFound,
                        "投屏会话不存在或已结束",
                        false,
                    )
                })?
        };
        let transport_state = self
            .control
            .status(&device_id)
            .await
            .unwrap_or(CastTransportStateDto::Unknown);
        let (position_ms, duration_ms) = self
            .control
            .position(&device_id)
            .await
            .unwrap_or((None, None));
        Ok(CastStatusDto {
            schema_version: 1,
            transport_state,
            position_ms,
            duration_ms,
        })
    }

    pub async fn stop(&self, req: CastStopRequest) -> Result<CastStopResult, AppError> {
        let device_id = {
            let g = self
                .grant_registry
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            g.get(&req.cast_session_id).map(|v| v.device_id.clone())
        };
        let Some(did) = device_id else {
            // 设备已确认停止（或从未推流）：重复 stop 幂等。
            self.rollback_play(&req.cast_session_id);
            return Ok(CastStopResult {
                schema_version: 1,
                stopped: true,
            });
        };
        // 失败时保留 registry 条目以便重试；成功才撤销。
        // 若该设备已被更新的 play（B）接管，控制锁要等 B 完整返回后才轮到本次 stop：
        // 此时 owner 是 B 的 grant，本次只撤销自己的 grant，绝不发 Stop 杀掉 B
        // （或未来任何内容），也不影响 B 的设备所有权。
        let guard = match self.device_lock(&did) {
            Some(lock) => lock.lock_owned().await,
            None => {
                self.rollback_play(&req.cast_session_id);
                return Ok(CastStopResult {
                    schema_version: 1,
                    stopped: true,
                });
            }
        };
        let owns_device = {
            let m = self.lock_controls();
            m.get(&did).and_then(|d| d.owner.as_deref()) == Some(req.cast_session_id.as_str())
        };
        if !owns_device {
            // 设备所有权已属于其他 grant（更新的 play 成功）或设备上不存在本次
            // 会话：旧 grant 只能撤销自己的授权，绝不发 Stop 停掉别人的内容。
            drop(guard);
            self.rollback_play(&req.cast_session_id);
            return Ok(CastStopResult {
                schema_version: 1,
                stopped: true,
            });
        }
        if let Err(e) = self.control.stop(&did).await {
            // 控制面明确失败：保留 owner 与 grant，重复 stop 可重试。
            drop(guard);
            return Err(e);
        }
        {
            let mut m = self.lock_controls();
            if let Some(d) = m.devices.get_mut(&did) {
                if d.owner.as_deref() == Some(req.cast_session_id.as_str()) {
                    d.owner = None;
                }
            }
        }
        drop(guard);
        self.rollback_play(&req.cast_session_id);
        Ok(CastStopResult {
            schema_version: 1,
            stopped: true,
        })
    }
}

/// play 注册 grant 后的 RAII 回滚：持锁期间的所有提前返回（正常 Err、superseded、
/// 控制面取消）都会撤销本 grant。取消边界：future 若在 `control.play` 等待中
/// 被 drop，控制面可能已在设备上生效而结果未知；此时回滚 grant 会使设备拉流
/// 失效，但本 grant 不会成为 owner（不登记物理成功），设备上的残余播放只能
/// 由后续 play/stop 覆盖——不会伪造"已回滚设备"的成功结论。
struct GrantRollback<'a> {
    service: &'a CastService,
    grant: Option<String>,
}

impl<'a> GrantRollback<'a> {
    fn new(service: &'a CastService, grant: String) -> Self {
        Self {
            service,
            grant: Some(grant),
        }
    }

    fn disarm(mut self) {
        self.grant = None;
    }
}

impl Drop for GrantRollback<'_> {
    fn drop(&mut self) {
        if let Some(grant) = self.grant.take() {
            self.service.rollback_play(&grant);
        }
    }
}

/// 更新的 play 已接管该设备（或替代了本次播放）。
fn superseded_error() -> AppError {
    AppError::new(
        "CAST_SUPERSEDED",
        ErrorKind::Conflict,
        "已有更新的投屏请求接管该设备",
        false,
    )
}

fn encode_url_for_cast(url: &str) -> String {
    let mut out = String::new();
    let mut chars = url.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            if let Some(&h) = chars.peek() {
                if h.is_ascii_hexdigit() {
                    chars.next();
                    if let Some(lo) = chars.next() {
                        if lo.is_ascii_hexdigit() {
                            out.push('%');
                            out.push(h);
                            out.push(lo);
                            continue;
                        }
                    }
                }
            }
            out.push_str(&format!("%{:02X}", c as u8));
        } else if c.is_ascii_alphanumeric()
            || matches!(
                c,
                '-' | '_'
                    | '.'
                    | '~'
                    | ':'
                    | '/'
                    | '?'
                    | '='
                    | '&'
                    | '#'
                    | '@'
                    | '!'
                    | '$'
                    | '\''
                    | '('
                    | ')'
                    | '*'
                    | '+'
                    | ','
                    | ';'
            )
        {
            out.push(c);
        } else {
            for b in c.to_string().as_bytes() {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

// 仅用于测试的内存实现
#[cfg(test)]
pub mod test_doubles {
    use std::sync::atomic::{AtomicU64, Ordering};

    use haven_domain::{
        ArtworkSet, Availability, AvailabilitySource, Edition, EditionId, MediaIndex, MediaItem,
        MediaItemId, MediaItemStatus, MediaType, Resource, ResourceId, ResourceLocator,
        ResourceType, Work, WorkId, WorkStatus, WorkType,
    };

    use crate::wire::CastProtocolDto;

    use super::*;

    /// 可控的流端口：读取自身 media_item_id 时按 `hls` 返回远端流资源；可挂起
    /// 条目读取以复现慢速 prepare。读取其他 ID 返回 None（prepare 失败）。
    pub struct FakeSessionPorts {
        media_item_id: MediaItemId,
        hls: bool,
        hold: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        started: tokio::sync::Notify,
    }

    impl FakeSessionPorts {
        pub fn new(hls: bool) -> Self {
            Self {
                media_item_id: MediaItemId::new(),
                hls,
                hold: Mutex::new(None),
                started: tokio::sync::Notify::new(),
            }
        }

        pub fn media_id(&self) -> String {
            self.media_item_id.to_string()
        }

        /// 挂起下一次 prepare：返回的 future 在 prepare 开始时才完成。
        pub async fn wait_prepare_started(&self) {
            self.started.notified().await;
        }

        pub fn install(&self, rx: tokio::sync::oneshot::Receiver<()>) {
            *self.hold.lock().unwrap_or_else(|e| e.into_inner()) = Some(rx);
        }

        fn take(&self) -> Option<tokio::sync::oneshot::Receiver<()>> {
            self.hold.lock().unwrap_or_else(|e| e.into_inner()).take()
        }

        fn work(&self) -> Work {
            Work {
                id: WorkId::new(),
                canonical_title: "t".into(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Standalone,
                release_year: None,
                language: None,
                director: None,
                actor: None,
                status: WorkStatus::Unknown,
                rating_value: None,
                rating_scale: None,
                artwork: ArtworkSet::default(),
                created_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
            }
        }

        fn edition(&self) -> Edition {
            Edition {
                id: EditionId::new(),
                work_id: WorkId::new(),
                title: "e".into(),
                subtitle: None,
                edition_type: MediaType::Movie,
                release_date: None,
                language: None,
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: ArtworkSet::default(),
                created_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
            }
        }

        fn media_item(&self, edition_id: EditionId) -> MediaItem {
            MediaItem {
                id: self.media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Movie,
                title: "m".into(),
                index: MediaIndex::Movie,
                duration_ms: None,
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
            }
        }

        fn resources(&self) -> Vec<Resource> {
            vec![Resource {
                id: ResourceId::new(),
                media_item_id: self.media_item_id,
                resource_type: if self.hls {
                    ResourceType::HlsStream
                } else {
                    ResourceType::VideoStream
                },
                source_id: None,
                storage_location_id: None,
                locator: ResourceLocator::Http {
                    url: if self.hls {
                        "https://example.com/stream.m3u8"
                    } else {
                        "https://example.com/stream.mp4"
                    }
                    .into(),
                },
                mime_type: None,
                size: None,
                hash: None,
                availability: Availability::Available,
                availability_source: AvailabilitySource::User,
                modified_ms: None,
                fingerprint_first: None,
                fingerprint_last: None,
                created_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
            }]
        }
    }

    #[async_trait]
    impl haven_domain::contracts::WorkRepository for FakeSessionPorts {
        async fn get(&self, _id: WorkId) -> Result<Option<Work>, AppError> {
            Ok(Some(self.work()))
        }
        async fn save(&self, _work: &Work) -> Result<(), AppError> {
            Ok(())
        }
        async fn list(&self, _limit: u32, _offset: u32) -> Result<Vec<Work>, AppError> {
            Ok(vec![])
        }
        async fn list_sorted(
            &self,
            _order: haven_domain::WorkOrder,
            _limit: u32,
            _offset: u32,
        ) -> Result<Vec<Work>, AppError> {
            Ok(vec![])
        }
        async fn list_filtered(
            &self,
            _order: haven_domain::WorkOrder,
            _category: Option<haven_domain::enums::ContentCategory>,
            _media_types: Option<&[MediaType]>,
            _query: Option<&str>,
            _limit: u32,
            _offset: u32,
        ) -> Result<Vec<Work>, AppError> {
            Ok(vec![])
        }
        async fn count_filtered(
            &self,
            _category: Option<haven_domain::enums::ContentCategory>,
            _media_types: Option<&[MediaType]>,
            _query: Option<&str>,
        ) -> Result<u64, AppError> {
            Ok(0)
        }
        async fn delete(&self, _id: WorkId) -> Result<bool, AppError> {
            Ok(false)
        }
        async fn id_for_source_ref(
            &self,
            _provider: &str,
            _external_id: &str,
        ) -> Result<Option<WorkId>, AppError> {
            Ok(None)
        }
        async fn has_any_source_ref(&self, _id: WorkId) -> Result<bool, AppError> {
            Ok(false)
        }
        async fn save_source_ref(
            &self,
            _provider: &str,
            _external_id: &str,
            _work_id: WorkId,
        ) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[async_trait]
    impl haven_domain::contracts::EditionRepository for FakeSessionPorts {
        async fn get(&self, _id: EditionId) -> Result<Option<Edition>, AppError> {
            Ok(Some(self.edition()))
        }
        async fn save(&self, _edition: &Edition) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_by_work(&self, _work_id: WorkId) -> Result<Vec<Edition>, AppError> {
            Ok(vec![])
        }
        async fn delete(&self, _id: EditionId) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    #[async_trait]
    impl haven_domain::contracts::MediaItemRepository for FakeSessionPorts {
        async fn get(&self, id: MediaItemId) -> Result<Option<MediaItem>, AppError> {
            if id != self.media_item_id {
                return Ok(None);
            }
            // prepare 真正进入后在此处等待，测试据此确定性地制造慢速解析。
            self.started.notify_one();
            let hold = self.take();
            if let Some(rx) = hold {
                let _ = rx.await;
            }
            Ok(Some(self.media_item(EditionId::new())))
        }
        async fn save(&self, _item: &MediaItem) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_by_edition(
            &self,
            _edition_id: EditionId,
        ) -> Result<Vec<MediaItem>, AppError> {
            Ok(vec![])
        }
        async fn delete(&self, _id: MediaItemId) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    #[async_trait]
    impl haven_domain::contracts::ResourceRepository for FakeSessionPorts {
        async fn get(&self, _id: ResourceId) -> Result<Option<Resource>, AppError> {
            Ok(None)
        }
        async fn save(&self, _resource: &Resource) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_by_media_item(
            &self,
            media_item_id: MediaItemId,
        ) -> Result<Vec<Resource>, AppError> {
            if media_item_id != self.media_item_id {
                return Ok(vec![]);
            }
            Ok(self.resources())
        }
        async fn delete(&self, _id: ResourceId) -> Result<bool, AppError> {
            Ok(false)
        }
        async fn mark_unavailable_by_storage(
            &self,
            _storage_location_id: haven_domain::StorageLocationId,
            _availability: Availability,
        ) -> Result<u64, AppError> {
            Ok(0)
        }
        async fn delete_by_storage(
            &self,
            _storage_location_id: haven_domain::StorageLocationId,
        ) -> Result<u64, AppError> {
            Ok(0)
        }
    }

    #[async_trait]
    impl haven_domain::contracts::StorageLocationRepository for FakeSessionPorts {
        async fn get(
            &self,
            _id: haven_domain::StorageLocationId,
        ) -> Result<Option<haven_domain::StorageLocation>, AppError> {
            Ok(None)
        }
        async fn save(&self, _location: &haven_domain::StorageLocation) -> Result<(), AppError> {
            Ok(())
        }
        async fn list(&self) -> Result<Vec<haven_domain::StorageLocation>, AppError> {
            Ok(vec![])
        }
        async fn delete(&self, _id: haven_domain::StorageLocationId) -> Result<bool, AppError> {
            Ok(false)
        }
        async fn clear_credential_ref(
            &self,
            _id: haven_domain::StorageLocationId,
        ) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    #[async_trait]
    impl haven_domain::contracts::ProgressRepository for FakeSessionPorts {
        async fn get_for_media_item(
            &self,
            _media_item_id: MediaItemId,
        ) -> Result<Option<haven_domain::Progress>, AppError> {
            Ok(None)
        }
        async fn save(&self, _progress: &haven_domain::Progress) -> Result<(), AppError> {
            Ok(())
        }
        async fn save_if_revision(
            &self,
            _progress: &haven_domain::Progress,
            _expected_revision: Option<&str>,
        ) -> Result<Option<String>, AppError> {
            Ok(None)
        }
        async fn mark_completed(
            &self,
            _progress: &haven_domain::Progress,
        ) -> Result<String, AppError> {
            Ok("r".into())
        }
        async fn recent(&self, _limit: u32) -> Result<Vec<haven_domain::Progress>, AppError> {
            Ok(vec![])
        }
    }

    /// 记录调用的设备控制面；第 N 次 play（按调用顺序）可挂起/失败，用于制造
    /// "B 在 A 持锁控制中到达""B 在 A 旧 stop 持锁控制中到达"等交错。
    #[derive(Default)]
    pub struct FakeControl {
        pub plays: Mutex<Vec<(String, String)>>,
        pub stops: Mutex<Vec<String>>,
        pub fail_plays: AtomicU64,
        pub fail_stops: AtomicU64,
        gates: Mutex<HashMap<u64, tokio::sync::oneshot::Receiver<()>>>,
        /// 每次 play 进入控制面时以 `notify_one` 存入一张许可：即使测试稍后才
        /// 等待也不会丢失通告（`notify_waiters` 会丢掉无等待者时的通知）。
        play_started: tokio::sync::Notify,
    }

    impl FakeControl {
        /// 挂起第 `index` 次（从 0 起）play 的返回；返回的 sender 放行该次 play。
        pub fn hold_play(&self, index: u64) -> tokio::sync::oneshot::Sender<()> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.gates
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(index, rx);
            tx
        }

        /// 等待一次 play 真正进入控制面。
        pub async fn wait_play_started(&self) {
            self.play_started.notified().await;
        }

        pub fn stop_count(&self) -> usize {
            self.stops.lock().unwrap_or_else(|e| e.into_inner()).len()
        }

        pub fn play_count(&self) -> usize {
            self.plays.lock().unwrap_or_else(|e| e.into_inner()).len()
        }
    }

    fn injected() -> AppError {
        AppError::new("TEST_INJECTED", ErrorKind::Internal, "injected", false)
    }

    #[async_trait]
    impl CastControlPort for FakeControl {
        async fn play(&self, device_id: &str, lan_url: &str, _title: &str) -> Result<(), AppError> {
            let index = {
                let mut p = self.plays.lock().unwrap_or_else(|e| e.into_inner());
                p.push((device_id.to_owned(), lan_url.to_owned()));
                p.len() as u64 - 1
            };
            let gate = self
                .gates
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&index);
            if let Some(rx) = gate {
                self.play_started.notify_one();
                let _ = rx.await;
            }
            if self.fail_plays.load(Ordering::SeqCst) > 0 {
                self.fail_plays.fetch_sub(1, Ordering::SeqCst);
                return Err(injected());
            }
            Ok(())
        }
        async fn stop(&self, device_id: &str) -> Result<(), AppError> {
            self.stops
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(device_id.to_owned());
            if self.fail_stops.load(Ordering::SeqCst) > 0 {
                self.fail_stops.fetch_sub(1, Ordering::SeqCst);
                return Err(injected());
            }
            Ok(())
        }
        async fn status(&self, _device_id: &str) -> Result<CastTransportStateDto, AppError> {
            Ok(CastTransportStateDto::Playing)
        }
        async fn position(&self, _device_id: &str) -> Result<(Option<u64>, Option<u64>), AppError> {
            Ok((None, None))
        }
    }

    /// 记录 grant 注册/撤销的内存媒体端口。
    #[derive(Default)]
    pub struct FakeMedia {
        pub held: Mutex<Vec<String>>,
    }

    impl FakeMedia {
        pub fn held(&self) -> Vec<String> {
            self.held.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    impl CastMediaPort for FakeMedia {
        fn register(&self, grant: String, _upstream_url: String, _title: String) -> String {
            self.held
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(grant.clone());
            format!("http://127.0.0.1:3500/cast/{}", grant)
        }
        fn revoke(&self, grant: &str) -> bool {
            let mut h = self.held.lock().unwrap_or_else(|e| e.into_inner());
            let before = h.len();
            h.retain(|g| g != grant);
            h.len() != before
        }
        fn lan_url(&self, grant: &str) -> Option<String> {
            Some(format!("http://127.0.0.1:3500/cast/{}", grant))
        }
    }

    pub struct NoopDiscovery;
    #[async_trait]
    impl CastDiscoveryPort for NoopDiscovery {
        async fn discover(&self, _timeout_ms: u32) -> Result<Vec<CastDeviceDto>, AppError> {
            Ok(vec![CastDeviceDto {
                device_id: "test-dlna-1".into(),
                friendly_name: "测试电视".into(),
                ip: "192.168.1.100".into(),
                protocol: CastProtocolDto::Dlna,
                model_name: Some("TestTV".into()),
            }])
        }
    }

    pub struct NoopControl;
    #[async_trait]
    impl CastControlPort for NoopControl {
        async fn play(
            &self,
            _device_id: &str,
            _lan_url: &str,
            _title: &str,
        ) -> Result<(), AppError> {
            Ok(())
        }
        async fn stop(&self, _device_id: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn status(&self, _device_id: &str) -> Result<CastTransportStateDto, AppError> {
            Ok(CastTransportStateDto::Playing)
        }
        async fn position(&self, _device_id: &str) -> Result<(Option<u64>, Option<u64>), AppError> {
            Ok((Some(5000), Some(1526000)))
        }
    }

    pub struct NoopMedia;
    #[async_trait]
    impl CastMediaPort for NoopMedia {
        fn register(&self, grant: String, _upstream_url: String, _title: String) -> String {
            format!("http://127.0.0.1:3500/cast/{}", grant)
        }
        fn revoke(&self, _grant: &str) -> bool {
            true
        }
        fn lan_url(&self, grant: &str) -> Option<String> {
            Some(format!("http://127.0.0.1:3500/cast/{}", grant))
        }
    }
}

// C1 投屏设备所有权并发回归。
#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use tokio::time::timeout;

    use super::test_doubles::{FakeControl, FakeMedia, FakeSessionPorts};
    use super::*;

    const DEV_A: &str = "dlna-a";
    const DEV_B: &str = "dlna-b";

    fn service(ports: Arc<FakeSessionPorts>) -> (CastService, Arc<FakeControl>, Arc<FakeMedia>) {
        let control = Arc::new(FakeControl::default());
        let media = Arc::new(FakeMedia::default());
        let stream = StreamService::new(ports);
        let svc = CastService::new(
            Arc::new(super::test_doubles::NoopDiscovery),
            control.clone(),
            media.clone(),
            stream,
            Arc::new(CastGrantRegistry::new("http://127.0.0.1:3500".into())),
        );
        (svc, control, media)
    }

    /// 每个等待都有上限：被挂起的 future 永不返回时测试失败而不是挂死 CI。
    const WAIT: std::time::Duration = std::time::Duration::from_secs(5);

    /// 设备 A 正在推送 A 的 grant（控制面 Play 已进入、尚未返回）。
    #[tokio::test]
    async fn stop_arriving_during_held_control_play_stays_safe() {
        // 只在持有设备控制锁（控制面 Play 尚未返回）时投入后续请求，避免依赖时序。
        let ports = Arc::new(FakeSessionPorts::new(true));
        let play_id = ports.media_id();
        let (svc, control, media) = service(ports);
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let release = control.hold_play(0);
                let svc_a = svc.clone();
                let req_a = CastPlayRequest {
                    media_item_id: play_id,
                    device_id: DEV_A.into(),
                    engine: SessionEngineDto::Playback,
                };
                let a = tokio::task::spawn_local(async move { svc_a.play(req_a).await });
                control.wait_play_started().await;
                // 设备上尚无任何已确认的 owner：此刻没有授权可撤销，也不发 Stop。
                let early = svc
                    .stop(CastStopRequest {
                        cast_session_id: "unknown-grant".into(),
                    })
                    .await
                    .expect("unknown grant stop");
                assert!(early.stopped);
                assert_eq!(control.stop_count(), 0);
                release.send(()).unwrap();
                a.await.unwrap().expect("A play");
            })
            .await;
        assert_eq!(media.held().len(), 1);
    }

    /// B 在 A 持锁执行控制面 play 时到达：A 物理成功仍被记录为 owner，B 排队
    /// 并随后取代它；此后 A 的 stop 不发 Stop，B 的 stop 恰好一次，全部撤销后
    /// 媒体端口为空。
    #[tokio::test]
    async fn newer_play_queues_behind_held_control_and_replaces_owner() {
        // B 在 A 持有设备控制锁时同步启动（single-thread 下 A 被门控挂起，B 必然
        // 停在设备锁上；不会出现 A 与 B 同时越过意图校验的非串行顺序）。
        let (svc, a, b, control, media) = tokio::task::LocalSet::new()
            .run_until(async {
                let ports = FakeSessionPorts::new(true);
                let id = ports.media_id();
                let (svc, control, media) = service(Arc::new(ports));
                let release = control.hold_play(0);
                let svc_a = svc.clone();
                let req_a = CastPlayRequest {
                    media_item_id: id.clone(),
                    device_id: DEV_A.into(),
                    engine: SessionEngineDto::Playback,
                };
                let a = tokio::task::spawn_local(async move { svc_a.play(req_a).await });
                control.wait_play_started().await;
                let svc_b = svc.clone();
                let req_b = CastPlayRequest {
                    media_item_id: id,
                    device_id: DEV_A.into(),
                    engine: SessionEngineDto::Playback,
                };
                let b = tokio::task::spawn_local(async move { svc_b.play(req_b).await });
                tokio::task::yield_now().await;
                assert!(!b.is_finished(), "B 必须排队等待 A 的设备控制");
                assert_eq!(control.play_count(), 1, "A 仍持有控制面");
                release.send(()).unwrap();
                let a = a.await.unwrap().expect("A 的旧 play 不因 B 排队被拒");
                let b = b.await.unwrap().expect("B play");
                (svc, a, b, control, media)
            })
            .await;
        assert_eq!(control.play_count(), 2);
        assert_eq!(media.held().len(), 2);
        // A 物理成功的 grant 在新 B 成功后仍可被其 stop 识别为过期并撤销。
        svc.stop(CastStopRequest {
            cast_session_id: a.cast_session_id,
        })
        .await
        .expect("stale stop");
        assert_eq!(control.stop_count(), 0, "过期 grant 不得下发 Stop");
        svc.stop(CastStopRequest {
            cast_session_id: b.cast_session_id.clone(),
        })
        .await
        .expect("stop B");
        assert_eq!(control.stop_count(), 1);
        assert!(media.held().is_empty());
    }

    /// 旧 stop 在 B 持锁执行控制面 play 时到达：等到 B 完成、owner 已是 B，
    /// 旧 stop 只撤销自身 grant，不向设备发送 Stop。
    #[tokio::test]
    async fn stale_stop_arriving_during_newer_held_play_never_stops() {
        let ports = FakeSessionPorts::new(true);
        let id = ports.media_id();
        let (svc, control, media) = service(Arc::new(ports));
        let play = |svc: CastService, media_item_id: String| {
            let req = CastPlayRequest {
                media_item_id,
                device_id: DEV_A.into(),
                engine: SessionEngineDto::Playback,
            };
            tokio::task::spawn_local(async move { svc.play(req).await })
        };
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let a = play(svc.clone(), id.clone())
                    .await
                    .unwrap()
                    .expect("A play");
                let release = control.hold_play(1);
                let b = play(svc.clone(), id.clone());
                control.wait_play_started().await;
                assert_eq!(control.play_count(), 2, "B 已进入控制面");
                // B 持有设备锁，旧 stop 排队在后面。
                let stop_svc = svc.clone();
                let stop_task = tokio::task::spawn_local(async move {
                    stop_svc
                        .stop(CastStopRequest {
                            cast_session_id: a.cast_session_id.clone(),
                        })
                        .await
                });
                tokio::task::yield_now().await;
                assert!(!stop_task.is_finished(), "stop 必须等待同设备控制串行");
                release.send(()).unwrap();
                let b = b.await.unwrap().expect("B play");
                stop_task.await.unwrap().expect("stale stop");
                assert_eq!(control.stop_count(), 0, "旧 grant stop 不得停掉 B");
                assert_eq!(media.held(), vec![b.cast_session_id]);
            })
            .await;
    }

    /// 更晚的 B 在 prepare 失败：设备上仍是 A 的物理播放，A 必须仍可停止；
    /// 更晚的意图不得让 A 变成"不可停止"。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn newer_prepare_failure_keeps_physical_owner_stoppable() {
        let ports = FakeSessionPorts::new(true);
        let id = ports.media_id();
        let (svc, control, media) = service(Arc::new(ports));
        let a = svc
            .play(CastPlayRequest {
                media_item_id: id,
                device_id: DEV_A.into(),
                engine: SessionEngineDto::Playback,
            })
            .await
            .expect("A play");
        // 未知条目：同一服务（同一流端口）中不存在，prepare 必须失败。
        let bad = svc
            .play(CastPlayRequest {
                media_item_id: "unknown-media-item".into(),
                device_id: DEV_A.into(),
                engine: SessionEngineDto::Playback,
            })
            .await;
        assert!(bad.is_err(), "未知条目的 prepare 必须失败");
        assert_eq!(control.play_count(), 1);
        svc.stop(CastStopRequest {
            cast_session_id: a.cast_session_id.clone(),
        })
        .await
        .expect("A stop");
        assert_eq!(control.stop_count(), 1);
        assert!(media.held().is_empty());
    }

    /// 旧实例的慢速 prepare 晚于新实例 play 返回：旧 play 必须被拒且不推流。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_stale_prepare_cannot_supersede_newer_play() {
        // 同一个 Arc<FakeSessionPorts>：只挂起第一次条目读取，两次 play 使用
        // 同一 media id；第二次读取在 hold 被取走后直接通过。
        let ports = Arc::new(FakeSessionPorts::new(true));
        let id = ports.media_id();
        let (tx, rx) = tokio::sync::oneshot::channel();
        ports.install(rx);
        let (svc, control, media) = service(ports.clone());
        let req = || CastPlayRequest {
            media_item_id: id.clone(),
            device_id: DEV_A.into(),
            engine: SessionEngineDto::Playback,
        };
        let svc_slow = svc.clone();
        let slow_req = req();
        let old = tokio::spawn(async move { svc_slow.play(slow_req).await });
        timeout(WAIT, ports.wait_prepare_started())
            .await
            .expect("慢速 prepare 必须开始");
        let new = timeout(WAIT, svc.play(req()))
            .await
            .expect("新 play 不得被 hung 的 prepare 阻塞")
            .expect("new play");
        tx.send(()).expect("release slow prepare");
        let old = timeout(WAIT, old)
            .await
            .expect("旧 play 必须结束")
            .unwrap()
            .unwrap_err();
        assert_eq!(old.code().as_str(), "CAST_SUPERSEDED");
        assert_eq!(control.play_count(), 1, "旧慢速 play 不得再推流");
        assert_eq!(media.held(), vec![new.cast_session_id.clone()]);
        // 新会话仍可正常停止，并且设备只被停止一次。
        svc.stop(CastStopRequest {
            cast_session_id: new.cast_session_id,
        })
        .await
        .expect("stop new");
        assert_eq!(control.stop_count(), 1);
        assert!(media.held().is_empty());
    }

    /// 不同设备互不阻塞：B 上的 play 在 A 的控制尚未返回时也能完成。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn different_devices_control_concurrently() {
        // ports 留在本任务内以避免克隆另一台 fake；id 先取出。
        let ports = Arc::new(FakeSessionPorts::new(true));
        let id = ports.media_id();
        let (svc, control, _media) = service(ports);
        let req = |device: &str| CastPlayRequest {
            media_item_id: id.clone(),
            device_id: device.into(),
            engine: SessionEngineDto::Playback,
        };
        let release = control.hold_play(0);
        let svc_a = svc.clone();
        let req_a = req(DEV_A);
        let a = tokio::spawn(async move { svc_a.play(req_a).await });
        timeout(WAIT, control.wait_play_started())
            .await
            .expect("A 必须进入控制面");
        let b = timeout(WAIT, svc.play(req(DEV_B)))
            .await
            .expect("B 不得被 A 阻塞")
            .expect("B play");
        let a_pending = !a.is_finished();
        release.send(()).unwrap();
        timeout(WAIT, a)
            .await
            .expect("A 必须被放行")
            .unwrap()
            .expect("A play");
        assert!(a_pending, "A 仍被挂起时 B 已完成");
        assert!(!b.cast_session_id.is_empty());
        let mut plays = control
            .plays
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        plays.sort();
        assert_eq!(plays.len(), 2);
        assert_eq!(plays[0].0, DEV_A);
        assert_eq!(plays[1].0, DEV_B);
    }

    /// 设备停止失败：grant 保留，重复 stop 可重试并在成功后撤销。
    #[tokio::test]
    async fn failed_stop_remains_retryable() {
        let ports = FakeSessionPorts::new(true);
        let id = ports.media_id();
        let (svc, control, media) = service(Arc::new(ports));
        let played = svc
            .play(CastPlayRequest {
                media_item_id: id,
                device_id: DEV_A.into(),
                engine: SessionEngineDto::Playback,
            })
            .await
            .expect("play");
        control.fail_stops.store(1, Ordering::SeqCst);
        let req = CastStopRequest {
            cast_session_id: played.cast_session_id.clone(),
        };
        let err = svc.stop(req.clone()).await.unwrap_err();
        assert_eq!(err.code().as_str(), "TEST_INJECTED");
        assert_eq!(media.held(), vec![played.cast_session_id.clone()]);
        assert!(
            svc.grant_registry
                .lan_url(&played.cast_session_id)
                .is_some()
        );
        svc.stop(req).await.expect("retry");
        assert_eq!(control.stop_count(), 2);
        assert!(media.held().is_empty());
        // 再次 stop 幂等。
        svc.stop(CastStopRequest {
            cast_session_id: played.cast_session_id,
        })
        .await
        .expect("idempotent");
        assert_eq!(control.stop_count(), 2);
    }

    /// 非 HLS 分支：控制面无状态，失败时媒体与 registry 双双回滚。
    #[tokio::test]
    async fn failed_play_rolls_back_grant_on_plain_stream() {
        let ports = FakeSessionPorts::new(false);
        let id = ports.media_id();
        let (svc, control, media) = service(Arc::new(ports));
        control.fail_plays.store(1, Ordering::SeqCst);
        let err = svc
            .play(CastPlayRequest {
                media_item_id: id,
                device_id: DEV_A.into(),
                engine: SessionEngineDto::Playback,
            })
            .await
            .unwrap_err();
        assert_eq!(err.code().as_str(), "TEST_INJECTED");
        assert!(media.held().is_empty());
        assert_eq!(svc.grant_registry.inner.lock().unwrap().len(), 0);
    }
}
