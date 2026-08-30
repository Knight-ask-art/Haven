//! Storage/Settings Command 核心逻辑测试（IPC-TAURI-001B 后端部分，审阅修复后）。
//!
//! 覆盖：storage list/register/rebind/disconnect/remove、settings get/update 的命令层行为
//! （错误归一化、未知 ID、非 Connected 拒绝、revision 冲突、非法 patch → INVALID_ARGUMENT、
//! settings.changed 事件仅 changed=true 产生）。

use std::path::PathBuf;
use std::sync::Arc;

use haven_application::wire::{ErrorDto, StorageStatusDto};
use haven_domain::settings::SettingsSection;
use haven_infrastructure::Db;
use haven_tauri_lib::commands::settings::{run_settings_get, run_settings_update};
use haven_tauri_lib::commands::storage_location::{
    run_rebind_local, run_register_local, run_storage_location_disconnect,
    run_storage_location_list, run_storage_location_remove,
};
use haven_tauri_lib::state::AppState;

fn app_state() -> AppState {
    AppState::new(Arc::new(Db::open_in_memory().unwrap()))
}

fn assert_error(err: &ErrorDto, expected_code: &str) {
    assert_eq!(err.code, expected_code, "错误码不符: {err:?}");
    assert!(!err.retryable, "校验类错误不可重试");
}

/// 注册本地目录（路径仅来自 Native 对话框流程：内部函数直接注入测试路径）。
async fn register_local(state: &AppState, name: &str, path: PathBuf) -> String {
    run_register_local(state, name.into(), path)
        .await
        .expect("注册本地目录")
        .to_string()
}

fn json_contains_string(value: &serde_json::Value, needle: &str) -> bool {
    match value {
        serde_json::Value::String(text) => text.contains(needle),
        serde_json::Value::Array(items) => {
            items.iter().any(|item| json_contains_string(item, needle))
        }
        serde_json::Value::Object(fields) => fields
            .values()
            .any(|field| json_contains_string(field, needle)),
        _ => false,
    }
}

#[tokio::test]
async fn storage_list_register_and_restart_survive() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("storage.db");
    let media = tempfile::TempDir::new().unwrap();
    let id;

    {
        let state = app_state_with_path(&db_path);
        id = register_local(&state, "电影库", media.path().to_path_buf()).await;
        assert_eq!(id.len(), 36, "前端只持有 opaque ID");
        let list = run_storage_location_list(&state).await.unwrap();
        assert_eq!(list.len(), 1);
        let serialized = serde_json::to_value(&list[0]).unwrap();
        let fields = serialized.as_object().expect("StorageLocationDto 是对象");
        assert_eq!(fields.len(), 4, "公开 DTO 必须保持四字段白名单");
        assert_eq!(serialized["locationId"], id.as_str());
        assert_eq!(serialized["displayName"], "电影库");
        assert_eq!(serialized["providerType"], "local");
        assert_eq!(serialized["status"], "connected");
        for sensitive in [
            "rootPath",
            "rootRef",
            "root_ref",
            "credentialRef",
            "credential_ref",
        ] {
            assert!(
                !fields.contains_key(sensitive),
                "StorageLocationDto 不得输出 {sensitive}"
            );
        }
        assert!(
            !json_contains_string(&serialized, media.path().to_string_lossy().as_ref()),
            "公开 DTO 不得包含测试目录路径: {serialized}"
        );
    }
    // 重启恢复
    let state = app_state_with_path(&db_path);
    let list = run_storage_location_list(&state).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].location_id, id);
    assert_eq!(list[0].display_name, "电影库");
}

fn app_state_with_path(db_path: &std::path::Path) -> AppState {
    AppState::new(Arc::new(Db::open(db_path).unwrap()))
}

#[tokio::test]
async fn storage_register_invalid_path_maps_to_invalid_argument() {
    let state = app_state();
    let err = run_register_local(&state, "库".into(), PathBuf::from("relative/path"))
        .await
        .expect_err("相对路径必须拒绝");
    assert_error(&err, "INVALID_ARGUMENT");
}

