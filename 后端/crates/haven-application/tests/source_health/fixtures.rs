//! 共享夹具：真实 SQLite + 真实 `SettingsUoW` 的来源注册表、观测构造与落库读取。
//!
//! 健康段的 section 命名与有界窗口长度在这里按**外部可观察契约**重新声明，而不是从
//! `source_registry::health` 里导出：单元测试曾经 `use super::health::{HEALTH_WINDOW,
//! health_section}`，但那些是 `pub(super)` 的内部实现细节，而集成测试只应该看见公开
//! 装配面。为了测试方便把它们提升为公开导出会把内部结构钉进 API，所以这里改为自己
//! 声明同一份契约值——断言的是「一个来源一行、窗口有界、越界即丢弃」这个可观察事实，
//! 一旦实现漂移（例如窗口长度改变），窗口长度断言会立刻失败。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use haven_application::services::source_registry::{
    InMemorySourceCredentialLease, SourceRegistryService, SourceSearchObservation,
};
use haven_application::wire::SourceDescriptorDto;
use haven_common::{AppError, UtcMillis};
use haven_domain::contracts::{SettingsRepository, SettingsRow};
use haven_domain::credential::{CredentialStore, SecretString};
use haven_domain::ids::CredentialRef;
use haven_infrastructure::Db;
use haven_infrastructure::db::repos::{SqliteRepositories, SqliteSettingsUoW};

/// 落库健康段的 section 前缀（与 `source_registry::health` 的 `source_health:` 对齐）。
pub const HEALTH_SECTION_PREFIX: &str = "source_health:";
/// 有界观测窗口长度（与 `source_registry::health::HEALTH_WINDOW` 对齐）。
pub const HEALTH_WINDOW: usize = 20;
/// 测试观测的固定延迟：让 latency 断言不依赖真实耗时。
pub const LATENCY_MS: u64 = 42;

/// 某个来源的健康段 section 名（一个来源一行，互不覆盖）。
pub fn health_section(source_id: &str) -> String {
    format!("{HEALTH_SECTION_PREFIX}{source_id}")
}

/// 真实 SQLite（内存）+ 真实设置事务的来源注册表。
///
/// 配置变更与健康写入都走 `SqliteSettingsUoW`：这些用例断言的是真实的落库行为与
/// 真实事务的原子性，不用替身假装事务。外部凭据 I/O 的独占租约在这里**显式注入**
/// （内存库不与任何其它进程共享事实，进程内租约就是它正确的独占范围）；没有它，
/// 凭据写入/清除、来源移除与清理扫描会 fail closed。
pub fn registry_with_health() -> (SourceRegistryService, Arc<SqliteRepositories>) {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let registry = SourceRegistryService::new(repos.clone())
        .with_settings_uow(Arc::new(SqliteSettingsUoW::new(db.clone())))
        .with_credential_lease(Arc::new(InMemorySourceCredentialLease::new()));
    (registry, repos)
}

/// 一次真实搜索观测。开始时戳显式给出：乱序完成因此不依赖真实时钟或线程调度。
pub fn obs(
    source_id: &str,
    source_revision: Option<String>,
    started_at_ms: i64,
    succeeded: bool,
) -> SourceSearchObservation {
    SourceSearchObservation {
        source_id: source_id.to_owned(),
        source_revision,
        started_at: UtcMillis(started_at_ms),
        succeeded,
        latency_ms: LATENCY_MS,
    }
}

/// 分发时会读到的配置版本（与真实分发同源）。
pub async fn current_revision(registry: &SourceRegistryService, source_id: &str) -> Option<String> {
    registry
        .enabled_sources_snapshot()
        .await
        .unwrap()
        .revisions
        .get(source_id)
        .cloned()
}

/// 注册表里某个来源的描述符（含健康投影）。
pub async fn descriptor_of(
    registry: &SourceRegistryService,
    source_id: &str,
) -> SourceDescriptorDto {
    registry
        .list()
        .await
        .unwrap()
        .sources
        .into_iter()
        .find(|source| source.source_id == source_id)
        .unwrap_or_else(|| panic!("来源 {source_id} 必须出现在注册表里"))
}

/// 该来源已落库的健康行（没有观测过 → None）。
pub async fn stored_health_row(repos: &SqliteRepositories, source_id: &str) -> Option<SettingsRow> {
    SettingsRepository::get(repos, &health_section(source_id))
        .await
        .unwrap()
}

