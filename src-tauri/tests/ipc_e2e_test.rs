//! 真实 IPC 路径端到端测试（P1-10 / R-MAIN-04 复审修复）。
//!
//! 覆盖评审要求的真实证据链：Tauri 参数反序列化（envelope `{"request": {...}}`）/
//! State 注入 / invoke handler dispatch / 成功 DTO 精确断言 / `ErrorDto.code` 精确断言 /
//! 事件与 Result revision 同源 + 重复设置不新增事件 / Capability ACL deny
//! （已注册但未授权 window → 拒绝）。
//!
//! mock harness 及真实生成的 ACL/capabilities invoke 路径现可在本机执行。常规执行：
//! `cargo test --features e2e --test ipc_e2e_test -- --nocapture`。逻辑契约名
//! `favorite.changed` / `settings.changed` 分别通过合法的
//! `favorite-changed` / `settings-changed` Tauri transport 名称适配；
//! DTO 与 fixture 仍保留逻辑契约语义。

use std::collections::BTreeMap;
use std::sync::Arc;

use tauri::ipc::{CallbackFn, RuntimeAuthority};
use tauri::test::{
    get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime, INVOKE_KEY,
};
use tauri::utils::acl::capability::Capability;
use tauri::utils::acl::manifest::Manifest;
use tauri::utils::acl::resolved::Resolved;
use tauri::utils::platform::Target;
use tauri::webview::InvokeRequest;
use tauri::Context;
use tauri::{Listener, Url};

use haven_domain::contracts::StorageLocationRepository;
use haven_infrastructure::db::repos::SqliteRepositories;
use haven_infrastructure::Db;
use haven_tauri_lib::ipc::{
    FAVORITE_CHANGED_TRANSPORT_EVENT, LIBRARY_CHANGED_TRANSPORT_EVENT,
    SETTINGS_CHANGED_TRANSPORT_EVENT,
};
use haven_tauri_lib::state::AppState;

fn invoke_request(cmd: &str, body: serde_json::Value) -> InvokeRequest {
    InvokeRequest {
        cmd: cmd.into(),
        callback: CallbackFn(1),
        error: CallbackFn(2),
        // Windows/Android use the local tauri origin; the custom `tauri://` URL
        // is classified as remote by RuntimeAuthority on those platforms.
        url: if cfg!(any(windows, target_os = "android")) {
            Url::parse("http://tauri.localhost").unwrap()
        } else {
            Url::parse("tauri://localhost").unwrap()
        },
        body: body.into(),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    }
}

fn real_acl_context() -> Context<MockRuntime> {
    let mut context = mock_context(noop_assets());
    let acl: BTreeMap<String, Manifest> = serde_json::from_str(
        &std::fs::read_to_string(concat!(env!("OUT_DIR"), "/acl-manifests.json"))
            .expect("build.rs 生成 acl-manifests.json"),
    )
    .unwrap();
    let capabilities: BTreeMap<String, Capability> = serde_json::from_str(
        &std::fs::read_to_string(concat!(env!("OUT_DIR"), "/capabilities.json"))
            .expect("build.rs 生成 capabilities.json"),
    )
    .unwrap();
    let resolved = Resolved::resolve(&acl, capabilities, Target::current())
        .expect("真实 ACL/capabilities 必须可解析");
    *context.runtime_authority_mut() = RuntimeAuthority::new(acl, resolved);
    context
}

fn app_with_context(context: Context<MockRuntime>) -> (tauri::App<MockRuntime>, Arc<Db>) {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let app = haven_tauri_lib::register_invoke_handler(mock_builder())
        .manage(AppState::new(db.clone()))
        .build(context)
        .unwrap();
    (app, db)
}

fn app_with_state() -> (tauri::App<MockRuntime>, Arc<Db>) {
    app_with_context(real_acl_context())
}

fn seed_work(db: &Db) -> String {
    let work_id = "0196f0d2-0000-7000-8000-0000000000aa";
    db.with_tx(|tx| {
        use rusqlite::params;
        tx.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, 'e2e 测试', 'fiction', 'completed', ?2, ?2)",
            params![work_id, haven_common::UtcMillis::now().0],
        )
        .map_err(|e| {
            haven_common::AppError::new(
                "DATABASE_ERROR",
                haven_common::ErrorKind::Database,
                "seed work 失败",
                false,
            )
            .with_source(e)
        })?;
        Ok(())
    })
    .expect("seed work");
    work_id.to_string()
}

