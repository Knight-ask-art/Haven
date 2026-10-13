//! 共享夹具：真实 SQLite + 真实 `SettingsUoW` 的来源注册表、**显式注入**的外部凭据 I/O
//! 租约、可注入失败/可确定交错的外部凭据端口，以及落库负载的读写。
//!
//! 为什么这些用例必须是 integration test：`SqliteSettingsUoW`（haven-infrastructure）
//! 实现的是 **normal** haven-application 实例里的 `SettingsUoW`，lib-test 实例里
//! `Arc::new(SqliteSettingsUoW)` 无法满足 `Arc<dyn SettingsUoW>`（E0277）。凭据生命周期
//! 的每一条保证（暂存先于外部写入、采纳栅栏、清理队列跨重启、外部 I/O 独占租约）都建立
//! 在这个真实事务之上，所以它们只能在这里跑。
//!
//! 租约一律**显式注入**，从不隐式兜底：
//! - [`Harness`] 用进程内租约（内存数据库不与任何其它进程共享事实，进程内独占就是它正确
//!   的范围）；同一个 Harness 的**所有**装配面共享同一个租约实例，因此「两个独立写入者」
//!   在进程内也能真实地互相排斥。
//! - [`FileHarness`] 用临时**文件**数据库 + 真实
//!   `FileSourceCredentialLease`，用于「两个独立构造的租约实例在同一把锁文件上竞争」。
//!
//! 交错一律由**确定性的构造**控制：要么让外部端口在 `set`/`delete` 期间跑一段真实的并发
//! 用例（[`DuringWrite`]），要么让写入者在外部 IO 里挂起直到测试放行（[`HoldGate`]）。
//! 没有任何 sleep。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use haven_application::services::settings::{SettingsTxPorts, SettingsUoW};
use haven_application::services::source_config_cache::InMemorySourceConfigCache;
use haven_application::services::source_registry::{
    InMemorySourceCredentialLease, SOURCES_SETTINGS_SECTION, SourceCredentialLease,
    SourceRegistryService,
};
use haven_application::wire::SourceSetCredentialRequest;
use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::contracts::{SettingsRepository, SettingsRow};
use haven_domain::credential::{CredentialStore, SecretString};
use haven_domain::ids::CredentialRef;
use haven_infrastructure::db::repos::{SqliteRepositories, SqliteSettingsUoW};
use haven_infrastructure::{Db, FileSourceCredentialLease};
use tokio::sync::Notify;

/// 真实 SQLite（内存）+ 真实设置事务 + 显式注入租约的来源注册表，以及同一个数据库的句柄。
pub struct Harness {
    pub db: Arc<Db>,
    pub repos: Arc<SqliteRepositories>,
    pub registry: SourceRegistryService,
    /// 本 Harness 全部分支装配面共享的那一份租约。
    lease: Arc<dyn SourceCredentialLease>,
}

impl Harness {
    pub fn new() -> Self {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let lease: Arc<dyn SourceCredentialLease> = Arc::new(InMemorySourceCredentialLease::new());
        let registry = SourceRegistryService::new(repos.clone())
            .with_settings_uow(Arc::new(SqliteSettingsUoW::new(db.clone())))
            .with_credential_lease(lease.clone());
        Self {
            db,
            repos,
            registry,
            lease,
        }
    }

    /// 重启等价：在**同一个数据库**上新建一个服务实例（内存状态一律重建），
    /// 但共享同一份租约——重启的是服务，不是这台机器上的写者仲裁。
    pub fn restart(&self) -> SourceRegistryService {
        self.independent()
    }

    /// 另一个**独立构造**的注册表实例：同一个数据库、同一份租约。它模拟「同一台机器上的
    /// 第二个写入者」，因此两个实例之间的外部凭据 I/O 真正互相排斥。
    pub fn independent(&self) -> SourceRegistryService {
        SourceRegistryService::new(self.repos.clone())
            .with_settings_uow(Arc::new(SqliteSettingsUoW::new(self.db.clone())))
            .with_credential_lease(self.lease.clone())
    }