#[tokio::test]
async fn storage_rebind_disconnect_remove_lifecycle() {
    let dir_a = tempfile::TempDir::new().unwrap();
    let dir_b = tempfile::TempDir::new().unwrap();
    let state = app_state();
    let id = register_local(&state, "库", dir_a.path().to_path_buf()).await;

    // 重新绑定（Native 选择的新路径）
    run_rebind_local(&state, id.clone(), dir_b.path().to_path_buf())
        .await
        .expect("重新绑定成功");
    let list = run_storage_location_list(&state).await.unwrap();
    assert_eq!(list[0].location_id, id);
    assert_eq!(list[0].display_name, "库");
    assert_eq!(list[0].status, StorageStatusDto::Connected);
    let rebound = serde_json::to_value(&list[0]).expect("StorageLocationDto 可序列化");
    assert!(
        !json_contains_string(&rebound, dir_b.path().to_string_lossy().as_ref()),
        "rebind 后公开 DTO 仍不得包含新目录路径: {rebound}"
    );

    // 断开（幂等）
    run_storage_location_disconnect(&state, id.clone())
        .await
        .unwrap();
    run_storage_location_disconnect(&state, id.clone())
        .await
        .unwrap();
    assert_eq!(
        run_storage_location_list(&state).await.unwrap()[0].status,
        StorageStatusDto::Disconnected
    );
    let serialized = serde_json::to_value(&run_storage_location_list(&state).await.unwrap()[0])
        .expect("StorageLocationDto 可序列化");
    assert_eq!(serialized["status"], "disconnected");

    // 移除
    run_storage_location_remove(&state, id).await.unwrap();
    assert!(run_storage_location_list(&state).await.unwrap().is_empty());
    assert!(dir_b.path().exists(), "remove 不得删除用户原始目录");
}

#[tokio::test]
async fn storage_unknown_id_returns_invalid_id_or_not_found() {
    let state = app_state();
    let err = run_storage_location_disconnect(&state, "not-a-uuid".into())
        .await
        .expect_err("非法 ID");
    assert_error(&err, "INVALID_ID");

    let err = run_storage_location_remove(&state, "0196f0d2-0000-7000-8000-0000000000ff".into())
        .await
        .expect_err("未知 ID");
    assert_error(&err, "RESOURCE_NOT_FOUND");
}

#[tokio::test]
async fn settings_get_returns_defaults_then_update_persists() {
    let state = app_state();
    let snap = run_settings_get(&state, "general".into()).await.unwrap();
    assert!(snap.revision.is_none(), "从未保存 → revision=None");
    assert_eq!(snap.value.section(), SettingsSection::General);

    // 部分更新（camelCase 字段 + section tag）→ changed=true + 事件
    let outcome = run_settings_update(
        &state,
        "general".into(),
        None,
        serde_json::json!({ "section": "general", "launchPage": "library" }),
    )
    .await
    .expect("更新成功");
    assert!(outcome.result.changed);
    let rev = outcome.result.revision.clone().unwrap();
    let event = outcome.event.expect("实际变化必须产生 settings.changed");
    assert_eq!(event.section, "general");
    assert_eq!(event.revision, rev, "事件 revision 与 Result 同源");

    // 幂等重复更新（带当前 revision）→ changed=false + 无事件
    let same = run_settings_update(
        &state,
        "general".into(),
        Some(rev.clone()),
        serde_json::json!({ "section": "general", "launchPage": "library" }),
    )
    .await
    .unwrap();
    assert!(!same.result.changed);
    assert!(same.event.is_none(), "幂等更新不发布 settings.changed");
    assert_eq!(same.result.revision.as_deref(), Some(rev.as_str()));
}