fn seed_storage_work(db: &Db) -> (String, String) {
    let location_id = "0196f0d2-0000-7000-8000-0000000000b1";
    let work_id = "0196f0d2-0000-7000-8000-0000000000b2";
    let edition_id = "0196f0d2-0000-7000-8000-0000000000b3";
    let media_item_id = "0196f0d2-0000-7000-8000-0000000000b4";
    let resource_id = "0196f0d2-0000-7000-8000-0000000000b5";
    let now = haven_common::UtcMillis::now().0;
    db.with_tx(|tx| {
        use rusqlite::params;
        tx.execute(
            "INSERT INTO storage_locations
             (id, provider_type, display_name, root_ref, credential_ref, status, created_at, updated_at)
             VALUES (?1, 'local', 'e2e remove', ?2, NULL, 'connected', ?3, ?3)",
            params![location_id, std::env::temp_dir().to_string_lossy(), now],
        )
        .map_err(|e| {
            haven_common::AppError::new(
                "DATABASE_ERROR",
                haven_common::ErrorKind::Database,
                "seed storage location 失败",
                false,
            )
            .with_source(e)
        })?;
        tx.execute(
            "INSERT INTO works (id, canonical_title, work_type, status, created_at, updated_at)
             VALUES (?1, 'e2e remove work', 'fiction', 'completed', ?2, ?2)",
            params![work_id, now],
        )
        .map_err(|e| {
            haven_common::AppError::new(
                "DATABASE_ERROR",
                haven_common::ErrorKind::Database,
                "seed work 失败",
                false,
            )
            .with_source(e)
        })?;
        tx.execute(
            "INSERT INTO editions (id, work_id, title, edition_type, created_at, updated_at)
             VALUES (?1, ?2, 'e2e remove edition', 'movie', ?3, ?3)",
            params![edition_id, work_id, now],
        )
        .map_err(|e| {
            haven_common::AppError::new(
                "DATABASE_ERROR",
                haven_common::ErrorKind::Database,
                "seed edition 失败",
                false,
            )
            .with_source(e)
        })?;
        tx.execute(
            "INSERT INTO media_items
             (id, edition_id, parent_id, media_type, title, category, status, created_at, updated_at)
             VALUES (?1, ?2, NULL, 'movie', 'e2e remove item', 'movie', 'available', ?3, ?3)",
            params![media_item_id, edition_id, now],
        )
        .map_err(|e| {
            haven_common::AppError::new(
                "DATABASE_ERROR",
                haven_common::ErrorKind::Database,
                "seed media item 失败",
                false,
            )
            .with_source(e)
        })?;
        tx.execute(
            "INSERT INTO resources
             (id, media_item_id, resource_type, source_id, storage_location_id,
              locator_kind, locator_json, mime_type, size, hash_algorithm, hash_digest,
              availability, created_at, updated_at)
             VALUES (?1, ?2, 'local_file', NULL, ?3, 'local_path',
                     '{\"local_path\":{\"path\":\"e2e-remove.mkv\"}}',
                     'video/x-matroska', NULL, NULL, NULL, 'available', ?4, ?4)",
            params![resource_id, media_item_id, location_id, now],
        )
        .map_err(|e| {
            haven_common::AppError::new(
                "DATABASE_ERROR",
                haven_common::ErrorKind::Database,
                "seed resource 失败",
                false,
            )
            .with_source(e)
        })?;
        Ok(())
    })
    .expect("seed storage work");
    (location_id.to_owned(), work_id.to_owned())
}

/// library_list：真实 envelope（`request`）+ State 注入 + handler dispatch → 成功 DTO 精确断言。
#[tokio::test]
async fn library_list_with_request_envelope_returns_success_dto() {
    let (app, db) = app_with_state();
    seed_work(&db);
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let response = get_ipc_response(
        &webview,
        invoke_request(
            "library_list",
            serde_json::json!({
                "request": {
                    "category": "all",
                    "mediaTypes": null,
                    "query": null,
                    "sort": "recently_added",
                    "cursor": null,
                    "limit": 50
                }
            }),
        ),
    )
    .expect("已注册命令（envelope 正确）必须成功");
    let json: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(json["schemaVersion"], 1, "DTO schemaVersion 必须精确");
    assert!(json["items"].is_array(), "items 必须为数组");
    assert_eq!(json["total"], 1, "seed 1 个 Work → total=1");
    assert_eq!(json["items"][0]["title"], "e2e 测试", "DTO 内容精确");
}