    /// 用注入的设置事务（例如会在第 N 次写事务上失败的实现）重建同一个注册表。
    pub fn with_uow(&self, uow: Arc<dyn SettingsUoW>) -> SourceRegistryService {
        SourceRegistryService::new(self.repos.clone())
            .with_settings_uow(uow)
            .with_credential_lease(self.lease.clone())
    }

    /// 没有注入租约的装配面：凭据写入/清除、来源移除与清理扫描必须 fail closed。
    pub fn without_lease(&self) -> SourceRegistryService {
        SourceRegistryService::new(self.repos.clone())
            .with_settings_uow(Arc::new(SqliteSettingsUoW::new(self.db.clone())))
    }

    /// 落库的来源负载 JSON（暂存槽与清理队列只在这里可见）。
    pub async fn stored_sources(&self) -> serde_json::Value {
        stored_sources(self.repos.as_ref()).await
    }

    /// 覆盖落库的来源负载（构造历史数据与「账实不符」的畸形状态）。
    pub async fn overwrite_sources(&self, value: &serde_json::Value) {
        overwrite_stored_sources(self.repos.as_ref(), value).await
    }
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

/// 临时**文件**数据库 + 真实文件锁租约：每个装配面各自构造一个独立的
/// [`FileSourceCredentialLease`]，共用同一把由数据库目录决定的锁文件。
pub struct FileHarness {
    /// 临时目录必须在整个用例期间存活。
    _dir: tempfile::TempDir,
    pub db: Arc<Db>,
    pub repos: Arc<SqliteRepositories>,
    pub registry: SourceRegistryService,
}

impl FileHarness {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("创建临时目录");
        let db = Arc::new(Db::open(&dir.path().join("haven.db")).expect("打开文件数据库"));
        let repos = Arc::new(SqliteRepositories::new(db.clone()));
        let registry = Self::assemble(&db, &repos);
        Self {
            _dir: dir,
            db,
            repos,
            registry,
        }
    }

    fn assemble(db: &Arc<Db>, repos: &Arc<SqliteRepositories>) -> SourceRegistryService {
        let lease = FileSourceCredentialLease::from_db(db).expect("文件数据库必须能给出跨进程租约");
        SourceRegistryService::new(repos.clone())
            .with_settings_uow(Arc::new(SqliteSettingsUoW::new(db.clone())))
            .with_credential_lease(Arc::new(lease))
    }

    /// 第二个**独立构造**的装配面：另一个租约实例、同一把锁文件。
    pub fn independent(&self) -> SourceRegistryService {
        Self::assemble(&self.db, &self.repos)
    }

    pub fn lock_file(&self) -> std::path::PathBuf {
        FileSourceCredentialLease::from_db(&self.db)
            .expect("文件数据库必须能给出跨进程租约")
            .lock_file()
            .to_path_buf()
    }

    pub async fn stored_sources(&self) -> serde_json::Value {
        stored_sources(self.repos.as_ref()).await
    }
}

/// 落库的来源负载 JSON（暂存槽与清理队列只在这里可见）。
pub async fn stored_sources(repos: &SqliteRepositories) -> serde_json::Value {
    let row = SettingsRepository::get(repos, SOURCES_SETTINGS_SECTION)
        .await
        .unwrap()
        .expect("sources 行必须已经存在");
    serde_json::from_str(&row.data_json).expect("来源负载必须是合法 JSON")
}

/// 覆盖落库的来源负载（用于构造历史数据与「账实不符」的畸形状态）。
pub async fn overwrite_stored_sources(repos: &SqliteRepositories, value: &serde_json::Value) {
    SettingsRepository::upsert(
        repos,
        &SettingsRow {
            section: SOURCES_SETTINGS_SECTION.to_owned(),
            schema_version: 4,
            revision: format!("test-revision-{}", uuid::Uuid::new_v4().simple()),
            data_json: serde_json::to_string(value).unwrap(),
            updated_at: UtcMillis::now(),
        },
    )
    .await
    .unwrap();
}

/// 落库负载里某个数组字段的长度（缺失按 0）。
pub fn len_of(value: &serde_json::Value, field: &str) -> usize {
    value
        .get(field)
        .and_then(|entries| entries.as_array())
        .map(Vec::len)
        .unwrap_or(0)
}