#[tokio::test]
async fn settings_playback_defaults_roundtrip_through_command_boundary() {
    let state = app_state();
    let initial = run_settings_get(&state, "playback".into())
        .await
        .expect("读取 playback 默认值");
    assert_eq!(initial.value.section(), SettingsSection::Playback);
    assert!(initial.revision.is_none());

    let outcome = run_settings_update(
        &state,
        "playback".into(),
        None,
        serde_json::json!({
            "section": "playback",
            "defaultPlaybackRate": "one_point_five",
            "autoResume": false,
            "autoNext": false
        }),
    )
    .await
    .expect("更新 playback 默认值");
    assert!(outcome.result.changed);
    assert_eq!(
        outcome.event.as_ref().map(|event| event.section.as_str()),
        Some("playback")
    );

    let updated = run_settings_get(&state, "playback".into())
        .await
        .expect("读取已保存 playback 值");
    let payload = serde_json::to_value(updated.value).expect("playback DTO 可序列化");
    assert_eq!(payload["defaultPlaybackRate"], "one_point_five");
    assert_eq!(payload["autoResume"], false);
    assert_eq!(payload["autoNext"], false);
    assert_eq!(updated.revision, outcome.result.revision);
}

#[tokio::test]
async fn settings_downloads_defaults_and_policy_roundtrip_through_command_boundary() {
    let state = app_state();
    let initial = run_settings_get(&state, "downloads".into())
        .await
        .expect("读取 downloads 默认值");
    assert_eq!(initial.value.section(), SettingsSection::Downloads);
    assert!(initial.revision.is_none());
    let initial_json = serde_json::to_value(initial.value).expect("downloads 默认 DTO 可序列化");
    assert_eq!(initial_json["concurrentTasks"], "three");
    assert_eq!(initial_json["speedLimit"], "unlimited");
    assert_eq!(initial_json["autoContinue"], true);

    let outcome = run_settings_update(
        &state,
        "downloads".into(),
        None,
        serde_json::json!({
            "section": "downloads",
            "concurrentTasks": "five",
            "speedLimit": "mbps2",
            "autoContinue": false
        }),
    )
    .await
    .expect("更新 downloads 策略");
    assert!(outcome.result.changed);
    assert_eq!(
        outcome.event.as_ref().map(|event| event.section.as_str()),
        Some("downloads")
    );

    let updated = run_settings_get(&state, "downloads".into())
        .await
        .expect("读取已保存 downloads 值");
    let payload = serde_json::to_value(updated.value).expect("downloads DTO 可序列化");
    assert_eq!(payload["concurrentTasks"], "five");
    assert_eq!(payload["speedLimit"], "mbps2");
    assert_eq!(payload["autoContinue"], false);
    assert_eq!(updated.revision, outcome.result.revision);

    let err = run_settings_update(
        &state,
        "downloads".into(),
        updated.revision,
        serde_json::json!({
            "section": "downloads",
            "speedLimit": "gigabytes_per_second"
        }),
    )
    .await
    .expect_err("非法限速枚举必须拒绝");
    assert_error(&err, "INVALID_ARGUMENT");
}

#[tokio::test]
async fn settings_comic_defaults_and_policy_roundtrip_through_command_boundary() {
    let state = app_state();
    let initial = run_settings_get(&state, "comic".into())
        .await
        .expect("读取 comic 默认值");
    assert_eq!(initial.value.section(), SettingsSection::Comic);
    assert!(initial.revision.is_none());
    let initial_json = serde_json::to_value(initial.value).expect("comic 默认 DTO 可序列化");
    assert_eq!(initial_json["viewMode"], "single");
    assert_eq!(initial_json["direction"], "rtl");
    assert_eq!(initial_json["pageGap"], "twelve");
    assert_eq!(initial_json["preloadPages"], "three");

    let outcome = run_settings_update(
        &state,
        "comic".into(),
        None,
        serde_json::json!({
            "section": "comic",
            "viewMode": "double",
            "direction": "ltr",
            "pageGap": "twenty_four",
            "preloadPages": "five"
        }),
    )
    .await
    .expect("更新 comic 默认值");
    assert!(outcome.result.changed);
    assert_eq!(
        outcome.event.as_ref().map(|event| event.section.as_str()),
        Some("comic")
    );

    let updated = run_settings_get(&state, "comic".into())
        .await
        .expect("读取已保存 comic 值");
    let payload = serde_json::to_value(updated.value).expect("comic DTO 可序列化");
    assert_eq!(payload["viewMode"], "double");
    assert_eq!(payload["direction"], "ltr");
    assert_eq!(payload["pageGap"], "twenty_four");
    assert_eq!(payload["preloadPages"], "five");
    assert_eq!(updated.revision, outcome.result.revision);
}

