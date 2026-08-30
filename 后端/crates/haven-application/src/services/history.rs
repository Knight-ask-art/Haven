//! HistoryService：`history` 记录 / 列表 / 清除（BE-HISTORY-001）。
//!
//! 规则（契约 §23）：
//! - 历史 ≠ 进度：记录"何时打开过"，不承载消费位置。
//! - `history_clear` 只清历史，不得清除 Progress / Favorite / Marker。
//! - record 幂等：同一 media_item 只保留一条（started_at 保留首次，last_active_at 刷新）。
//! - MediaItem 不存在 → `MEDIA_ITEM_NOT_FOUND`（友好错误码，链校验兜底在 Repository）。

use std::sync::Arc;

use haven_common::AppError;
use haven_domain::contracts::{EditionRepository, HistoryRepository, MediaItemRepository};
use haven_domain::entities::HistoryEntry;
use haven_domain::ids::{HistoryEntryId, MediaItemId};
use haven_domain::settings::{SettingsSection, SettingsValue};

use crate::mapper::time::utc_millis_to_rfc3339;
use crate::services::settings::SettingsService;
use crate::wire::HistoryEntryDto;

/// HistoryService 所需端口。
/// 访问方法由 blanket impl 提供（具体类型 → 子契约 coercion），
/// 避免 dyn→dyn trait upcasting（MSRV 1.85 不支持，E0658）。
pub trait HistoryPorts:
    MediaItemRepository + EditionRepository + HistoryRepository + Send + Sync
{
    fn as_history(&self) -> &dyn HistoryRepository;
    fn as_media_item(&self) -> &dyn MediaItemRepository;
    fn as_edition(&self) -> &dyn EditionRepository;
}
impl<T> HistoryPorts for T
where
    T: MediaItemRepository + EditionRepository + HistoryRepository + Send + Sync,
{
    fn as_history(&self) -> &dyn HistoryRepository {
        self
    }
    fn as_media_item(&self) -> &dyn MediaItemRepository {
        self
    }
    fn as_edition(&self) -> &dyn EditionRepository {
        self
    }
}

#[derive(Clone)]
pub struct HistoryService {
    ports: Arc<dyn HistoryPorts>,
    settings: Arc<SettingsService>,
}

impl HistoryService {
    pub fn new(ports: Arc<dyn HistoryPorts>, settings: Arc<SettingsService>) -> Self {
        Self { ports, settings }
    }

    /// 记录一次打开（Session 打开内容时调用）。
    pub async fn record(&self, media_item_id: MediaItemId) -> Result<(), AppError> {
        if !self.playback_history_enabled().await {
            // 隐私设置只控制新消费记录的写入；已有 history 仍可由查询/清理命令读取或删除。
            return Ok(());
        }
        self.ensure_media_item(media_item_id).await?;
        let now = haven_common::UtcMillis::now();

        let existing = self
            .ports
            .as_history()
            .list_for_media_item(media_item_id)
            .await?
            .into_iter()
            .next();

        let entry = match existing {
            Some(mut entry) => {
                // 刷新活跃时间；started_at 保留首次记录。
                entry.last_active_at = now;
                entry
            }
            None => {
                // workId/editionId 由 MediaItem 推导（与 Progress 同规则）。
                let item = self
                    .ports
                    .as_media_item()
                    .get(media_item_id)
                    .await?
                    .ok_or_else(media_item_not_found)?;
                let edition = self
                    .ports
                    .as_edition()
                    .get(item.edition_id)
                    .await?
                    .ok_or_else(edition_not_found)?;
                HistoryEntry {
                    id: HistoryEntryId::new(),
                    media_item_id,
                    work_id: edition.work_id,
                    edition_id: edition.id,
                    locator: None,
                    started_at: now,
                    last_active_at: now,
                    completed_at: None,
                }
            }
        };
        self.ports.as_history().save(&entry).await
    }

    async fn playback_history_enabled(&self) -> bool {
        let Ok(snapshot) = self.settings.get(SettingsSection::Privacy).await else {
            // 设置读取失败时保持升级前行为，避免静默丢失用户的历史记录；下一次
            // Session 打开仍会重试读取设置。
            return true;
        };
        match snapshot.value {
            SettingsValue::Privacy(value) => value.playback_history,
            _ => true,
        }
    }