/// 来源身份 → 槽位 provider 段（与生产实现同一套命名规则：`sha256(sourceId)` 前 24
/// 字节的 48 个小写十六进制字符）。
///
/// 用例自己重算一遍，而不是从实现里导出：断言的是「槽位确实按来源身份 + 种类生成」这个
/// **外部可观察**的事实，实现一旦漂移（例如退回用 sourceId 拼 target），这里立刻失败。
pub fn scoped_provider_for(kind_provider: &str, source_id: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(source_id.as_bytes());
    let mut scope = String::with_capacity(48);
    for byte in digest.iter().take(24) {
        let byte = *byte;
        scope.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        scope.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    format!("{kind_provider}_{scope}")
}

/// 构造一条**真实形状**的来源作用域版本槽（随机 UUID profile）。
pub fn source_scoped_slot(kind_provider: &str, source_id: &str) -> String {
    format!(
        "haven:{}:{}",
        scoped_provider_for(kind_provider, source_id),
        uuid::Uuid::new_v4().simple()
    )
}

/// 外部 IO（`set` 或 `delete`）期间要并发跑的一段真实用例。它是**确定性交错**的载体：
/// 真实 keystore 里插不进这段窗口，而它正是「外部 IO 期间来源被删除/被改写」要覆盖的地方。
///
/// 钩子是一次性的：下一次外部 IO 会取走它并执行，之后的 IO 不再触发。
pub enum DuringWrite {
    /// 外部 IO 期间改来源端点（推进该来源的配置版本）。
    ///
    /// 配置变更**不持租约**（它不碰外部凭据库），因此这是唯一还能在外部写入期间合法发生
    /// 的来源变更；采纳栅栏必须因此失效。
    UpdateEndpoint {
        registry: SourceRegistryService,
        source_id: String,
        endpoint: String,
    },
    /// 外部写入期间尝试**合规地**删除整个来源。
    ///
    /// 移除同样要持租约，所以这里必然拿到 `SOURCE_CREDENTIAL_BUSY`：这正是「一次外部写入
    /// 期间不存在第二个合规写者」的直接观测。结果记在 [`MemoryCredentialStore::hook_outcomes`]。
    RemoveSource {
        registry: SourceRegistryService,
        source_id: String,
        cache: Arc<InMemorySourceConfigCache>,
    },
    /// 外部写入期间尝试**合规地**清除该来源凭据（同样必然 BUSY）。
    ClearCredential {
        registry: SourceRegistryService,
        source_id: String,
    },
    /// **非合规**写者：绕过注册表与租约，直接在数据库里删掉来源记录。
    ///
    /// 合规路径已经做不出这件事（移除要持租约）。这里保留它是为了让采纳栅栏的
    /// 「来源已消失」分支继续被真实覆盖，而不是因为「理论上不可达」就没人再验证它。
    ForgeSourceRemoval {
        repos: Arc<SqliteRepositories>,
        source_id: String,
    },
    /// **非合规**写者：绕过注册表与租约，直接在数据库里撤销该来源的凭据引用并把该引用
    /// 登记进清理队列（等价于一次合规清除，但不持租约）。
    ///
    /// 用来钉住「扫描按身份出队，绝不覆盖并发新增的义务」这条不变式。
    ForgeRevokedCredential {
        repos: Arc<SqliteRepositories>,
        source_id: String,
    },
}

/// 让写入者在外部 IO 中途挂起直到测试放行的门。
///
/// 这是「另一个写入者此刻正在写外部存储」的**确定性**表达：没有 sleep，也没有靠调度
/// 概率去抢时序。释放之后写入者继续跑完整个「外部写入 → 采纳」区间。
pub struct HoldGate {
    entered: Notify,
    release: Notify,
}

impl HoldGate {
    pub fn new() -> Self {
        Self {
            entered: Notify::new(),
            release: Notify::new(),
        }
    }

    /// 等写入者真的进入了外部 IO（它已经过了暂存、正在写系统凭据库）。
    pub async fn entered(&self) {
        self.entered.notified().await;
    }

    /// 放行：写入者继续完成外部写入与采纳。
    pub fn release(&self) {
        self.release.notify_one();
    }
}

impl Default for HoldGate {
    fn default() -> Self {
        Self::new()
    }
}

/// 记录每一次外部 IO 的内存凭据库。
///
/// - 可注入 `set` / `delete` 失败（模拟系统凭据库不可用）；
/// - 可在 `set` 期间并发跑一段真实用例（[`DuringWrite`]）；
/// - 可让下一次 `set` 挂起在外部 IO 中间（[`HoldGate`]），用来证明「外部写入期间没有
///   第二个合规写者」。
pub struct MemoryCredentialStore {
    entries: Mutex<HashMap<String, String>>,
    writes: Mutex<Vec<String>>,
    deletes: Mutex<Vec<String>>,
    fail_writes: AtomicBool,
    fail_deletes: AtomicBool,
    /// 只对这些引用失败（模拟「个别条目删不掉」），其余删除照常。
    fail_delete_targets: Mutex<HashSet<String>>,
    hold: Mutex<Option<Arc<HoldGate>>>,
    during_io: Mutex<Option<DuringWrite>>,
    /// 钩子里那次并发用例的真实结果（`"ok"` 或错误码），供用例断言。
    hook_outcomes: Mutex<Vec<String>>,
}

impl Default for MemoryCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryCredentialStore {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            writes: Mutex::new(Vec::new()),
            deletes: Mutex::new(Vec::new()),
            fail_writes: AtomicBool::new(false),
            fail_deletes: AtomicBool::new(false),
            fail_delete_targets: Mutex::new(HashSet::new()),
            hold: Mutex::new(None),
            during_io: Mutex::new(None),
            hook_outcomes: Mutex::new(Vec::new()),
        }
    }

    /// 预置一条「已经在系统凭据库里」的秘密（历史数据、外部写入的遗留物）。
    pub fn seed(&self, reference: &str, secret: &str) {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(reference.to_owned(), secret.to_owned());
    }

    pub fn fail_writes(&self, fail: bool) {
        self.fail_writes.store(fail, Ordering::SeqCst);
    }

    pub fn fail_deletes(&self, fail: bool) {
        self.fail_deletes.store(fail, Ordering::SeqCst);
    }

    /// 只让这一条引用的删除失败（其余删除照常）。
    pub fn fail_delete_for(&self, reference: &str) {
        self.fail_delete_targets
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(reference.to_owned());
    }

    /// 恢复所有引用的删除（模拟系统凭据库重新可用）。
    pub fn allow_all_deletes(&self) {
        self.fail_delete_targets
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        self.fail_deletes.store(false, Ordering::SeqCst);
    }

    /// 让**下一次** `set` 挂起在外部 IO 中途，直到 [`HoldGate::release`]。
    pub fn hold_next_set(&self, gate: Arc<HoldGate>) {
        *self.hold.lock().unwrap_or_else(|error| error.into_inner()) = Some(gate);
    }

    pub fn during_write(&self, action: DuringWrite) {
        *self
            .during_io
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(action);
    }

    /// 真实写入过外部存储的引用（按发生顺序）。
    pub fn writes(&self) -> Vec<String> {
        self.writes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// 真实调用过删除的引用（按发生顺序，含目标本就不存在的调用）。
    pub fn deletes(&self) -> Vec<String> {
        self.deletes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// 钩子里那次并发用例的真实结果（按发生顺序）。
    pub fn hook_outcomes(&self) -> Vec<String> {
        self.hook_outcomes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// 外部存储里仍然存在的引用（按字典序）。
    pub fn live_references(&self) -> Vec<String> {
        let mut live: Vec<String> = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .keys()
            .cloned()
            .collect();
        live.sort();
        live
    }

    pub fn contains(&self, reference: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_key(reference)
    }

    fn note_outcome(&self, outcome: Result<(), AppError>) {
        let recorded = match outcome {
            Ok(()) => "ok".to_owned(),
            Err(error) => error.code().as_str().to_owned(),
        };
        self.hook_outcomes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(recorded);
    }
}

/// 系统凭据库不可用时的稳定错误码（与生产实现同族）。
fn credential_unavailable() -> AppError {
    AppError::new(
        "CREDENTIAL_ACCESS_FAILED",
        ErrorKind::Security,
        "模拟系统凭据库不可用",
        true,
    )
}

#[async_trait::async_trait]
impl CredentialStore for MemoryCredentialStore {
    async fn set(&self, target: &CredentialRef, secret: &SecretString) -> Result<(), AppError> {
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err(credential_unavailable());
        }
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(target.as_str().to_owned(), secret.expose().to_owned());
        self.writes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(target.as_str().to_owned());

        // 外部写入期间的真实并发用例（来源被删除/被改写/凭据被清除）。
        let during = self.take_during_io();
        self.run_during_io(during).await;

        // 外部写入中途挂起：另一个写入者必须在这段时间里拿到 BUSY。
        let hold = self
            .hold
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(gate) = hold {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
        Ok(())
    }

    async fn get(&self, target: &CredentialRef) -> Result<Option<SecretString>, AppError> {
        Ok(self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(target.as_str())
            .map(|value| SecretString::new(value.clone())))
    }

    async fn delete(&self, target: &CredentialRef) -> Result<bool, AppError> {
        let selectively_failing = self
            .fail_delete_targets
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(target.as_str());
        if self.fail_deletes.load(Ordering::SeqCst) || selectively_failing {
            return Err(credential_unavailable());
        }
        let deleted = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(target.as_str())
            .is_some();
        self.deletes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(target.as_str().to_owned());

        // 删除期间的真实并发用例：清理扫描正是要在这里证明「并发新增的条目不会被丢掉」。
        let during = self.take_during_io();
        self.run_during_io(during).await;
        Ok(deleted)
    }
}

impl MemoryCredentialStore {
    fn take_during_io(&self) -> Option<DuringWrite> {
        self.during_io
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }

    /// 跑一段真实的并发用例（`set` 与 `delete` 共用同一条钩子）。
    async fn run_during_io(&self, during: Option<DuringWrite>) {
        let Some(during) = during else {
            return;
        };
        match during {
            DuringWrite::UpdateEndpoint {
                registry,
                source_id,
                endpoint,
            } => {
                registry
                    .update_custom_source(haven_application::wire::SourceUpdateRequest {
                        source_id,
                        display_name: None,
                        endpoint: Some(endpoint),
                    })
                    .await
                    .expect("外部 IO 期间改地址必须成功");
            }
            DuringWrite::RemoveSource {
                registry,
                source_id,
                cache,
            } => {
                // 合规的移除必须在租约上与本次外部写入互斥：这里应当拿到 BUSY。
                let outcome = registry
                    .remove_custom_source(&source_id, self, cache.as_ref())
                    .await
                    .map(|_| ());
                self.note_outcome(outcome);
            }
            DuringWrite::ClearCredential {
                registry,
                source_id,
            } => {
                let outcome = registry
                    .set_custom_source_credential(
                        &haven_application::wire::SourceSetCredentialRequest {
                            username: None,
                            source_id,
                            secret: None,
                        },
                        self,
                    )
                    .await;
                self.note_outcome(outcome);
            }
            DuringWrite::ForgeSourceRemoval { repos, source_id } => {
                forge_stored_payload(&repos, |payload| {
                    strip_stored_source(payload, &source_id);
                })
                .await;
            }
            DuringWrite::ForgeRevokedCredential { repos, source_id } => {
                forge_stored_payload(&repos, |payload| {
                    revoke_stored_credential(payload, &source_id);
                })
                .await;
            }
        }
    }
}

/// 非合规写者：直接改写落库的 sources 负载（不经过注册表，也不持租约）。
pub async fn forge_stored_payload(
    repos: &SqliteRepositories,
    mutate: impl FnOnce(&mut serde_json::Value),
) {
    let mut payload = stored_sources(repos).await;
    mutate(&mut payload);
    overwrite_stored_sources(repos, &payload).await;
}

/// 从落库负载里移除一个来源的全部痕迹（记录、启用位、端点、版本 token）。
pub fn strip_stored_source(payload: &mut serde_json::Value, source_id: &str) {
    if let Some(sources) = payload["customSources"].as_array_mut() {
        sources.retain(|record| record["sourceId"] != source_id);
    }
    if let Some(enabled) = payload["enabledSources"].as_array_mut() {
        enabled.retain(|id| id != source_id);
    }
    if let Some(endpoints) = payload["endpoints"].as_object_mut() {
        endpoints.remove(source_id);
    }
    if let Some(revisions) = payload["sourceRevisions"].as_object_mut() {
        revisions.remove(source_id);
    }
}

/// 从落库负载里撤销某来源的凭据引用，并把该引用登记进清理队列。
pub fn revoke_stored_credential(payload: &mut serde_json::Value, source_id: &str) {
    let Some(sources) = payload["customSources"].as_array_mut() else {
        return;
    };
    let Some(record) = sources
        .iter_mut()
        .find(|record| record["sourceId"] == source_id)
    else {
        return;
    };
    let Some(reference) = record["credentialRef"].as_str().map(str::to_owned) else {
        return;
    };
    record["credentialRef"] = serde_json::Value::Null;
    let cleanup = payload["credentialCleanup"]
        .as_array_mut()
        .expect("清理队列必须是数组");
    cleanup.push(serde_json::json!({
        "reference": reference,
        "sourceId": source_id,
    }));
}

/// 删除默认失败、写入照常的来源配置缓存（用于「清理义务必须可重试」）。
pub struct FailingDeleteCache {
    inner: InMemorySourceConfigCache,
    fail_deletes: AtomicBool,
}

impl FailingDeleteCache {
    pub fn new() -> Self {
        Self {
            inner: InMemorySourceConfigCache::new(),
            fail_deletes: AtomicBool::new(true),
        }
    }

    /// 缓存端口恢复可用（模拟用户在端口恢复之后重试）。
    pub fn allow_deletes(&self) {
        self.fail_deletes.store(false, Ordering::SeqCst);
    }

    pub fn inner(&self) -> &InMemorySourceConfigCache {
        &self.inner
    }
}

impl Default for FailingDeleteCache {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl haven_application::services::source_config_cache::SourceConfigCache for FailingDeleteCache {
    async fn put(
        &self,
        source_id: &str,
        body: &[u8],
        fetched_at: UtcMillis,
    ) -> Result<(), AppError> {
        self.inner.put(source_id, body, fetched_at).await
    }

    async fn get(
        &self,
        source_id: &str,
    ) -> Result<Option<haven_application::services::CachedSourceConfig>, AppError> {
        self.inner.get(source_id).await
    }

    async fn delete(&self, source_id: &str) -> Result<bool, AppError> {
        if self.fail_deletes.load(Ordering::SeqCst) {
            return Err(AppError::new(
                "DATABASE_ERROR",
                ErrorKind::Database,
                "模拟缓存删除失败",
                true,
            ));
        }
        self.inner.delete(source_id).await
    }
}

// ---------- 公开装配面上的小工具 ----------

/// 包住真实 `SqliteSettingsUoW` 的故障注入：指定的第 N 次**写事务**返回数据库错误，其余照常。
///
/// 数据库错误与外部存储错误是两类不同的失败，凭据生命周期对它们的处置也不同：
/// 暂存之前失败必须一个字节都不写外部存储；采纳失败则要靠补偿与暂存记录把义务留住；
/// 删除成功但出队事务失败则必须让义务留在队列里，由下一次扫描幂等地补齐。
///
/// 可以一次指定多个失败序号（例如「采纳失败 + 补偿登记也失败」这种连续故障），
/// 用来钉住「登记义务失败时原来的暂存记录必须原样保留」。
pub struct FlakySettingsUoW {
    inner: SqliteSettingsUoW,
    /// 要失败的写事务序号（从 0 开始）；空集合 = 全部通过。
    fail_runs: Mutex<HashSet<usize>>,
    runs: AtomicUsize,
}

impl FlakySettingsUoW {
    pub fn new(db: Arc<Db>) -> Self {
        Self {
            inner: SqliteSettingsUoW::new(db),
            fail_runs: Mutex::new(HashSet::new()),
            runs: AtomicUsize::new(0),
        }
    }

    /// 让第 `index` 次写事务失败，并从这一次开始重新计数。
    pub fn fail_run_at(&self, index: usize) {
        self.fail_runs_at(&[index]);
    }

    /// 让这几个序号的写事务都失败，并从第 0 次开始重新计数。
    pub fn fail_runs_at(&self, indices: &[usize]) {
        self.runs.store(0, Ordering::SeqCst);
        let mut plan = self
            .fail_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        plan.clear();
        plan.extend(indices.iter().copied());
    }

    /// 数据库恢复可用。
    pub fn heal(&self) {
        self.fail_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }
}

impl SettingsUoW for FlakySettingsUoW {
    fn run(
        &self,
        f: &dyn Fn(&dyn SettingsTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        let index = self.runs.fetch_add(1, Ordering::SeqCst);
        let should_fail = self
            .fail_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&index);
        if should_fail {
            return Err(AppError::new(
                "DATABASE_ERROR",
                ErrorKind::Database,
                "模拟设置事务失败",
                true,
            ));
        }
        self.inner.run(f)
    }

    fn run_read(
        &self,
        f: &dyn Fn(&dyn SettingsTxPorts) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        self.inner.run_read(f)
    }
}

/// 登记一个自定义 OPDS 书源并返回它的 sourceId。
pub async fn add_source(registry: &SourceRegistryService, name: &str, endpoint: &str) -> String {
    registry
        .add_custom_source(name, endpoint)
        .await
        .expect("登记自定义来源必须成功")
        .source_id
}

/// `source_set_credential` 请求（`Some` = 写入，`None` = 清除）。
pub fn credential_request(source_id: &str, secret: Option<&str>) -> SourceSetCredentialRequest {
    SourceSetCredentialRequest {
        username: None,
        source_id: source_id.to_owned(),
        secret: secret.map(str::to_owned),
    }
}

/// 写入一条凭据，返回真实结果（错误由调用方断言）。
pub async fn write_credential(
    registry: &SourceRegistryService,
    store: &MemoryCredentialStore,
    source_id: &str,
    secret: &str,
) -> Result<(), AppError> {
    registry
        .set_custom_source_credential(&credential_request(source_id, Some(secret)), store)
        .await
}

/// 清除凭据，返回真实结果。
pub async fn clear_credential(
    registry: &SourceRegistryService,
    store: &MemoryCredentialStore,
    source_id: &str,
) -> Result<(), AppError> {
    registry
        .set_custom_source_credential(&credential_request(source_id, None), store)
        .await
}

/// 当前持久化的凭据引用（`None` = 未配置）。
pub async fn credential_ref(registry: &SourceRegistryService, source_id: &str) -> Option<String> {
    registry
        .custom_credential_ref(source_id)
        .await
        .expect("已登记来源的凭据引用必须可读")
        .map(|reference| reference.as_str().to_owned())
}

/// 注册表里某个来源的描述符。
pub async fn descriptor_of(
    registry: &SourceRegistryService,
    source_id: &str,
) -> haven_application::wire::SourceDescriptorDto {
    registry
        .list()
        .await
        .unwrap()
        .sources
        .into_iter()
        .find(|source| source.source_id == source_id)
        .unwrap_or_else(|| panic!("来源 {source_id} 必须出现在注册表里"))
}

/// 某个来源当前的配置版本（健康观测新鲜度的依据）。
pub async fn current_revision(registry: &SourceRegistryService, source_id: &str) -> Option<String> {
    registry
        .enabled_sources_snapshot()
        .await
        .unwrap()
        .revisions
        .get(source_id)
        .cloned()
}

/// 注册表里是否还有这个来源。
pub async fn source_exists(registry: &SourceRegistryService, source_id: &str) -> bool {
    registry
        .list()
        .await
        .unwrap()
        .sources
        .iter()
        .any(|source| source.source_id == source_id)
}
