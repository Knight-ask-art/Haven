//! SettingsService 集成测试（BE-SETTINGS-001 验收 + R-MAIN-01 复审回归）。
//!
//! 真实 SQLite + file-backed：默认值 / 更新持久化 / 重启恢复 / 幂等 / revision 冲突 /
//! 非法枚举与未知字段拒绝 / 未知 Section / 迁移升级 / **stale-idempotent 与双连接并发**。
//!
//! 并发控制契约（R-MAIN-01）：
//! - expected 校验**先于**一切（含幂等短路）：过期 revision 即使提交相同值也冲突；
//! - 已有行 + expected=None → 冲突；从未保存 + 非空 expected → 冲突；
//! - 并发（含双连接）同一 expected_revision 恰好一个成功。

use std::sync::Arc;

use haven_application::services::resource_preferences::{
    PreferenceTarget, ResourcePreferenceService,
};
use haven_application::services::settings::{SettingsService, SettingsUpdateResult};
use haven_domain::settings::{
    AppearancePatch, ComicDirection, ComicPageGap, ComicPatch, ComicPreloadPages, ComicViewMode,
    Density, DownloadConcurrency, DownloadPatch, DownloadSpeedLimit, GeneralPatch, LaunchPage,
    PlaybackPatch, PlaybackRate, PreferenceData, PrivacyPatch, ReadingContentWidth,
    ReadingFontFamily, ReadingFontSize, ReadingLineHeight, ReadingPatch, ReadingTheme,
    SettingsPatch, SettingsSection, SettingsValue, Theme,
};
use haven_infrastructure::Db;
use haven_infrastructure::db::repos::{SqliteRepositories, SqliteSettingsUoW};

fn service(db: &Arc<Db>) -> SettingsService {
    SettingsService::new(Arc::new(SqliteSettingsUoW::new(db.clone())))
}

fn general_patch(launch_page: Option<LaunchPage>) -> SettingsPatch {
    SettingsPatch::General(GeneralPatch {
        launch_page,
        restore_session: None,
        language: None,
        notifications: None,
    })
}

fn appearance_patch(theme: Option<Theme>) -> SettingsPatch {
    SettingsPatch::Appearance(AppearancePatch {
        theme,
        density: None,
        sidebar: None,
        reduce_motion: None,
    })
}

#[tokio::test]
async fn get_returns_defaults_without_revision() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let general = svc.get(SettingsSection::General).await.unwrap();
    assert_eq!(
        general.value,
        SettingsValue::default_for(SettingsSection::General)
    );
    assert!(general.revision.is_none(), "从未保存 → revision=None");

    let appearance = svc.get(SettingsSection::Appearance).await.unwrap();
    assert_eq!(
        appearance.value,
        SettingsValue::default_for(SettingsSection::Appearance)
    );
    assert!(appearance.revision.is_none());

    let privacy = svc.get(SettingsSection::Privacy).await.unwrap();
    assert_eq!(
        privacy.value,
        SettingsValue::default_for(SettingsSection::Privacy)
    );
    assert!(privacy.revision.is_none());
}