    /// 标记完成（Session 结束时调用）。
    pub async fn complete(&self, media_item_id: MediaItemId) -> Result<(), AppError> {
        let entry = self
            .ports
            .as_history()
            .list_for_media_item(media_item_id)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                AppError::new(
                    "HISTORY_NOT_FOUND",
                    haven_common::ErrorKind::NotFound,
                    "历史记录不存在",
                    false,
                )
            })?;
        let mut entry = entry;
        entry.completed_at = Some(haven_common::UtcMillis::now());
        self.ports.as_history().save(&entry).await
    }

    pub async fn list_for_media_item(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Vec<HistoryEntryDto>, AppError> {
        let entries = self
            .ports
            .as_history()
            .list_for_media_item(media_item_id)
            .await?;
        Ok(entries.iter().map(to_dto).collect())
    }

    pub async fn recent(&self, limit: u32) -> Result<Vec<HistoryEntryDto>, AppError> {
        let entries = self
            .ports
            .as_history()
            .recent(limit.min(crate::services::library::MAX_LIMIT))
            .await?;
        Ok(entries.iter().map(to_dto).collect())
    }

    /// `history_clear`：只清历史，不动 Progress / Favorite / Marker（契约 §23.2）。
    /// 单条 DELETE 语句原子执行，无条目数上限（审查修复：原实现只删 10k 条）。
    pub async fn clear(&self) -> Result<(), AppError> {
        self.ports.as_history().clear_all().await
    }

    async fn ensure_media_item(&self, media_item_id: MediaItemId) -> Result<(), AppError> {
        match self.ports.as_media_item().get(media_item_id).await? {
            Some(_) => Ok(()),
            None => Err(media_item_not_found()),
        }
    }
}

fn to_dto(entry: &HistoryEntry) -> HistoryEntryDto {
    HistoryEntryDto {
        history_entry_id: entry.id.to_string(),
        media_item_id: entry.media_item_id.to_string(),
        work_id: entry.work_id.to_string(),
        edition_id: entry.edition_id.to_string(),
        started_at: utc_millis_to_rfc3339(entry.started_at),
        last_active_at: utc_millis_to_rfc3339(entry.last_active_at),
        completed_at: entry.completed_at.map(utc_millis_to_rfc3339),
    }
}

fn media_item_not_found() -> AppError {
    AppError::new(
        "MEDIA_ITEM_NOT_FOUND",
        haven_common::ErrorKind::NotFound,
        "媒体条目不存在",
        false,
    )
}