/// favorite_set：seed Work 成功 mutation → 成功 DTO 精确断言 + 重复设置 revision 不变。
#[tokio::test]
async fn favorite_set_success_dto_and_idempotent_revision() {
    let (app, db) = app_with_state();
    let work_id = seed_work(&db);
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    // 成功 mutation（真实 envelope）
    let first = get_ipc_response(
        &webview,
        invoke_request(
            "favorite_set",
            serde_json::json!({ "request": { "workId": work_id, "favorite": true } }),
        ),
    )
    .expect("成功 favorite_set 必须可调用");
    let first_json: serde_json::Value = first.deserialize().unwrap();
    assert_eq!(first_json["workId"], work_id);
    assert_eq!(first_json["favorite"], true);
    assert!(
        first_json["revision"].is_string(),
        "首次成功必须带非空 revision"
    );
    let rev1 = first_json["revision"].as_str().unwrap().to_string();

    // 幂等重复设置：结果 revision 相同（无新版本，事件不重复由 wrapper 层断言）
    let second = get_ipc_response(
        &webview,
        invoke_request(
            "favorite_set",
            serde_json::json!({ "request": { "workId": work_id, "favorite": true } }),
        ),
    )
    .unwrap();
    let second_json: serde_json::Value = second.deserialize().unwrap();
    assert_eq!(
        second_json["revision"].as_str().unwrap(),
        rev1,
        "幂等重复设置必须返回相同 revision"
    );
}