/// 落库记录的原始 JSON（窗口长度、版本、时间戳都只在这里可见）。
pub async fn stored_health_json(repos: &SqliteRepositories, source_id: &str) -> serde_json::Value {
    let row = stored_health_row(repos, source_id)
        .await
        .expect("该来源必须有已落库的健康行");
    serde_json::from_str(&row.data_json).expect("健康行必须是合法 JSON")
}

/// 删除路径需要一个凭据端口；这些用例关心的是来源记录与健康事实。
pub struct NoopCredentialStore;

#[async_trait::async_trait]
impl CredentialStore for NoopCredentialStore {
    async fn set(&self, _target: &CredentialRef, _secret: &SecretString) -> Result<(), AppError> {
        Ok(())
    }

    async fn get(&self, _target: &CredentialRef) -> Result<Option<SecretString>, AppError> {
        Ok(None)
    }

    async fn delete(&self, _target: &CredentialRef) -> Result<bool, AppError> {
        Ok(false)
    }
}

/// 凭据端口的内存替身（写入初始秘密、断言补偿还原）。
pub struct MemoryStore {
    entries: Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl CredentialStore for MemoryStore {
    async fn set(&self, target: &CredentialRef, secret: &SecretString) -> Result<(), AppError> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(target.as_str().to_owned(), secret.expose().to_owned());
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
        Ok(self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(target.as_str())
            .is_some())
    }
}

/// keystore IO 之后是否需要复核：在 `set` / `delete` 期间改写来源端点。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditOn {
    Set,
    Delete,
}

/// 在 keystore IO 期间改写来源端点的凭据端口：模拟「保存凭据的过程中来源被改配置」。
///
/// 真实 keystore 无法在测试里插进这段窗口，而它正是「IO 之后复核 + 只删除自己的槽位」
/// 要覆盖的地方。写入与删除的 target 都被记录下来，供断言「只动自己的槽位」。
pub struct EditingCredentialStore {
    registry: SourceRegistryService,
    source_id: String,
    endpoint: String,
    edit_on: EditOn,
    entries: Mutex<HashMap<String, String>>,
    writes: Mutex<Vec<String>>,
    deletes: Mutex<Vec<String>>,
}

impl EditingCredentialStore {
    pub fn new(
        registry: SourceRegistryService,
        source_id: &str,
        endpoint: &str,
        edit_on: EditOn,
    ) -> Self {
        Self {
            registry,
            source_id: source_id.to_owned(),
            endpoint: endpoint.to_owned(),
            edit_on,
            entries: Mutex::new(HashMap::new()),
            writes: Mutex::new(Vec::new()),
            deletes: Mutex::new(Vec::new()),
        }
    }

    /// 预置一条「IO 之前就已经在 keystore 里」的秘密。
    pub fn seed_secret(&self, target: &str, secret: &str) {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(target.to_owned(), secret.to_owned());
    }

    /// 真实写入过的 target（按发生顺序）。
    pub fn writes(&self) -> Vec<String> {
        self.writes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// 真实调用过删除的 target（按发生顺序）。
    pub fn deletes(&self) -> Vec<String> {
        self.deletes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// IO 期间的并发配置变更（改端点 → 该来源的配置版本推进）。
    ///
    /// 自定义源改地址走 `update_custom_source`：`set_endpoint` 只接受内置目录里的 sourceId。
    async fn edit_source_config(&self) {
        self.registry
            .update_custom_source(haven_application::wire::SourceUpdateRequest {
                source_id: self.source_id.clone(),
                display_name: None,
                endpoint: Some(self.endpoint.clone()),
            })
            .await
            .expect("IO 期间改地址必须成功");
    }
}

#[async_trait::async_trait]
impl CredentialStore for EditingCredentialStore {
    async fn set(&self, target: &CredentialRef, secret: &SecretString) -> Result<(), AppError> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(target.as_str().to_owned(), secret.expose().to_owned());
        self.writes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(target.as_str().to_owned());
        if self.edit_on == EditOn::Set {
            self.edit_source_config().await;
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
        let removed = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(target.as_str())
            .is_some();
        self.deletes
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(target.as_str().to_owned());
        if self.edit_on == EditOn::Delete {
            self.edit_source_config().await;
        }
        Ok(removed)
    }
}