fn edition_not_found() -> AppError {
    AppError::new(
        "EDITION_NOT_FOUND",
        haven_common::ErrorKind::NotFound,
        "版本不存在",
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::settings::{SettingsTxPorts, SettingsUoW};
    use haven_domain::contracts::SettingsRow;
    use haven_domain::contracts::{EditionRepository, HistoryRepository, MediaItemRepository};
    use haven_domain::entities::{Edition, HistoryEntry, MediaIndex, MediaItem};
    use haven_domain::enums::{MediaItemStatus, MediaType};
    use haven_domain::ids::{EditionId, WorkId};

    struct StaticSettingsUow {
        row: Option<SettingsRow>,
    }

    struct StaticSettingsTx<'a> {
        row: &'a Option<SettingsRow>,
    }

    impl SettingsTxPorts for StaticSettingsTx<'_> {
        fn load(&self, section: &str) -> Result<Option<SettingsRow>, AppError> {
            Ok(self
                .row
                .as_ref()
                .filter(|row| row.section == section)
                .cloned())
        }

        fn cas_write(
            &self,
            _section: &str,
            _expected_revision: Option<&str>,
            _row: &SettingsRow,
        ) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    impl SettingsUoW for StaticSettingsUow {
        fn run(
            &self,
            f: &dyn Fn(&dyn SettingsTxPorts) -> Result<(), AppError>,
        ) -> Result<(), AppError> {
            let tx = StaticSettingsTx { row: &self.row };
            f(&tx)
        }

        fn run_read(
            &self,
            f: &dyn Fn(&dyn SettingsTxPorts) -> Result<(), AppError>,
        ) -> Result<(), AppError> {
            let tx = StaticSettingsTx { row: &self.row };
            f(&tx)
        }
    }

    fn settings_service(playback_history: bool) -> Arc<SettingsService> {
        let value = SettingsValue::Privacy(haven_domain::settings::PrivacySettings {
            search_history: true,
            playback_history,
        });
        Arc::new(SettingsService::new(Arc::new(StaticSettingsUow {
            row: Some(SettingsRow {
                section: "privacy".into(),
                schema_version: 1,
                revision: "test-revision".into(),
                data_json: serde_json::to_string(&value).unwrap(),
                updated_at: haven_common::UtcMillis(1),
            }),
        })))
    }

    struct MemPorts {
        items: Vec<MediaItem>,
        editions: Vec<Edition>,
        entries: std::sync::Mutex<Vec<HistoryEntry>>,
    }

    fn mem_ports() -> (MemPorts, MediaItemId) {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let ports = MemPorts {
            items: vec![MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Movie,
                title: "电影".into(),
                index: MediaIndex::Movie,
                duration_ms: None,
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
            }],
            editions: vec![Edition {
                id: edition_id,
                work_id,
                title: "版本".into(),
                subtitle: None,
                edition_type: MediaType::Movie,
                release_date: None,
                language: None,
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: Default::default(),
                created_at: haven_common::UtcMillis(1),
                updated_at: haven_common::UtcMillis(1),
            }],
            entries: std::sync::Mutex::new(vec![]),
        };
        (ports, media_item_id)
    }

    #[async_trait::async_trait]
    impl MediaItemRepository for MemPorts {
        async fn get(&self, id: MediaItemId) -> Result<Option<MediaItem>, AppError> {
            Ok(self.items.iter().find(|i| i.id == id).cloned())
        }
        async fn save(&self, _m: &MediaItem) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_by_edition(&self, _e: EditionId) -> Result<Vec<MediaItem>, AppError> {
            Ok(vec![])
        }
        async fn delete(&self, _id: MediaItemId) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    #[async_trait::async_trait]
    impl EditionRepository for MemPorts {
        async fn get(&self, id: EditionId) -> Result<Option<Edition>, AppError> {
            Ok(self.editions.iter().find(|e| e.id == id).cloned())
        }
        async fn save(&self, _e: &Edition) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_by_work(&self, _w: WorkId) -> Result<Vec<Edition>, AppError> {
            Ok(vec![])
        }
        async fn delete(&self, _id: EditionId) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    #[async_trait::async_trait]
    impl HistoryRepository for MemPorts {
        async fn get(&self, id: HistoryEntryId) -> Result<Option<HistoryEntry>, AppError> {
            Ok(self
                .entries
                .lock()
                .unwrap()
                .iter()
                .find(|e| e.id == id)
                .cloned())
        }
        async fn save(&self, entry: &HistoryEntry) -> Result<(), AppError> {
            let mut entries = self.entries.lock().unwrap();
            if let Some(existing) = entries.iter_mut().find(|e| e.id == entry.id) {
                *existing = entry.clone();
            } else {
                entries.push(entry.clone());
            }
            Ok(())
        }
        async fn list_for_media_item(
            &self,
            media_item_id: MediaItemId,
        ) -> Result<Vec<HistoryEntry>, AppError> {
            Ok(self
                .entries
                .lock()
                .unwrap()
                .iter()
                .filter(|e| e.media_item_id == media_item_id)
                .cloned()
                .collect())
        }
        async fn recent(&self, _limit: u32) -> Result<Vec<HistoryEntry>, AppError> {
            Ok(self.entries.lock().unwrap().clone())
        }
        async fn clear_all(&self) -> Result<(), AppError> {
            self.entries.lock().unwrap().clear();
            Ok(())
        }
        async fn delete(&self, id: HistoryEntryId) -> Result<bool, AppError> {
            let mut entries = self.entries.lock().unwrap();
            let before = entries.len();
            entries.retain(|e| e.id != id);
            Ok(entries.len() < before)
        }
    }

    #[tokio::test]
    async fn record_creates_then_refreshes_same_entry() {
        let (ports, media_item_id) = mem_ports();
        let service = HistoryService::new(Arc::new(ports), settings_service(true));
        service.record(media_item_id).await.unwrap();
        service.record(media_item_id).await.unwrap();

        let entries = service.list_for_media_item(media_item_id).await.unwrap();
        assert_eq!(entries.len(), 1, "同一 media_item 只保留一条历史");
        assert_eq!(entries[0].media_item_id, media_item_id.to_string());
    }

    #[tokio::test]
    async fn record_missing_media_item_errors() {
        let (ports, _) = mem_ports();
        let service = HistoryService::new(Arc::new(ports), settings_service(true));
        let err = service.record(MediaItemId::new()).await.unwrap_err();
        assert_eq!(err.code().as_str(), "MEDIA_ITEM_NOT_FOUND");
    }

    #[tokio::test]
    async fn complete_marks_completed_at() {
        let (ports, media_item_id) = mem_ports();
        let service = HistoryService::new(Arc::new(ports), settings_service(true));
        service.record(media_item_id).await.unwrap();
        service.complete(media_item_id).await.unwrap();
        let entries = service.list_for_media_item(media_item_id).await.unwrap();
        assert!(entries[0].completed_at.is_some());
    }

    #[tokio::test]
    async fn clear_removes_history_only() {
        let (ports, media_item_id) = mem_ports();
        let service = HistoryService::new(Arc::new(ports), settings_service(true));
        service.record(media_item_id).await.unwrap();
        service.clear().await.unwrap();
        assert!(
            service
                .list_for_media_item(media_item_id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn disabled_playback_history_skips_new_records_without_validating_media() {
        let (ports, media_item_id) = mem_ports();
        let service = HistoryService::new(Arc::new(ports), settings_service(false));

        // 关闭后不写入，也不因媒体条目不存在而制造无意义的错误。
        service.record(MediaItemId::new()).await.unwrap();
        assert!(
            service
                .list_for_media_item(media_item_id)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