#[tokio::test]
async fn resource_preferences_merge_global_edition_and_media_item_layers() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let settings = service(&db);
    settings
        .update(
            SettingsSection::Reading,
            None,
            SettingsPatch::Reading(ReadingPatch {
                font_family: Some(ReadingFontFamily::Sans),
                custom_font_family: None,
                font_size: None,
                line_height: None,
                content_width: None,
                theme: None,
                custom_background: None,
                custom_text: None,
                font_weight: None,
                letter_spacing: None,
                system_auto: None,
                pagination: None,
            }),
        )
        .await
        .unwrap();

    let work_id = haven_domain::ids::WorkId::new();
    let edition_id = haven_domain::ids::EditionId::new();
    let media_item_id = haven_domain::ids::MediaItemId::new();
    let now = haven_common::UtcMillis::now().0;
    db.with_tx(|conn| {
        let work = work_id.to_string();
        let edition = edition_id.to_string();
        let media_item = media_item_id.to_string();
        conn.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, '资源偏好测试', 'fiction', 'completed', ?2, ?2)",
            (&work, now),
        )
        .map_err(|error| {
            haven_common::AppError::new(
                "DATABASE_ERROR",
                haven_common::ErrorKind::Database,
                "写入资源偏好测试作品失败",
                false,
            )
            .with_source(error)
        })?;
        conn.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, '版本', 'book', ?3, ?3)",
            (&edition, &work, now),
        )
        .map_err(|error| {
            haven_common::AppError::new(
                "DATABASE_ERROR",
                haven_common::ErrorKind::Database,
                "写入资源偏好测试版本失败",
                false,
            )
            .with_source(error)
        })?;
        conn.execute(
            "INSERT INTO media_items (id, edition_id, media_type, title, status, created_at, updated_at)
             VALUES (?1, ?2, 'book', '资源', 'available', ?3, ?3)",
            (&media_item, &edition, now),
        )
        .map_err(|error| {
            haven_common::AppError::new(
                "DATABASE_ERROR",
                haven_common::ErrorKind::Database,
                "写入资源偏好测试媒体条目失败",
                false,
            )
            .with_source(error)
        })?;
        Ok(())
    })
    .unwrap();

    let repos = Arc::new(SqliteRepositories::new(db.clone()));
    let preferences = ResourcePreferenceService::new(
        repos.clone(),
        repos.clone(),
        repos.clone(),
        settings.clone(),
    );
    let edition_result = preferences
        .update(
            media_item_id,
            edition_id,
            PreferenceTarget::Edition,
            PreferenceData {
                reading: Some(ReadingPatch {
                    font_size: Some(ReadingFontSize::Large),
                    theme: Some(ReadingTheme::Dark),
                    ..ReadingPatch::default()
                }),
                comic: Some(ComicPatch {
                    view_mode: Some(ComicViewMode::Double),
                    ..ComicPatch::default()
                }),
            },
            None,
        )
        .await
        .unwrap();
    assert!(edition_result.changed);

    let media_result = preferences
        .update(
            media_item_id,
            edition_id,
            PreferenceTarget::MediaItem,
            PreferenceData {
                reading: Some(ReadingPatch {
                    content_width: Some(ReadingContentWidth::Wide),
                    ..ReadingPatch::default()
                }),
                comic: Some(ComicPatch {
                    direction: Some(ComicDirection::Ltr),
                    ..ComicPatch::default()
                }),
            },
            None,
        )
        .await
        .unwrap();
    assert!(media_result.changed);
    assert_eq!(
        media_result.snapshot.effective_reading.font_family,
        ReadingFontFamily::Sans
    );
    assert_eq!(
        media_result.snapshot.effective_reading.font_size,
        ReadingFontSize::Large
    );
    assert_eq!(
        media_result.snapshot.effective_reading.content_width,
        ReadingContentWidth::Wide
    );
    assert_eq!(
        media_result.snapshot.effective_reading.theme,
        ReadingTheme::Dark
    );
    assert_eq!(
        media_result.snapshot.effective_comic.view_mode,
        ComicViewMode::Double
    );
    assert_eq!(
        media_result.snapshot.effective_comic.direction,
        ComicDirection::Ltr
    );
    assert_eq!(
        media_result
            .snapshot
            .edition_reading_patch
            .as_ref()
            .and_then(|patch| patch.font_size),
        Some(ReadingFontSize::Large)
    );
    assert_eq!(
        media_result
            .snapshot
            .media_item_reading_patch
            .as_ref()
            .and_then(|patch| patch.content_width),
        Some(ReadingContentWidth::Wide)
    );

    // Updating only the Comic section must preserve the existing Reading patch.
    let media_revision = media_result.snapshot.media_item_revision.clone();
    let preserved_reading = media_result.snapshot.media_item_reading_patch.clone();
    let comic_only = preferences
        .update(
            media_item_id,
            edition_id,
            PreferenceTarget::MediaItem,
            PreferenceData {
                reading: preserved_reading.clone(),
                comic: Some(ComicPatch {
                    direction: Some(ComicDirection::Rtl),
                    ..ComicPatch::default()
                }),
            },
            media_revision.as_deref(),
        )
        .await
        .unwrap();
    assert!(comic_only.changed);
    assert_eq!(
        comic_only.snapshot.effective_reading.content_width,
        ReadingContentWidth::Wide
    );
    assert_eq!(
        comic_only.snapshot.effective_comic.direction,
        ComicDirection::Rtl
    );

    // A null Reading patch clears only that section; the Comic override remains.
    let reset_revision = comic_only.snapshot.media_item_revision.clone();
    let comic_preserved = comic_only.snapshot.media_item_comic_patch.clone();
    let reset_reading = preferences
        .update(
            media_item_id,
            edition_id,
            PreferenceTarget::MediaItem,
            PreferenceData {
                reading: None,
                comic: comic_preserved,
            },
            reset_revision.as_deref(),
        )
        .await
        .unwrap();
    assert!(reset_reading.changed);
    assert!(reset_reading.snapshot.media_item_reading_patch.is_none());
    assert_eq!(
        reset_reading.snapshot.effective_reading.content_width,
        ReadingContentWidth::Medium
    );
    assert_eq!(
        reset_reading.snapshot.effective_comic.direction,
        ComicDirection::Rtl
    );
}

#[tokio::test]
async fn privacy_search_history_setting_persists_and_uses_independent_revision() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let first = svc
        .update(
            SettingsSection::Privacy,
            None,
            SettingsPatch::Privacy(PrivacyPatch {
                search_history: Some(false),
                playback_history: None,
            }),
        )
        .await
        .unwrap();
    assert!(first.changed);
    let revision = first.revision.clone().expect("privacy 变化必须带 revision");
    match &first.value {
        SettingsValue::Privacy(value) => assert!(!value.search_history),
        _ => panic!("section 必须一致"),
    }

    let snapshot = svc.get(SettingsSection::Privacy).await.unwrap();
    assert_eq!(snapshot.revision.as_deref(), Some(revision.as_str()));
    match snapshot.value {
        SettingsValue::Privacy(value) => assert!(!value.search_history),
        _ => panic!("section 必须一致"),
    }
}