#[tokio::test]
async fn settings_privacy_history_switch_roundtrips_through_command_boundary() {
    let state = app_state();
    let initial = run_settings_get(&state, "privacy".into())
        .await
        .expect("读取 privacy 默认值");
    let initial_json = serde_json::to_value(initial.value).expect("privacy 默认 DTO 可序列化");
    assert_eq!(initial_json["searchHistory"], true);
    assert_eq!(initial_json["playbackHistory"], true);

    let outcome = run_settings_update(
        &state,
        "privacy".into(),
        None,
        serde_json::json!({ "section": "privacy", "playbackHistory": false }),
    )
    .await
    .expect("关闭播放与阅读历史");
    assert!(outcome.result.changed);

    let updated = run_settings_get(&state, "privacy".into())
        .await
        .expect("读取已保存 privacy 值");
    let payload = serde_json::to_value(updated.value).expect("privacy DTO 可序列化");
    assert_eq!(payload["searchHistory"], true);
    assert_eq!(payload["playbackHistory"], false);
    assert_eq!(updated.revision, outcome.result.revision);
}

#[tokio::test]
async fn settings_stale_revision_conflicts() {
    let state = app_state();
    let first = run_settings_update(
        &state,
        "appearance".into(),
        None,
        serde_json::json!({ "section": "appearance", "theme": "dark" }),
    )
    .await
    .unwrap();
    let rev = first.result.revision.unwrap();

    let err = run_settings_update(
        &state,
        "appearance".into(),
        Some("set-stale-version".into()),
        serde_json::json!({ "section": "appearance", "theme": "light" }),
    )
    .await
    .expect_err("过期 revision 必须冲突");
    assert_error(&err, "REVISION_CONFLICT");
    assert!(!rev.is_empty());
}

#[tokio::test]
async fn settings_invalid_inputs_map_to_invalid_argument() {
    let state = app_state();

    // 未知 Section
    let err = run_settings_get(&state, "bogus".into())
        .await
        .expect_err("未知 section");
    assert_error(&err, "INVALID_ARGUMENT");

    // 未知字段
    let err = run_settings_update(
        &state,
        "general".into(),
        None,
        serde_json::json!({ "section": "general", "bogusField": 1 }),
    )
    .await
    .expect_err("未知字段必须拒绝");
    assert_error(&err, "INVALID_ARGUMENT");

    // 非法枚举
    let err = run_settings_update(
        &state,
        "appearance".into(),
        None,
        serde_json::json!({ "section": "appearance", "theme": "neon" }),
    )
    .await
    .expect_err("非法枚举必须拒绝");
    assert_error(&err, "INVALID_ARGUMENT");

    // patch section 与请求不一致
    let err = run_settings_update(
        &state,
        "general".into(),
        None,
        serde_json::json!({ "section": "appearance", "theme": "dark" }),
    )
    .await
    .expect_err("section 不一致必须拒绝");
    assert_error(&err, "INVALID_ARGUMENT");
}

#[tokio::test]
async fn settings_unchanged_value_returns_no_event() {
    let state = app_state();
    // 空 patch（值不变）→ changed=false + 无事件
    let outcome = run_settings_update(
        &state,
        "general".into(),
        None,
        serde_json::json!({ "section": "general" }),
    )
    .await
    .unwrap();
    assert!(!outcome.result.changed);
    assert!(outcome.event.is_none());
}