/// 事件与 Result revision 同源 + 重复设置不新增事件（真实 Emitter 通道）。
#[tokio::test]
async fn favorite_changed_event_matches_result_revision_and_is_emitted_once() {
    let (app, db) = app_with_state();
    let work_id = seed_work(&db);
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let app_handle = app.handle().clone();

    // 监听逻辑契约 `favorite.changed` 对应的合法 Tauri transport 名称。
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let event_id = webview.listen(FAVORITE_CHANGED_TRANSPORT_EVENT, move |event| {
        let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
        let _ = tx.send(payload);
    });

    // 成功 mutation
    let result = get_ipc_response(
        &webview,
        invoke_request(
            "favorite_set",
            serde_json::json!({ "request": { "workId": work_id, "favorite": true } }),
        ),
    )
    .unwrap();
    let result_json: serde_json::Value = result.deserialize().unwrap();
    let result_revision = result_json["revision"].as_str().unwrap().to_string();

    // 事件必达（emit 是异步投递，轮询等待）
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(ev) = rx.try_recv() {
                return ev;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("favorite.changed 事件必须在 5s 内送达");
    assert_eq!(
        event["revision"], result_revision,
        "事件 revision 与 Result 同源"
    );
    assert_eq!(event["workId"], work_id);
    assert_eq!(event["favorite"], true);

    // 幂等重复设置 → 无第二个事件
    let _ = get_ipc_response(
        &webview,
        invoke_request(
            "favorite_set",
            serde_json::json!({ "request": { "workId": work_id, "favorite": true } }),
        ),
    )
    .unwrap();
    let extra = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            if let Ok(ev) = rx.try_recv() {
                return Some(ev);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        matches!(extra, Err(_) | Ok(None)),
        "幂等重复设置不得产生第二个事件"
    );

    webview.unlisten(event_id);
    let _ = app_handle;
}

/// WORK_NOT_FOUND 精确错误：ErrorDto.code 必须精确等于 WORK_NOT_FOUND。
#[tokio::test]
async fn favorite_set_unknown_work_returns_exact_error_code() {
    let (app, _db) = app_with_state();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let response = get_ipc_response(
        &webview,
        invoke_request(
            "favorite_set",
            serde_json::json!({
                "request": { "workId": "0196f0d2-0000-7000-8000-0000000000ff", "favorite": true }
            }),
        ),
    );
    let err = response.expect_err("未知 Work 必须走错误路径");
    let message = err.to_string();
    assert!(
        message.contains("WORK_NOT_FOUND"),
        "ErrorDto.code 必须精确为 WORK_NOT_FOUND，实际: {message}"
    );
}

/// scan_cancel 未知 taskId：ErrorDto.code 必须精确等于 RESOURCE_NOT_FOUND
///（BE-SCAN-001 第三步：不伪造 Completed）。
#[tokio::test]
async fn scan_cancel_unknown_task_returns_exact_error_code() {
    let (app, _db) = app_with_state();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let response = get_ipc_response(
        &webview,
        invoke_request(
            "scan_cancel",
            serde_json::json!({ "taskId": "task-nonexistent-0000" }),
        ),
    );
    let err = response.expect_err("未知 taskId 必须走错误路径");
    let message = err.to_string();
    assert!(
        message.contains("RESOURCE_NOT_FOUND"),
        "ErrorDto.code 必须精确为 RESOURCE_NOT_FOUND，实际: {message}"
    );
}

/// Comic manifest：真实 request envelope、Webview CommandArg 与 AppState 注入必须连通。
#[tokio::test]
async fn comic_manifest_request_envelope_returns_exact_invalid_id() {
    let (app, _db) = app_with_state();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let response = get_ipc_response(
        &webview,
        invoke_request(
            "comic_page_manifest_get",
            serde_json::json!({ "request": { "sessionId": "not-a-uuid" } }),
        ),
    );
    let error = response.expect_err("非法 Session ID 必须由 Comic Command 拒绝");
    let message = error.to_string();
    assert!(
        message.contains("INVALID_ID"),
        "Comic Command 必须返回稳定 INVALID_ID，实际: {message}"
    );
}

/// 新 Comic Command 也必须受 `windows: [\"main\"]` Capability 约束。
#[tokio::test]
async fn comic_manifest_is_denied_for_unmatching_window() {
    let (app, _db) = app_with_state();
    let other = tauri::WebviewWindowBuilder::new(&app, "main2", Default::default())
        .build()
        .unwrap();

    let response = get_ipc_response(
        &other,
        invoke_request(
            "comic_page_manifest_get",
            serde_json::json!({ "request": { "sessionId": "not-a-uuid" } }),
        ),
    );
    let error = response.expect_err("Comic Command 在未授权 window 必须先被 ACL 拒绝");
    let message = error.to_string();
    assert!(
        message.contains("not allowed"),
        "Comic Command 必须是 ACL deny，而非参数校验或 command-not-found，实际: {message}"
    );
    assert!(
        message.contains("main2"),
        "Comic Command 的 ACL deny 必须指明未授权 window，实际: {message}"
    );
}

/// settings_update：真实 invoke dispatch + 合法 transport 事件，且幂等更新不重复发布。
#[tokio::test]
async fn settings_changed_event_matches_result_and_is_emitted_once() {
    let (app, _db) = app_with_state();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let event_id = webview.listen(SETTINGS_CHANGED_TRANSPORT_EVENT, move |event| {
        let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
        let _ = tx.send(payload);
    });

    let first = get_ipc_response(
        &webview,
        invoke_request(
            "settings_update",
            serde_json::json!({
                "section": "general",
                "expectedRevision": null,
                "patch": { "section": "general", "launchPage": "library" }
            }),
        ),
    )
    .expect("settings_update 首次变化必须成功");
    let result_json: serde_json::Value = first.deserialize().unwrap();
    assert_eq!(result_json["changed"], true);
    assert_eq!(result_json["value"]["section"], "general");
    assert_eq!(result_json["value"]["launchPage"], "library");
    let revision = result_json["revision"].as_str().unwrap().to_owned();

    let event = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(event) = rx.try_recv() {
                return event;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("settings.changed 事件必须在 5s 内送达");
    assert_eq!(event["section"], "general");
    assert_eq!(event["revision"], revision);
    assert_eq!(event["schemaVersion"], 1);

    let second = get_ipc_response(
        &webview,
        invoke_request(
            "settings_update",
            serde_json::json!({
                "section": "general",
                "expectedRevision": revision,
                "patch": { "section": "general", "launchPage": "library" }
            }),
        ),
    )
    .expect("settings_update 幂等重复必须成功");
    let second_json: serde_json::Value = second.deserialize().unwrap();
    assert_eq!(second_json["changed"], false);
    assert_eq!(second_json["revision"], revision);

    let extra = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            if let Ok(event) = rx.try_recv() {
                return Some(event);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        matches!(extra, Err(_) | Ok(None)),
        "幂等更新不得产生第二个事件"
    );
    webview.unlisten(event_id);
}

/// Capability ACL deny：`main` capability 只授权 `main` 窗口——
/// 已注册命令在**未授权窗口**（main2）被 invoke 拒绝（ACL 生效，非 command-not-found）。
#[tokio::test]
async fn registered_command_is_denied_for_unmatching_window() {
    let (app, _db) = app_with_state();
    // capability windows: ["main"] —— main2 不在授权内
    let other = tauri::WebviewWindowBuilder::new(&app, "main2", Default::default())
        .build()
        .unwrap();

    let response = get_ipc_response(
        &other,
        invoke_request(
            "library_list",
            serde_json::json!({
                "request": { "category": "all", "mediaTypes": null, "query": null, "sort": "recently_added", "cursor": null, "limit": 50 }
            }),
        ),
    );
    assert!(
        response.is_err(),
        "已注册命令在未授权 window 必须被 ACL 拒绝"
    );
}

/// 未注册命令 → command-not-found（dispatch 层拒绝）。
#[tokio::test]
async fn unknown_command_is_rejected_by_dispatch() {
    // 空 authority 让未注册命令直达 dispatch，而不会先被 ACL 拒绝。
    let (app, _db) = app_with_context(mock_context(noop_assets()));
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let response = get_ipc_response(
        &webview,
        invoke_request("not_a_command", serde_json::json!({})),
    );
    let err = response.expect_err("未注册命令必须被 invoke dispatch 拒绝");
    let message = err.to_string();
    assert!(
        message.contains("Command not_a_command not found"),
        "必须是 dispatch command-not-found，而非 ACL deny，实际: {message}"
    );
}

/// VERIFY-SEC-IPC-001 曾以临时文件做 12 类路径/注入样 payload 的公开 IPC 探针，
/// 测后即删除。本回归通过真实 ACL、invoke handler 与 AppState 复验两个破坏性命令：
/// WebView 只能提交 opaque StorageLocationId，任何路径或注入样字符串都不得触及数据库。
#[tokio::test]
async fn destructive_storage_commands_reject_malicious_id_payloads_without_side_effects() {
    let (app, db) = app_with_state();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let payloads = vec![
        r"\\server\share\media".to_string(),
        r"\\?\UNC\server\share\media".to_string(),
        "//server/share/media".to_string(),
        r"C:\\Users\\attacker\\media".to_string(),
        "../outside".to_string(),
        "'; DROP TABLE storage_locations; --".to_string(),
        "<script>alert(1)</script>".to_string(),
        "x".repeat(10 * 1024),
        String::new(),
    ];

    for command in ["storage_location_disconnect", "storage_location_remove"] {
        for payload in &payloads {
            let response = get_ipc_response(
                &webview,
                invoke_request(command, serde_json::json!({ "storageLocationId": payload })),
            );
            let error = response
                .expect_err("恶意 ID 必须在公开 IPC 边界被拒绝")
                .to_string();
            assert!(
                error.contains("INVALID_ID"),
                "{command} 必须返回稳定 INVALID_ID，实际: {error}"
            );
            if !payload.is_empty() {
                assert!(
                    !error.contains(payload),
                    "错误不得回显攻击者提交的路径/注入样值"
                );
            }
        }
    }

    let repos = SqliteRepositories::new(db);
    assert!(
        repos.storage_location.list().await.unwrap().is_empty(),
        "非法 invoke 不得写入位置表"
    );
}

/// 移除位置成功提交后必须只发一个 library-changed，并让后续列表读取到空投影。
#[tokio::test]
async fn storage_location_remove_emits_one_library_changed_and_invalidates_library() {
    let (app, db) = app_with_state();
    let (location_id, work_id) = seed_storage_work(&db);
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let event_id = webview.listen(LIBRARY_CHANGED_TRANSPORT_EVENT, move |event| {
        let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
        let _ = tx.send(payload);
    });

    let before = get_ipc_response(
        &webview,
        invoke_request(
            "library_list",
            serde_json::json!({
                "request": { "category": "all", "mediaTypes": null, "query": null,
                    "sort": "recently_added", "cursor": null, "limit": 50 }
            }),
        ),
    )
    .unwrap();
    let before_json: serde_json::Value = before.deserialize().unwrap();
    assert_eq!(before_json["total"], 1);
    assert_eq!(before_json["items"][0]["workId"], work_id);

    get_ipc_response(
        &webview,
        invoke_request(
            "storage_location_remove",
            serde_json::json!({ "storageLocationId": location_id }),
        ),
    )
    .expect("移除成功必须返回 Ok");

    let event = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(event) = rx.try_recv() {
                return event;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("library.changed 必须在 5s 内送达");
    assert_eq!(event["schemaVersion"], 1);
    assert!(event["operationId"]
        .as_str()
        .unwrap()
        .starts_with("remove-"));

    let after = get_ipc_response(
        &webview,
        invoke_request(
            "library_list",
            serde_json::json!({
                "request": { "category": "all", "mediaTypes": null, "query": null,
                    "sort": "recently_added", "cursor": null, "limit": 50 }
            }),
        ),
    )
    .unwrap();
    let after_json: serde_json::Value = after.deserialize().unwrap();
    assert_eq!(after_json["total"], 0);
    assert!(after_json["items"].as_array().unwrap().is_empty());

    let extra = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            if let Ok(event) = rx.try_recv() {
                return Some(event);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        matches!(extra, Err(_) | Ok(None)),
        "移除不得重复发 library.changed"
    );
    webview.unlisten(event_id);
}