#[tokio::test]
async fn privacy_playback_history_setting_persists_and_survives_restart() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("privacy-settings.db");

    let (revision, expected_value) = {
        let db = Arc::new(Db::open(&db_path).unwrap());
        let svc = service(&db);
        let initial = svc.get(SettingsSection::Privacy).await.unwrap();
        assert_eq!(
            initial.value,
            SettingsValue::default_for(SettingsSection::Privacy)
        );
        assert!(initial.revision.is_none());

        let first = svc
            .update(
                SettingsSection::Privacy,
                None,
                SettingsPatch::Privacy(PrivacyPatch {
                    search_history: None,
                    playback_history: Some(false),
                }),
            )
            .await
            .unwrap();
        assert!(first.changed);
        let revision = first.revision.clone().expect("privacy 变化必须带 revision");
        assert_eq!(
            first.value,
            SettingsValue::Privacy(haven_domain::settings::PrivacySettings {
                search_history: true,
                playback_history: false,
            })
        );
        (revision, first.value)
    };

    let db = Arc::new(Db::open(&db_path).unwrap());
    let svc = service(&db);
    let restored = svc.get(SettingsSection::Privacy).await.unwrap();
    assert_eq!(restored.revision.as_deref(), Some(revision.as_str()));
    assert_eq!(restored.value, expected_value);
}

#[tokio::test]
async fn playback_defaults_persist_and_roundtrip_without_migration() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let first = svc
        .update(
            SettingsSection::Playback,
            None,
            SettingsPatch::Playback(PlaybackPatch {
                default_playback_rate: Some(PlaybackRate::OnePointTwoFive),
                auto_resume: Some(false),
                auto_next: Some(false),
            }),
        )
        .await
        .unwrap();
    assert!(first.changed);
    match first.value {
        SettingsValue::Playback(value) => {
            assert_eq!(value.default_playback_rate, PlaybackRate::OnePointTwoFive);
            assert!(!value.auto_resume);
            assert!(!value.auto_next);
        }
        _ => panic!("section 必须一致"),
    }

    let snapshot = svc.get(SettingsSection::Playback).await.unwrap();
    assert!(snapshot.revision.is_some());
    match snapshot.value {
        SettingsValue::Playback(value) => {
            assert_eq!(value.default_playback_rate, PlaybackRate::OnePointTwoFive);
            assert!(!value.auto_resume);
            assert!(!value.auto_next);
        }
        _ => panic!("section 必须一致"),
    }
}

#[tokio::test]
async fn reading_defaults_persist_and_roundtrip_without_migration() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let initial = svc.get(SettingsSection::Reading).await.unwrap();
    assert_eq!(
        initial.value,
        SettingsValue::default_for(SettingsSection::Reading)
    );
    assert!(initial.revision.is_none());

    let first = svc
        .update(
            SettingsSection::Reading,
            None,
            SettingsPatch::Reading(ReadingPatch {
                font_family: Some(ReadingFontFamily::Kai),
                custom_font_family: None,
                font_size: Some(ReadingFontSize::Large),
                line_height: Some(ReadingLineHeight::Airy),
                content_width: Some(ReadingContentWidth::Wide),
                theme: Some(ReadingTheme::Dark),
                custom_background: None,
                custom_text: None,
                font_weight: None,
                letter_spacing: None,
                system_auto: None,
                pagination: None,
            }),
        )
        .await
        .unwrap();
    assert!(first.changed);
    let revision = first.revision.clone().expect("reading 变化必须带 revision");
    match &first.value {
        SettingsValue::Reading(value) => {
            assert_eq!(value.font_family, ReadingFontFamily::Kai);
            assert_eq!(value.font_size, ReadingFontSize::Large);
            assert_eq!(value.line_height, ReadingLineHeight::Airy);
            assert_eq!(value.content_width, ReadingContentWidth::Wide);
            assert_eq!(value.theme, ReadingTheme::Dark);
        }
        _ => panic!("section 必须一致"),
    }

    let snapshot = svc.get(SettingsSection::Reading).await.unwrap();
    assert_eq!(snapshot.revision.as_deref(), Some(revision.as_str()));
    assert_eq!(snapshot.value, first.value);
}

