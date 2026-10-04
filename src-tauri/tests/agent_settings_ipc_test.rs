//! Agent 全局设置 Typed IPC 端到端测试（A1 垂直切片）。
//!
//! 覆盖真实链路：Tauri 参数反序列化（envelope `{"request": {...}}`）→ State 注入 →
//! invoke handler dispatch → 真实 SQLite（migrations 042/043/044）→ Application
//! service → 设置 CAS 写入与回执。
//!
//! 断言重点是**安全边界**，而不是"能跑通"：
//! - 批准只提交 proposal id + digest，wire 里没有任何 token 材料；
//! - stale context / digest 不匹配 / replay 都零设置写入；
//! - 脱敏快照不把绝对路径或凭据送回 WebView；
//! - ACL 只授权 main 窗口（未授权窗口被拒）。

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
use tauri::{Context, Manager, Url};

use haven_application::services::AgentEventKind;
use haven_domain::ids::AgentSessionId;
use haven_infrastructure::Db;
use haven_tauri_lib::state::AppState;

fn invoke_request(cmd: &str, body: serde_json::Value) -> InvokeRequest {
    InvokeRequest {
        cmd: cmd.into(),
        callback: CallbackFn(1),
        error: CallbackFn(2),
        // Windows/Android 使用本地 tauri origin；`tauri://` 在这些平台会被判为 remote。
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

/// 读取当前 authoritative 阅读设置数据行（直接查库，证明写入真的发生了）。
fn reading_row(db: &Db) -> Option<String> {
    let mut row = None;
    db.with_tx(|tx| {
        row = tx
            .query_row(
                "SELECT data_json FROM settings WHERE section = 'reading'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok();
        Ok(())
    })
    .unwrap();
    row
}

fn reading_revision(db: &Db) -> Option<String> {
    let mut revision = None;
    db.with_tx(|tx| {
        revision = tx
            .query_row(
                "SELECT revision FROM settings WHERE section = 'reading'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok();
        Ok(())
    })
    .unwrap();
    revision
}

/// 直接写一条 reading 设置行（模拟"用户已经在设置页保存过"）。
fn seed_reading(db: &Db, revision: &str, data_json: &str) {
    let now = haven_common::UtcMillis::now().0;
    db.with_tx(|tx| {
        tx.execute(
            "INSERT INTO settings (section, schema_version, revision, data_json, updated_at)
             VALUES ('reading', 1, ?1, ?2, ?3)
             ON CONFLICT(section) DO UPDATE SET
                revision = excluded.revision,
                data_json = excluded.data_json,
                updated_at = excluded.updated_at",
            rusqlite::params![revision, data_json, now],
        )
        .map_err(|error| {
            haven_common::AppError::new(
                "DATABASE_ERROR",
                haven_common::ErrorKind::Database,
                "seed reading 失败",
                false,
            )
            .with_source(error)
        })?;
        Ok(())
    })
    .expect("seed reading");
}

/// 读取上下文（成功路径），返回解析后的 DTO。
fn context_get(webview: &tauri::WebviewWindow<MockRuntime>) -> serde_json::Value {
    let response = get_ipc_response(
        webview,
        invoke_request("agent_settings_context_get", serde_json::json!({})),
    )
    .expect("agent_settings_context_get 必须在 main 窗口被允许");
    response.deserialize().unwrap()
}

fn create_proposal(
    webview: &tauri::WebviewWindow<MockRuntime>,
    context: &serde_json::Value,
    patch: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({
        "request": {
            "sessionId": "0196f0d2-0000-7000-8000-0000000000f1",
            "requestId": "0196f0d2-0000-7000-8000-0000000000f2",
            "contextId": context["contextId"],
            "contextHash": context["contextHash"],
            "baseRevision": context["revision"],
            "patch": patch,
        }
    });
    match get_ipc_response(
        webview,
        invoke_request("agent_settings_proposal_create", body),
    ) {
        Ok(response) => Ok(response.deserialize().unwrap()),
        Err(error) => Err(error.to_string()),
    }
}

#[test]
fn capability_manifest_command_reports_closed_capabilities() {
    let (app, _db) = app_with_context(real_acl_context());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let response = get_ipc_response(
        &webview,
        invoke_request("agent_capability_manifest_get", serde_json::json!({})),
    )
    .expect("能力清单命令必须在 main 窗口被允许");
    let json: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(json["agentApiVersion"], 1);
    assert_eq!(json["capabilities"]["settingsRead"], true);
    assert_eq!(json["capabilities"]["settingsProposal"], true);
    // 当前切片已经实现的读取/提案能力必须公开为 true；清单不是授权开关。
    for capability in [
        "librarySummaryRead",
        "settingSourcesRead",
        "resourcePreferenceRead",
        "resourcePreferenceProposal",
        "mediaCapabilitiesRead",
        "onboardingRead",
    ] {
        assert_eq!(
            json["capabilities"][capability], true,
            "{capability} 必须与当前已实现切片一致"
        );
    }
    // 高风险或尚未实现能力一律 false。
    for capability in [
        "metadataProposal",
        "renameProposal",
        "secretRead",
        "filesystemWrite",
    ] {
        assert_eq!(
            json["capabilities"][capability], false,
            "{capability} 必须保持关闭"
        );
    }
}

#[test]
fn context_get_returns_redacted_snapshot_without_paths_or_secrets() {
    let (app, db) = app_with_context(real_acl_context());
    // 自由文本里塞入绝对路径与凭据形态：服务端必须清空并登记。
    seed_reading(
        &db,
        "rev-ctx-1",
        r##"{"section":"reading","fontFamily":"serif","customFontFamily":"C:/Windows/Fonts/msyh.ttc","fontSize":"medium","lineHeight":"comfortable","contentWidth":"medium","theme":"warm","customBackground":"#f7f1e3","customText":null,"fontWeight":"regular","letterSpacing":"normal","systemAuto":true,"pagination":"scroll"}"##,
    );
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let context = context_get(&webview);
    assert_eq!(context["schemaVersion"], 1);
    assert_eq!(context["subject"]["section"], "reading");
    assert_eq!(context["revision"], "rev-ctx-1");
    assert_eq!(
        context["reading"]["customFontFamily"],
        serde_json::Value::Null
    );
    assert_eq!(context["reading"]["customBackground"], "#f7f1e3");
    assert_eq!(
        context["reading"]["redactedFields"],
        serde_json::json!(["custom_font_family"])
    );
    // contextId 是 contextHash 的派生身份（形状可核验）。
    let hash = context["contextHash"].as_str().unwrap();
    assert_eq!(hash.len(), 64);
    let id = context["contextId"].as_str().unwrap();
    assert_eq!(id.len(), 36);
    // 绝对路径与凭据关键词不得出现在响应里。
    let raw = serde_json::to_string(&context).unwrap();
    for forbidden in ["C:/Windows", "msyh.ttc", "api_key", "token", "password"] {
        assert!(!raw.contains(forbidden), "上下文响应不得包含 {forbidden}");
    }
}

#[test]
fn proposal_creation_and_approval_write_settings_exactly_once() {
    let (app, db) = app_with_context(real_acl_context());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let context = context_get(&webview);
    // 从未保存过 → revision 为 null。
    assert_eq!(context["revision"], serde_json::Value::Null);
    let proposal = create_proposal(
        &webview,
        &context,
        serde_json::json!({ "fontSize": "large", "lineHeight": "airy" }),
    )
    .expect("提案必须能创建");
    assert_eq!(proposal["status"], "pending");
    assert_eq!(proposal["baseRevision"], serde_json::Value::Null);
    let digest = proposal["digest"].as_str().unwrap().to_owned();
    assert_eq!(digest.len(), 64);
    assert!(digest
        .chars()
        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
    let proposal_id = proposal["proposalId"].as_str().unwrap().to_owned();
    assert_eq!(proposal["changes"].as_array().unwrap().len(), 2);
    // 创建提案不写设置。
    assert!(reading_row(&db).is_none());

    // 批准：只提交 proposal id + UI 显示的那份 digest。
    let response = get_ipc_response(
        &webview,
        invoke_request(
            "agent_settings_proposal_approve",
            serde_json::json!({
                "request": { "proposalId": proposal_id, "expectedDigest": digest }
            }),
        ),
    )
    .expect("批准必须被允许");
    let result: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(result["proposal"]["status"], "applied");
    assert_eq!(result["receipt"]["status"], "applied");
    assert_eq!(result["receipt"]["changed"], true);
    assert_eq!(result["receipt"]["proposalDigest"], digest);
    assert!(result["receipt"]["appliedRevision"].as_str().is_some());
    // 真实写入：设置行出现且档位就是提案里的值。
    let stored = reading_row(&db).expect("批准后必须写出阅读设置行");
    assert!(stored.contains("\"large\""), "store={stored}");
    assert!(stored.contains("\"airy\""), "store={stored}");
    // 回执/响应里没有 token 材料。
    let raw = serde_json::to_string(&result).unwrap();
    for forbidden in ["token", "Token", "approvalToken"] {
        assert!(!raw.contains(forbidden), "批准响应不得包含 {forbidden}");
    }

    // Trace 只是旁路事实，但生产 AppState 必须把创建/批准阶段接到同一个 collector，
    // 且 CAS / Apply / Receipt 的顺序不能被 UI 看到成错序。
    let session_id = "0196f0d2-0000-7000-8000-0000000000f1"
        .parse::<AgentSessionId>()
        .unwrap();
    let trace = app.state::<AppState>().agent_trace.snapshot(session_id);
    let kinds: Vec<_> = trace.iter().map(|event| event.kind).collect();
    assert_eq!(
        kinds,
        vec![
            AgentEventKind::RequestStarted,
            AgentEventKind::ContextLoaded,
            AgentEventKind::ProposalCreated,
            AgentEventKind::WaitingForApproval,
            AgentEventKind::CasStarted,
            AgentEventKind::Applied,
            AgentEventKind::ReceiptCreated,
        ]
    );
    assert!(trace
        .windows(2)
        .all(|events| events[0].sequence < events[1].sequence));

    let trace_response = get_ipc_response(
        &webview,
        invoke_request(
            "agent_trace_get",
            serde_json::json!({
                "request": { "sessionId": session_id.to_string() }
            }),
        ),
    )
    .expect("agent_trace_get 必须返回同一份有界轨迹");
    let trace_json: serde_json::Value = trace_response.deserialize().unwrap();
    assert_eq!(trace_json["schemaVersion"], 1);
    assert_eq!(trace_json["events"].as_array().unwrap().len(), 7);
    let trace_raw = serde_json::to_string(&trace_json).unwrap();
    for forbidden in ["apiKey", "rawResponse", "absolutePath", "sql", "secret"] {
        assert!(
            !trace_raw.contains(forbidden),
            "轨迹响应不得包含 {forbidden}"
        );
    }

    // 回执读取命令与批准结果同源。
    let response = get_ipc_response(
        &webview,
        invoke_request(
            "agent_setting_change_receipt_get",
            serde_json::json!({ "request": { "proposalId": result["proposal"]["proposalId"] } }),
        ),
    )
    .expect("回执读取必须被允许");
    let receipt: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(receipt["proposalDigest"], digest);
}

#[test]
fn stale_context_and_digest_mismatch_write_nothing() {
    let (app, db) = app_with_context(real_acl_context());
    seed_reading(
        &db,
        "rev-stale-1",
        r##"{"section":"reading","fontFamily":"serif","customFontFamily":null,"fontSize":"medium","lineHeight":"comfortable","contentWidth":"medium","theme":"warm","customBackground":null,"customText":null,"fontWeight":"regular","letterSpacing":"normal","systemAuto":true,"pagination":"scroll"}"##,
    );
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let context = context_get(&webview);
    let revision_before = reading_revision(&db);
    let row_before = reading_row(&db);

    // 用一份**过期**上下文的 hash 建提案：fail-closed。
    let mut forged = context.clone();
    forged["contextHash"] = serde_json::json!("f".repeat(64));
    assert!(create_proposal(
        &webview,
        &forged,
        serde_json::json!({ "fontSize": "large" })
    )
    .is_err());

    // 用错误 revision 建提案：fail-closed。
    let mut wrong_revision = context.clone();
    wrong_revision["revision"] = serde_json::json!("rev-does-not-exist");
    assert!(create_proposal(
        &webview,
        &wrong_revision,
        serde_json::json!({ "fontSize": "large" })
    )
    .is_err());

    // 合法提案创建后，用一个不匹配的 digest 批准：零写入。
    let proposal = create_proposal(
        &webview,
        &context,
        serde_json::json!({ "fontSize": "large" }),
    )
    .expect("合法上下文必须能创建提案");
    let error = get_ipc_response(
        &webview,
        invoke_request(
            "agent_settings_proposal_approve",
            serde_json::json!({
                "request": {
                    "proposalId": proposal["proposalId"],
                    "expectedDigest": "0".repeat(64),
                }
            }),
        ),
    )
    .expect_err("digest 不匹配必须被拒绝");
    assert!(
        error
            .to_string()
            .contains("SETTING_PROPOSAL_DIGEST_MISMATCH"),
        "错误必须是稳定摘要不匹配码：{error}"
    );

    // 全程零写入：设置行与 revision 都保持原样。
    assert_eq!(reading_revision(&db), revision_before);
    assert_eq!(reading_row(&db), row_before);

    // 批准一次后再用同一份 digest 重放：拒绝且不产生第二次写入。
    let digest = proposal["digest"].as_str().unwrap().to_owned();
    let approve_body = serde_json::json!({
        "request": { "proposalId": proposal["proposalId"], "expectedDigest": digest }
    });
    get_ipc_response(
        &webview,
        invoke_request("agent_settings_proposal_approve", approve_body.clone()),
    )
    .expect("首次批准必须成功");
    let after_first = reading_row(&db);
    assert!(after_first.is_some());
    assert!(get_ipc_response(
        &webview,
        invoke_request("agent_settings_proposal_approve", approve_body)
    )
    .is_err());
    assert_eq!(reading_row(&db), after_first, "重放不得再次写设置");
}

#[test]
fn reject_writes_nothing_and_is_digest_checked() {
    let (app, db) = app_with_context(real_acl_context());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let context = context_get(&webview);
    let proposal = create_proposal(
        &webview,
        &context,
        serde_json::json!({ "fontSize": "large" }),
    )
    .expect("提案必须能创建");
    let proposal_id = proposal["proposalId"].as_str().unwrap().to_owned();
    let digest = proposal["digest"].as_str().unwrap().to_owned();

    // 错误 digest：不能把拒绝写成事实。
    assert!(get_ipc_response(
        &webview,
        invoke_request(
            "agent_settings_proposal_reject",
            serde_json::json!({
                "request": { "proposalId": proposal_id, "expectedDigest": "1".repeat(64) }
            }),
        )
    )
    .is_err());

    let response = get_ipc_response(
        &webview,
        invoke_request(
            "agent_settings_proposal_reject",
            serde_json::json!({
                "request": { "proposalId": proposal_id, "expectedDigest": digest }
            }),
        ),
    )
    .expect("拒绝必须被允许");
    let rejected: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(rejected["proposal"]["status"], "rejected");
    // 零写入：阅读分区从未被保存过。
    assert!(reading_row(&db).is_none());

    // 提案回读命令可用，且未应用时回执为 null。
    let response = get_ipc_response(
        &webview,
        invoke_request(
            "agent_settings_proposal_get",
            serde_json::json!({ "request": { "proposalId": rejected["proposal"]["proposalId"] } }),
        ),
    )
    .expect("提案回读必须被允许");
    let reread: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(reread["proposal"]["status"], "rejected");
    assert_eq!(reread["receipt"], serde_json::Value::Null);
}

#[test]
fn malformed_ids_return_invalid_argument_without_side_effects() {
    let (app, _db) = app_with_context(real_acl_context());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    for (command, body) in [
        (
            "agent_settings_proposal_get",
            serde_json::json!({ "request": { "proposalId": "not-a-uuid" } }),
        ),
        (
            "agent_setting_change_receipt_get",
            serde_json::json!({ "request": { "proposalId": "" } }),
        ),
        (
            "agent_settings_proposal_approve",
            serde_json::json!({
                "request": { "proposalId": "not-a-uuid", "expectedDigest": "a".repeat(64) }
            }),
        ),
    ] {
        let error = get_ipc_response(&webview, invoke_request(command, body))
            .expect_err("非法 ID 必须被拒绝");
        assert!(
            error.to_string().contains("INVALID_ARGUMENT"),
            "{command} 必须返回稳定 INVALID_ARGUMENT：{error}"
        );
    }

    // 未知字段（例如把 token 塞进请求）必须在反序列化边界被拒绝。
    let error = get_ipc_response(
        &webview,
        invoke_request(
            "agent_settings_proposal_approve",
            serde_json::json!({
                "request": {
                    "proposalId": "0196f0d2-0000-7000-8000-0000000000f1",
                    "expectedDigest": "a".repeat(64),
                    "approvalToken": "b".repeat(64),
                }
            }),
        ),
    )
    .expect_err("未知字段必须被拒绝");
    let text = error.to_string();
    assert!(
        text.contains("approvalToken")
            || text.contains("invalid args")
            || text.contains("unknown field"),
        "请求载荷必须拒绝未知字段：{text}"
    );
}

#[test]
fn agent_commands_are_denied_for_unmatching_window() {
    let (app, _db) = app_with_context(real_acl_context());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main2", Default::default())
        .build()
        .unwrap();

    for command in [
        "agent_capability_manifest_get",
        "agent_settings_context_get",
        "agent_settings_proposal_create",
        "agent_settings_proposal_approve",
        "agent_settings_proposal_reject",
        "agent_settings_proposal_get",
        "agent_setting_change_receipt_get",
    ] {
        let error = get_ipc_response(&webview, invoke_request(command, serde_json::json!({})))
            .expect_err("未授权窗口必须被 ACL 拒绝");
        let text = error.to_string();
        assert!(
            text.contains("not allowed"),
            "{command} 必须被 ACL 拒绝：{text}"
        );
        assert!(
            text.contains("main2"),
            "{command} 拒绝信息必须指明窗口：{text}"
        );
    }
}
