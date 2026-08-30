//! 资源内设 Application Service（ADR-RESOURCE-PREF-001）。
//!
//! 负责真实媒体归属校验、global/edition/media-item 三层 effective 合并以及
//! 作用域 CAS 编排。持久化、SQL 与文件系统仍由 Domain Port/Infrastructure 提供。

use std::sync::Arc;

use haven_common::{AppError, UtcMillis};
use haven_domain::contracts::{
    EditionPreference, EditionRepository, MediaItemPreference, MediaItemRepository,
    ResourcePreferenceRepository,
};
use haven_domain::ids::{EditionId, MediaItemId};
use haven_domain::settings::{
    ComicPatch, ComicSettings, PreferenceData, ReadingPatch, ReadingSettings, SettingsPatch,
    SettingsSection, SettingsValue,
};

use super::settings::SettingsService;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferenceTarget {
    Edition,
    MediaItem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreferenceSnapshot {
    pub media_item_id: MediaItemId,
    pub edition_id: EditionId,
    /// 最具体覆盖（保留用于旧客户端兼容）。
    pub reading_patch: Option<ReadingPatch>,
    pub comic_patch: Option<ComicPatch>,
    /// 各作用域的原始覆盖，供设置页独立编辑 Edition 与 MediaItem。
    pub edition_reading_patch: Option<ReadingPatch>,
    pub edition_comic_patch: Option<ComicPatch>,
    pub media_item_reading_patch: Option<ReadingPatch>,
    pub media_item_comic_patch: Option<ComicPatch>,
    pub effective_reading: ReadingSettings,
    pub effective_comic: ComicSettings,
    pub media_item_revision: Option<String>,
    pub edition_revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreferenceUpdateResult {
    pub snapshot: PreferenceSnapshot,
    pub target: PreferenceTarget,
    pub revision: Option<String>,
    pub changed: bool,
}

#[derive(Clone)]
pub struct ResourcePreferenceService {
    preferences: Arc<dyn ResourcePreferenceRepository>,
    media_items: Arc<dyn MediaItemRepository>,
    editions: Arc<dyn EditionRepository>,
    settings: SettingsService,
}

impl ResourcePreferenceService {
    pub fn new(
        preferences: Arc<dyn ResourcePreferenceRepository>,
        media_items: Arc<dyn MediaItemRepository>,
        editions: Arc<dyn EditionRepository>,
        settings: SettingsService,
    ) -> Self {
        Self {
            preferences,
            media_items,
            editions,
            settings,
        }
    }

    pub async fn get(
        &self,
        media_item_id: MediaItemId,
        edition_id: EditionId,
    ) -> Result<PreferenceSnapshot, AppError> {
        self.validate_media_edition(media_item_id, edition_id)
            .await?;
        self.build_snapshot(media_item_id, edition_id).await
    }

    pub async fn update(
        &self,
        media_item_id: MediaItemId,
        edition_id: EditionId,
        target: PreferenceTarget,
        data: PreferenceData,
        expected_revision: Option<&str>,
    ) -> Result<PreferenceUpdateResult, AppError> {
        self.validate_media_edition(media_item_id, edition_id)
            .await?;

        let (changed, revision) = match target {
            PreferenceTarget::Edition => {
                let current = self.preferences.get_edition(edition_id).await?;
                let current_revision = current.as_ref().map(|value| value.revision.as_str());
                if !revision_matches(current_revision, expected_revision) {
                    return Err(conflict());
                }
                let current_data = current
                    .as_ref()
                    .map(|value| value.data.clone())
                    .unwrap_or_default();
                if current_data == data {
                    (false, current.map(|value| value.revision))
                } else {
                    let revision = new_revision("edition");
                    let row = EditionPreference {
                        edition_id,
                        data,
                        revision: revision.clone(),
                        updated_at: UtcMillis::now(),
                    };
                    if !self
                        .preferences
                        .cas_upsert_edition(&row, expected_revision)
                        .await?
                    {
                        return Err(conflict());
                    }
                    (true, Some(revision))
                }
            }
            PreferenceTarget::MediaItem => {
                let current = self.preferences.get_media_item(media_item_id).await?;
                let current_revision = current.as_ref().map(|value| value.revision.as_str());
                if !revision_matches(current_revision, expected_revision) {
                    return Err(conflict());
                }
                let current_data = current
                    .as_ref()
                    .map(|value| value.data.clone())
                    .unwrap_or_default();
                if current_data == data {
                    (false, current.map(|value| value.revision))
                } else {
                    let revision = new_revision("media");
                    let row = MediaItemPreference {
                        media_item_id,
                        edition_id,
                        data,
                        revision: revision.clone(),
                        updated_at: UtcMillis::now(),
                    };
                    if !self
                        .preferences
                        .cas_upsert_media_item(&row, expected_revision)
                        .await?
                    {
                        return Err(conflict());
                    }
                    (true, Some(revision))
                }
            }
        };

        Ok(PreferenceUpdateResult {
            snapshot: self.build_snapshot(media_item_id, edition_id).await?,
            target,
            revision,
            changed,
        })
    }

    async fn validate_media_edition(
        &self,
        media_item_id: MediaItemId,
        edition_id: EditionId,
    ) -> Result<(), AppError> {
        let media = self
            .media_items
            .get(media_item_id)
            .await?
            .ok_or_else(|| not_found("MEDIA_ITEM_NOT_FOUND", "媒体条目不存在"))?;
        if media.edition_id != edition_id {
            return Err(AppError::new(
                "INVALID_ARGUMENT",
                haven_common::ErrorKind::Validation,
                "媒体条目与版本不匹配",
                false,
            ));
        }
        if self.editions.get(edition_id).await?.is_none() {
            return Err(not_found("EDITION_NOT_FOUND", "版本不存在"));
        }
        Ok(())
    }

    async fn build_snapshot(
        &self,
        media_item_id: MediaItemId,
        edition_id: EditionId,
    ) -> Result<PreferenceSnapshot, AppError> {
        let edition_pref = self.load_edition_preference(edition_id).await?;
        let media_pref = self.load_media_item_preference(media_item_id).await?;
        let global_reading = match self.settings.get(SettingsSection::Reading).await?.value {
            SettingsValue::Reading(value) => value,
            _ => return Err(internal("全局阅读设置分区不一致")),
        };
        let global_comic = match self.settings.get(SettingsSection::Comic).await?.value {
            SettingsValue::Comic(value) => value,
            _ => return Err(internal("全局漫画设置分区不一致")),
        };

        let edition_reading = edition_pref
            .as_ref()
            .and_then(|value| value.data.reading.as_ref());
        let media_reading = media_pref
            .as_ref()
            .and_then(|value| value.data.reading.as_ref());
        let edition_comic = edition_pref
            .as_ref()
            .and_then(|value| value.data.comic.as_ref());
        let media_comic = media_pref
            .as_ref()
            .and_then(|value| value.data.comic.as_ref());

        // 作用域按字段逐层叠加，而不是选择一个完整 Patch：媒体条目只覆盖
        // 自己提供的字段，未提供字段继续继承版本和全局设置。
        let effective_reading = apply_reading_patch(
            apply_reading_patch(global_reading, edition_reading),
            media_reading,
        );
        let effective_comic =
            apply_comic_patch(apply_comic_patch(global_comic, edition_comic), media_comic);
        // Wire 中保留最具体的原始覆盖，UI 可据此显示“本资源/版本”来源；
        // effective 值已经包含两层 Patch 的字段级合并结果。
        let reading_patch = media_pref
            .as_ref()
            .and_then(|value| value.data.reading.clone())
            .or_else(|| {
                edition_pref
                    .as_ref()
                    .and_then(|value| value.data.reading.clone())
            });
        let comic_patch = media_pref
            .as_ref()
            .and_then(|value| value.data.comic.clone())
            .or_else(|| {
                edition_pref
                    .as_ref()
                    .and_then(|value| value.data.comic.clone())
            });

        let edition_reading_patch = edition_pref
            .as_ref()
            .and_then(|value| value.data.reading.clone());
        let edition_comic_patch = edition_pref
            .as_ref()
            .and_then(|value| value.data.comic.clone());
        let media_item_reading_patch = media_pref
            .as_ref()
            .and_then(|value| value.data.reading.clone());
        let media_item_comic_patch = media_pref
            .as_ref()
            .and_then(|value| value.data.comic.clone());

        Ok(PreferenceSnapshot {
            media_item_id,
            edition_id,
            reading_patch,
            comic_patch,
            edition_reading_patch,
            edition_comic_patch,
            media_item_reading_patch,
            media_item_comic_patch,
            effective_reading,
            effective_comic,
            media_item_revision: media_pref.map(|value| value.revision),
            edition_revision: edition_pref.map(|value| value.revision),
        })
    }

    async fn load_edition_preference(
        &self,
        edition_id: EditionId,
    ) -> Result<Option<EditionPreference>, AppError> {
        match self.preferences.get_edition(edition_id).await {
            // 损坏的覆盖只影响该层；读取侧回退到全局默认并保留数据库行供
            // 诊断/后续重置，不让一个坏 JSON 阻塞打开资源。
            Err(error) if error.code().as_str() == "SETTINGS_DATA_CORRUPT" => Ok(None),
            result => result,
        }
    }

    async fn load_media_item_preference(
        &self,
        media_item_id: MediaItemId,
    ) -> Result<Option<MediaItemPreference>, AppError> {
        match self.preferences.get_media_item(media_item_id).await {
            Err(error) if error.code().as_str() == "SETTINGS_DATA_CORRUPT" => Ok(None),
            result => result,
        }
    }
}

fn apply_reading_patch(current: ReadingSettings, patch: Option<&ReadingPatch>) -> ReadingSettings {
    if let Some(patch) = patch {
        if let SettingsValue::Reading(value) =
            SettingsPatch::Reading(patch.clone()).apply_to(&SettingsValue::Reading(current.clone()))
        {
            return value;
        }
    }
    current
}

fn apply_comic_patch(current: ComicSettings, patch: Option<&ComicPatch>) -> ComicSettings {
    if let Some(patch) = patch {
        if let SettingsValue::Comic(value) =
            SettingsPatch::Comic(patch.clone()).apply_to(&SettingsValue::Comic(current.clone()))
        {
            return value;
        }
    }
    current
}

fn revision_matches(current: Option<&str>, expected: Option<&str>) -> bool {
    match (current, expected) {
        (None, None) => true,
        (Some(current), Some(expected)) => current == expected,
        _ => false,
    }
}

fn new_revision(scope: &str) -> String {
    format!(
        "pref-{scope}-{:016x}-{:x}",
        UtcMillis::now().0,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.subsec_nanos())
            .unwrap_or(0)
    )
}

fn conflict() -> AppError {
    AppError::new(
        "REVISION_CONFLICT",
        haven_common::ErrorKind::Conflict,
        "资源内设已被其他窗口更新，请重新加载后再保存",
        false,
    )
}

fn not_found(code: &'static str, message: &'static str) -> AppError {
    AppError::new(code, haven_common::ErrorKind::NotFound, message, false)
}

fn internal(message: &'static str) -> AppError {
    AppError::new(
        "INTERNAL_ERROR",
        haven_common::ErrorKind::Internal,
        message,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Effective 合并逻辑的轻量回归；真实 SQLite/CAS 由 infrastructure repository
    // 与应用集成测试覆盖。这里专门验证三层字段级覆盖和 revision 语义。
    #[test]
    fn revision_matching_is_scope_strict() {
        assert!(revision_matches(None, None));
        assert!(!revision_matches(None, Some("x")));
        assert!(!revision_matches(Some("x"), None));
        assert!(revision_matches(Some("x"), Some("x")));
        assert!(!revision_matches(Some("x"), Some("y")));
    }

    #[test]
    fn effective_preferences_merge_each_layer_by_field() {
        let global = ReadingSettings::default();
        let edition = ReadingPatch {
            font_size: Some(haven_domain::settings::ReadingFontSize::Large),
            theme: Some(haven_domain::settings::ReadingTheme::Dark),
            ..ReadingPatch::default()
        };
        let media = ReadingPatch {
            content_width: Some(haven_domain::settings::ReadingContentWidth::Wide),
            ..ReadingPatch::default()
        };
        let merged = apply_reading_patch(apply_reading_patch(global, Some(&edition)), Some(&media));
        assert_eq!(
            merged.font_size,
            haven_domain::settings::ReadingFontSize::Large
        );
        assert_eq!(merged.theme, haven_domain::settings::ReadingTheme::Dark);
        assert_eq!(
            merged.content_width,
            haven_domain::settings::ReadingContentWidth::Wide
        );
    }
}