#[tokio::test]
async fn comic_defaults_persist_and_roundtrip_without_migration() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let initial = svc.get(SettingsSection::Comic).await.unwrap();
    assert_eq!(
        initial.value,
        SettingsValue::default_for(SettingsSection::Comic)
    );
    assert!(initial.revision.is_none());

    let first = svc
        .update(
            SettingsSection::Comic,
            None,
            SettingsPatch::Comic(ComicPatch {
                view_mode: Some(ComicViewMode::Double),
                direction: Some(ComicDirection::Ltr),
                page_gap: Some(ComicPageGap::TwentyFour),
                preload_pages: Some(ComicPreloadPages::Five),
            }),
        )
        .await
        .unwrap();
    assert!(first.changed);
    let revision = first.revision.clone().expect("comic 变化必须带 revision");
    assert_eq!(
        first.value,
        SettingsValue::Comic(haven_domain::settings::ComicSettings {
            view_mode: ComicViewMode::Double,
            direction: ComicDirection::Ltr,
            page_gap: ComicPageGap::TwentyFour,
            preload_pages: ComicPreloadPages::Five,
        })
    );

    let snapshot = svc.get(SettingsSection::Comic).await.unwrap();
    assert_eq!(snapshot.revision.as_deref(), Some(revision.as_str()));
    assert_eq!(snapshot.value, first.value);
}

#[tokio::test]
async fn settings_downloads_defaults_persist_and_survive_restart() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("downloads-settings.db");

    let (revision, expected_value) = {
        let db = Arc::new(Db::open(&db_path).unwrap());
        let svc = service(&db);
        let initial = svc.get(SettingsSection::Downloads).await.unwrap();
        assert_eq!(
            initial.value,
            SettingsValue::default_for(SettingsSection::Downloads)
        );
        assert!(initial.revision.is_none());

        let first = svc
            .update(
                SettingsSection::Downloads,
                None,
                SettingsPatch::Downloads(DownloadPatch {
                    concurrent_tasks: Some(DownloadConcurrency::Five),
                    speed_limit: Some(DownloadSpeedLimit::Mbps2),
                    auto_continue: Some(false),
                }),
            )
            .await
            .unwrap();
        assert!(first.changed);
        let revision = first.revision.clone().expect("下载设置变化必须带 revision");
        assert_eq!(
            first.value,
            SettingsValue::Downloads(haven_domain::settings::DownloadSettings {
                concurrent_tasks: DownloadConcurrency::Five,
                speed_limit: DownloadSpeedLimit::Mbps2,
                auto_continue: false,
            })
        );

        let stale = svc
            .update(
                SettingsSection::Downloads,
                Some("set-stale-downloads"),
                SettingsPatch::Downloads(DownloadPatch {
                    concurrent_tasks: Some(DownloadConcurrency::One),
                    speed_limit: None,
                    auto_continue: None,
                }),
            )
            .await
            .unwrap_err();
        assert_eq!(stale.code().as_str(), "REVISION_CONFLICT");

        (revision, first.value)
    };

    let db = Arc::new(Db::open(&db_path).unwrap());
    let svc = service(&db);
    let restored = svc.get(SettingsSection::Downloads).await.unwrap();
    assert_eq!(restored.revision.as_deref(), Some(revision.as_str()));
    assert_eq!(restored.value, expected_value);
}

#[tokio::test]
async fn update_persists_and_survives_restart() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("settings.db");
    let db = Arc::new(Db::open(&db_path).unwrap());
    let svc = service(&db);

    // 首次更新：expected=None → 成功 + changed=true + 非空 revision
    let result = svc
        .update(
            SettingsSection::General,
            None,
            general_patch(Some(LaunchPage::Library)),
        )
        .await
        .unwrap();
    assert!(result.changed);
    let revision = result.revision.clone().expect("变化必须带 revision");
    match &result.value {
        SettingsValue::General(g) => assert_eq!(g.launch_page, LaunchPage::Library),
        _ => panic!("section 必须一致"),
    }

    // 重启后恢复
    drop(svc);
    drop(db);
    let db2 = Arc::new(Db::open(&db_path).unwrap());
    let svc2 = service(&db2);
    let snap = svc2.get(SettingsSection::General).await.unwrap();
    match &snap.value {
        SettingsValue::General(g) => assert_eq!(g.launch_page, LaunchPage::Library),
        _ => panic!(),
    }
    assert_eq!(
        snap.revision.as_deref(),
        Some(revision.as_str()),
        "重启后 revision 一致"
    );
}

#[tokio::test]
async fn same_value_update_is_idempotent() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let first = svc
        .update(
            SettingsSection::Appearance,
            None,
            appearance_patch(Some(Theme::Dark)),
        )
        .await
        .unwrap();
    assert!(first.changed);
    let rev1 = first.revision.clone().unwrap();

    // 相同值 + 当前 revision → 幂等（changed=false + 相同 revision + 不写库）
    let same = svc
        .update(
            SettingsSection::Appearance,
            Some(&rev1),
            appearance_patch(Some(Theme::Dark)),
        )
        .await
        .unwrap();
    assert!(!same.changed, "相同值重复更新不得视为变更");
    assert_eq!(
        same.revision.as_deref(),
        Some(rev1.as_str()),
        "幂等返回当前 revision"
    );

    // 空 patch（无字段变化）同样幂等
    let empty = svc
        .update(
            SettingsSection::Appearance,
            Some(&rev1),
            SettingsPatch::Appearance(AppearancePatch::default()),
        )
        .await
        .unwrap();
    assert!(!empty.changed);
    assert_eq!(empty.revision.as_deref(), Some(rev1.as_str()));
}

#[tokio::test]
async fn stale_revision_conflicts() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let first = svc
        .update(
            SettingsSection::General,
            None,
            general_patch(Some(LaunchPage::Library)),
        )
        .await
        .unwrap();
    let rev1 = first.revision.clone().unwrap();

    // 过期 revision → REVISION_CONFLICT，不静默覆盖
    let stale = svc
        .update(
            SettingsSection::General,
            Some("set-0000000000000000-0"),
            general_patch(Some(LaunchPage::Home)),
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code().as_str(), "REVISION_CONFLICT");

    // 从未保存但带 expected revision → 冲突
    let err = svc
        .update(
            SettingsSection::Appearance,
            Some("set-x"),
            appearance_patch(Some(Theme::Light)),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code().as_str(), "REVISION_CONFLICT");

    // 未受影响的 Section 保持原值
    let general = svc.get(SettingsSection::General).await.unwrap();
    match general.value {
        SettingsValue::General(g) => assert_eq!(g.launch_page, LaunchPage::Library, "冲突不得覆盖"),
        _ => panic!(),
    }
    assert_eq!(general.revision.as_deref(), Some(rev1.as_str()));
}

/// R-MAIN-01 核心回归：**过期 revision + 相同值**必须冲突（幂等短路不得绕过 expected 校验）。
#[tokio::test]
async fn stale_revision_with_same_value_still_conflicts() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    // 当前值 Dark、当前 revision rev1
    let first = svc
        .update(
            SettingsSection::Appearance,
            None,
            appearance_patch(Some(Theme::Dark)),
        )
        .await
        .unwrap();
    let rev1 = first.revision.clone().unwrap();

    // 携带过期 R0 再设置 Dark（相同值）→ 必须 REVISION_CONFLICT，而不是错误成功。
    let stale = svc
        .update(
            SettingsSection::Appearance,
            Some("set-0000000000000000-0"),
            appearance_patch(Some(Theme::Dark)),
        )
        .await
        .unwrap_err();
    assert_eq!(
        stale.code().as_str(),
        "REVISION_CONFLICT",
        "过期 revision 即使提交相同值也必须冲突"
    );

    // 状态未被破坏：revision 仍是 rev1
    let snap = svc.get(SettingsSection::Appearance).await.unwrap();
    assert_eq!(snap.revision.as_deref(), Some(rev1.as_str()));
}

/// R-MAIN-01：**已有行 + expected=None**（即使值相同）→ 冲突（客户端无状态不能盲目提交）。
#[tokio::test]
async fn existing_row_with_expected_none_conflicts() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let first = svc
        .update(
            SettingsSection::Appearance,
            None,
            appearance_patch(Some(Theme::Dark)),
        )
        .await
        .unwrap();
    assert!(first.changed);

    // 已有行 + expected=None + 相同值 → REVISION_CONFLICT（不得绕过）。
    let err = svc
        .update(
            SettingsSection::Appearance,
            None,
            appearance_patch(Some(Theme::Dark)),
        )
        .await
        .unwrap_err();
    assert_eq!(
        err.code().as_str(),
        "REVISION_CONFLICT",
        "已有行携带 expected=None 必须冲突"
    );
}

/// R-MAIN-01：从未保存的 Section，携带任意非空 expected 提交空 patch → 冲突。
#[tokio::test]
async fn never_saved_with_expected_and_empty_patch_conflicts() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let err = svc
        .update(
            SettingsSection::Appearance,
            Some("set-anything"),
            SettingsPatch::Appearance(AppearancePatch::default()),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code().as_str(), "REVISION_CONFLICT");

    // 无任何残留
    let snap = svc.get(SettingsSection::Appearance).await.unwrap();
    assert!(snap.revision.is_none());
}

#[tokio::test]
async fn invalid_inputs_are_rejected_at_boundary() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    // 未知 Section：wire 解析层拒绝（闭合枚举）
    assert_eq!(SettingsSection::parse("bogus"), None);

    // 非法枚举 / 未知字段 / 类型错误 → 反序列化边界拒绝
    assert!(
        serde_json::from_str::<SettingsPatch>(r#"{"section":"general","language":"klingon"}"#)
            .is_err()
    );
    assert!(
        serde_json::from_str::<SettingsPatch>(r#"{"section":"appearance","theme":"neon"}"#)
            .is_err()
    );
    assert!(serde_json::from_str::<SettingsPatch>(r#"{"section":"general","bogus":1}"#).is_err());
    assert!(
        serde_json::from_str::<SettingsPatch>(r#"{"section":"general","launchPage":42}"#).is_err()
    );
    assert!(
        serde_json::from_str::<SettingsPatch>(r#"{"section":"unknown","launchPage":"home"}"#)
            .is_err()
    );

    // patch 与 section 不一致 → 拒绝
    let err = svc
        .update(
            SettingsSection::General,
            None,
            SettingsPatch::Appearance(AppearancePatch {
                theme: Some(Theme::Light),
                density: None,
                sidebar: None,
                reduce_motion: None,
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code().as_str(), "INVALID_ARGUMENT");

    // 拒绝后无任何写入
    let general = svc.get(SettingsSection::General).await.unwrap();
    assert!(general.revision.is_none(), "非法输入不得产生写入");
}

#[tokio::test]
async fn sections_are_isolated() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let g = svc
        .update(
            SettingsSection::General,
            None,
            general_patch(Some(LaunchPage::LastSession)),
        )
        .await
        .unwrap();
    let a = svc
        .update(
            SettingsSection::Appearance,
            None,
            SettingsPatch::Appearance(AppearancePatch {
                theme: Some(Theme::Dark),
                density: Some(Density::Compact),
                sidebar: None,
                reduce_motion: None,
            }),
        )
        .await
        .unwrap();

    // 两个 Section 独立 revision 与值
    assert_ne!(g.revision, a.revision);
    let appearance = svc.get(SettingsSection::Appearance).await.unwrap();
    match appearance.value {
        SettingsValue::Appearance(v) => {
            assert_eq!(v.theme, Theme::Dark);
            assert_eq!(v.density, Density::Compact);
            assert!(!v.reduce_motion, "未提供的字段保持默认");
        }
        _ => panic!(),
    }
    // General 不受影响
    let general = svc.get(SettingsSection::General).await.unwrap();
    match general.value {
        SettingsValue::General(v) => assert_eq!(v.launch_page, LaunchPage::LastSession),
        _ => panic!(),
    }
}

#[tokio::test]
async fn migrations_include_settings_table() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    // 迁移 007 已应用（Db::open_in_memory 全量迁移）；通过 service 读写验证表可用。
    let svc = service(&db);
    let result: SettingsUpdateResult = svc
        .update(
            SettingsSection::General,
            None,
            general_patch(Some(LaunchPage::LastSession)),
        )
        .await
        .unwrap();
    assert!(result.changed);
    let snap = svc.get(SettingsSection::General).await.unwrap();
    match snap.value {
        SettingsValue::General(v) => assert_eq!(v.launch_page, LaunchPage::LastSession),
        _ => panic!(),
    }
}

/// 单连接顺序 CAS：同 expected 两个请求 → 恰好一成一败（原有语义，保持回归）。
#[tokio::test]
async fn sequential_updates_with_same_revision_exactly_one_wins() {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let svc = service(&db);

    let first = svc
        .update(
            SettingsSection::General,
            None,
            general_patch(Some(LaunchPage::Library)),
        )
        .await
        .unwrap();
    assert!(first.changed);
    let rev1 = first.revision.unwrap();

    // 两个请求都持有 rev1（模拟 T1/T2 同时读取同一版本）
    let a = svc
        .update(
            SettingsSection::General,
            Some(&rev1),
            general_patch(Some(LaunchPage::LastSession)),
        )
        .await;
    let b = svc
        .update(
            SettingsSection::General,
            Some(&rev1),
            general_patch(Some(LaunchPage::Home)),
        )
        .await;

    // 恰好一个成功、一个冲突（顺序执行下第一个成功，第二个必然冲突）
    let (ok, conflict) = match (a, b) {
        (Ok(_), Err(e)) => (true, e),
        (Err(e), Ok(_)) => (true, e),
        _ => panic!("必须恰好一个成功一个冲突"),
    };
    assert!(ok);
    assert_eq!(conflict.code().as_str(), "REVISION_CONFLICT");

    // 最终值 = 成功者的写入（不被静默覆盖）
    let snap = svc.get(SettingsSection::General).await.unwrap();
    let value = snap.value;
    let expected = [LaunchPage::LastSession, LaunchPage::Home].contains(&match &value {
        SettingsValue::General(g) => g.launch_page,
        _ => panic!(),
    });
    assert!(expected, "最终值为成功请求的写入（无静默覆盖）");
}

/// R-MAIN-01：**双连接**顺序 CAS——两个独立 Db 实例（file-backed、各自连接）
/// 持有同一 revision：第一个连接成功，第二个连接读到新 revision → 冲突。
#[tokio::test]
async fn two_connections_sequential_cas() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("settings.db");

    // 连接 A（先建库 + 迁移）
    let db_a = Arc::new(Db::open(&db_path).unwrap());
    let svc_a = service(&db_a);
    let first = svc_a
        .update(
            SettingsSection::General,
            None,
            general_patch(Some(LaunchPage::Library)),
        )
        .await
        .unwrap();
    assert!(first.changed);
    let rev1 = first.revision.unwrap();

    // 连接 B 独立打开同一文件（第二个连接）
    let db_b = Arc::new(Db::open(&db_path).unwrap());
    let svc_b = service(&db_b);

    // A 用 rev1 成功推进
    let a = svc_a
        .update(
            SettingsSection::General,
            Some(&rev1),
            general_patch(Some(LaunchPage::LastSession)),
        )
        .await;
    assert!(a.is_ok(), "第一个连接使用当前 revision 必须成功");
    let rev2 = a.unwrap().revision.unwrap();

    // B 仍持旧 rev1（模拟第二窗口的过期快照）→ 冲突；B 使用 rev2 → 成功
    let b_stale = svc_b
        .update(
            SettingsSection::General,
            Some(&rev1),
            general_patch(Some(LaunchPage::Home)),
        )
        .await
        .unwrap_err();
    assert_eq!(
        b_stale.code().as_str(),
        "REVISION_CONFLICT",
        "另一连接的过期 revision 必须冲突"
    );
    let b_ok = svc_b
        .update(
            SettingsSection::General,
            Some(&rev2),
            general_patch(Some(LaunchPage::Home)),
        )
        .await
        .unwrap();
    assert!(b_ok.changed);

    // 最终值 = B 的写入
    let snap = svc_a.get(SettingsSection::General).await.unwrap();
    match snap.value {
        SettingsValue::General(g) => assert_eq!(g.launch_page, LaunchPage::Home),
        _ => panic!(),
    }
}

/// R-MAIN-07：**真并发**（两个独立 file-backed Db + 两个 OS 线程 + Barrier 同时发起）。
/// 写路径 BEGIN IMMEDIATE + 数据库层条件写：恰好一个 changed=true，另一个 REVISION_CONFLICT；
/// **失败码只能是 REVISION_CONFLICT，不得泄漏 DATABASE_ERROR/BUSY**；最终值来自成功请求。
/// 多轮压力 + 每轮两个线程在 barrier 处同时释放（不依赖任何 await 交错的假并发）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_connections_concurrent_cas_exactly_one_wins() {
    use std::sync::Barrier;

    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("settings.db");

    // 预置：首次写入（expected=None），产生 rev1。
    {
        let db = Arc::new(Db::open(&db_path).unwrap());
        let svc = service(&db);
        let first = svc
            .update(
                SettingsSection::General,
                None,
                general_patch(Some(LaunchPage::Library)),
            )
            .await
            .unwrap();
        assert!(first.changed);
    }

    for round in 0..8u32 {
        // 读当前 revision + 当前值（本轮两个竞争者的共同 expected；
        // 目标值从枚举中排除当前值，保证两方都非幂等 → 强制 CAS 竞争而非幂等短路）。
        let (current_rev, current_page) = {
            let db_read = Arc::new(Db::open(&db_path).unwrap());
            let svc_read = service(&db_read);
            let snap = svc_read.get(SettingsSection::General).await.unwrap();
            let page = match snap.value {
                SettingsValue::General(g) => g.launch_page,
                _ => panic!(),
            };
            (snap.revision.unwrap(), page)
        };
        let targets: Vec<LaunchPage> = [
            LaunchPage::Home,
            LaunchPage::Library,
            LaunchPage::Continue,
            LaunchPage::LastSession,
        ]
        .into_iter()
        .filter(|v| *v != current_page)
        .collect();
        let target_a = targets[0];
        let target_b = targets[1];

        // 两个独立连接 + 两个 OS 线程；barrier 保证同时进入 BEGIN IMMEDIATE 竞争。
        let db_a = Arc::new(Db::open(&db_path).unwrap());
        let db_b = Arc::new(Db::open(&db_path).unwrap());
        let svc_a = service(&db_a);
        let svc_b = service(&db_b);

        let barrier = Arc::new(Barrier::new(2));
        let handle = tokio::runtime::Handle::current();

        // 线程 A：target_a；线程 B：target_b（不同值，便于判定胜者）。
        let barrier_a = barrier.clone();
        let handle_a = handle.clone();
        let svc_a2 = svc_a.clone();
        let rev_a = current_rev.clone();
        let th_a = std::thread::spawn(move || {
            barrier_a.wait();
            handle_a.block_on(async move {
                svc_a2
                    .update(
                        SettingsSection::General,
                        Some(&rev_a),
                        general_patch(Some(target_a)),
                    )
                    .await
            })
        });

        let barrier_b = barrier;
        let handle_b = handle.clone();
        let svc_b2 = svc_b.clone();
        let rev_b = current_rev.clone();
        let th_b = std::thread::spawn(move || {
            barrier_b.wait();
            handle_b.block_on(async move {
                svc_b2
                    .update(
                        SettingsSection::General,
                        Some(&rev_b),
                        general_patch(Some(target_b)),
                    )
                    .await
            })
        });

        let result_a = th_a.join().expect("线程 A 不应 panic");
        let result_b = th_b.join().expect("线程 B 不应 panic");

        // 恰好一个 Written(changed=true)；失败方必须是 REVISION_CONFLICT（不是 DATABASE_ERROR）。
        let mut success: Option<SettingsUpdateResult> = None;
        for r in [&result_a, &result_b] {
            match r {
                Ok(res) => {
                    assert!(res.changed, "第 {round} 轮胜者必须 changed=true");
                    assert!(
                        success.replace(res.clone()).is_none(),
                        "每轮必须恰好一个成功"
                    );
                }
                Err(e) => assert_eq!(
                    e.code().as_str(),
                    "REVISION_CONFLICT",
                    "第 {round} 轮失败方必须稳定 REVISION_CONFLICT（实际 {}），不得泄漏 DATABASE_ERROR/BUSY",
                    e.code().as_str()
                ),
            }
        }
        let winner = success.expect("每轮必须恰好一个成功");

        // 最终值 = 胜者写入；revision 被推进（≠ current_rev）。
        let db_check = Arc::new(Db::open(&db_path).unwrap());
        let svc_check = service(&db_check);
        let snap = svc_check.get(SettingsSection::General).await.unwrap();
        assert_eq!(
            snap.revision.as_deref(),
            Some(winner.revision.as_deref().unwrap()),
            "第 {round} 轮 DB revision 必须等于胜者返回的 revision"
        );
        assert_ne!(
            snap.revision.as_deref(),
            Some(current_rev.as_str()),
            "第 {round} 轮 revision 必须被推进"
        );
        let actual = match snap.value {
            SettingsValue::General(g) => g.launch_page,
            _ => panic!(),
        };
        let winner_page = match &winner.value {
            SettingsValue::General(g) => g.launch_page,
            _ => panic!(),
        };
        assert_eq!(actual, winner_page, "第 {round} 轮最终值必须来自成功请求");
        drop(db_check);
        drop(db_a);
        drop(db_b);
    }
}

/// R-MAIN-07：**首次写入并发**——全新 DB，两个独立连接 + OS 线程同时 `expected=None`。
/// 原子 INSERT（DO UPDATE ... WHERE revision IS NULL 恒假）兜底：
/// 恰好一个 changed=true，另一个 REVISION_CONFLICT，绝不静默覆盖，不泄漏 DATABASE_ERROR。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_connections_concurrent_first_write_exactly_one_wins() {
    use std::sync::Barrier;

    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("settings.db");

    let db_a = Arc::new(Db::open(&db_path).unwrap());
    let db_b = Arc::new(Db::open(&db_path).unwrap());
    let svc_a = service(&db_a);
    let svc_b = service(&db_b);

    let barrier = Arc::new(Barrier::new(2));
    let handle = tokio::runtime::Handle::current();

    let barrier_a = barrier.clone();
    let handle_a = handle.clone();
    let svc_a2 = svc_a.clone();
    // 两个目标都必须 ≠ 默认值（General 默认 launch_page=Home），否则幂等短路；
    // 用 LastSession vs Continue，确保两方都产生真实写入竞争。
    let th_a = std::thread::spawn(move || {
        barrier_a.wait();
        handle_a.block_on(async move {
            svc_a2
                .update(
                    SettingsSection::General,
                    None,
                    general_patch(Some(LaunchPage::LastSession)),
                )
                .await
        })
    });

    let barrier_b = barrier;
    let handle_b = handle.clone();
    let svc_b2 = svc_b.clone();
    let th_b = std::thread::spawn(move || {
        barrier_b.wait();
        handle_b.block_on(async move {
            svc_b2
                .update(
                    SettingsSection::General,
                    None,
                    general_patch(Some(LaunchPage::Continue)),
                )
                .await
        })
    });

    let result_a = th_a.join().expect("线程 A 不应 panic");
    let result_b = th_b.join().expect("线程 B 不应 panic");

    let mut success: Option<SettingsUpdateResult> = None;
    for r in [&result_a, &result_b] {
        match r {
            Ok(res) => {
                assert!(res.changed);
                assert!(
                    success.replace(res.clone()).is_none(),
                    "首次并发必须恰好一个成功"
                );
            }
            Err(e) => assert_eq!(
                e.code().as_str(),
                "REVISION_CONFLICT",
                "首次并发失败方必须是 REVISION_CONFLICT（实际 {}），不得泄漏 DATABASE_ERROR",
                e.code().as_str()
            ),
        }
    }
    let winner = success.expect("首次并发必须恰好一个成功");

    // 最终值来自胜者；revision 非空。
    let db_check = Arc::new(Db::open(&db_path).unwrap());
    let svc_check = service(&db_check);
    let snap = svc_check.get(SettingsSection::General).await.unwrap();
    assert_eq!(
        snap.revision.as_deref(),
        Some(winner.revision.as_deref().unwrap())
    );
    let actual = match snap.value {
        SettingsValue::General(g) => g.launch_page,
        _ => panic!(),
    };
    let winner_page = match &winner.value {
        SettingsValue::General(g) => g.launch_page,
        _ => panic!(),
    };
    assert_eq!(actual, winner_page, "首次并发最终值必须来自成功请求");
    drop(db_check);
}
