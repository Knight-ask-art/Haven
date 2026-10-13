//! AI Provider Profile Typed IPC 端到端测试（A2 基础切片）。
//!
//! 覆盖真实链路：Tauri 参数反序列化（envelope `{"request": {...}}`）→ State 注入 →
//! invoke handler dispatch → 真实 SQLite（migrations 045）→ Application service →
//! profile CAS 写入与凭据清理编排。
//!
//! 断言重点是**安全边界**，而不是"能跑通"：
//! - profile 校验失败（私网端点 / 本地路径 / 非法 id）零写入；
//! - 响应里没有 API key、凭据 target 名或 credentialRef；
//! - 没有密钥时不发起模型发现请求，而是返回诚实的空目录 + `no_credential`；
//! - 停用的 profile 同样不发请求；
//! - stale revision 冲突且不覆盖；
//! - ACL 只授权 main 窗口。
//!
//! 默认测试不会调用 `credential_set`。单独 opt-in 的 ignored live test 使用随机隔离
//! profile、内存数据库与真实 Windows 凭据管理器；结束/展开栈时删除测试凭据。
//!
//! **平台差异**：非 Windows 平台按 ADR-001 装配 `UnsupportedCredentialStore`，
//! profile 行写入后读取凭据存在性必然失败。因此 `profile_lifecycle_*` 在非 Windows
//! 上断言稳定的 `CREDENTIAL_UNSUPPORTED`（不可重试、不回显端点）后提前返回；
//! 完整的 CAS / 空目录 / 停用 / 删除生命周期只在 Windows（真实 Credential Manager）
//! 上执行。测试不会为跑通而替换成伪造的凭据后端。

mod support;
use support::{invoke_request, real_acl_context};

use std::sync::Arc;

use tauri::test::{get_ipc_response, mock_builder, MockRuntime};

#[cfg(windows)]
use tauri::Manager;

use haven_infrastructure::Db;
use haven_tauri_lib::state::AppState;

fn app() -> tauri::App<MockRuntime> {
    let db = Arc::new(Db::open_in_memory().unwrap());
    haven_tauri_lib::register_invoke_handler(mock_builder())
        .manage(AppState::new(db))
        .build(real_acl_context())
        .unwrap()
}

/// Live verification is deliberately not a CI dependency or a provider fixture.
/// Input credentials come only from the test process, never source or arguments.
#[cfg(windows)]
#[test]
#[ignore = "requires explicit local provider configuration and transient credentials"]
fn live_local_provider_recommendation_approval_and_receipt() {
    use haven_domain::credential::SecretString;
    use haven_domain::ids::{AgentRequestId, AgentSessionId};

    fn checked_ipc(
        webview: &tauri::WebviewWindow<MockRuntime>,
        command: &str,
        body: serde_json::Value,
        secret: &SecretString,
    ) -> serde_json::Value {
        let response =
            get_ipc_response(webview, invoke_request(command, body)).unwrap_or_else(|error| {
                // Do not print arbitrary error payloads or echo server content.
                let code = error
                    .get("code")
                    .and_then(serde_json::Value::as_str)
                    .filter(|code| {
                        code.len() <= 100
                            && code
                                .bytes()
                                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                    })
                    .unwrap_or("LIVE_IPC_ERROR");
                panic!("live stage {command} failed: {code}");
            });
        let value: serde_json::Value = response.deserialize().expect("typed JSON response");
        if value.to_string().contains(secret.expose()) {
            panic!("live IPC response leaked credential material");
        }
        value
    }

    struct CredentialCleanup<'a> {
        webview: &'a tauri::WebviewWindow<MockRuntime>,
        profile_id: String,
    }
    impl Drop for CredentialCleanup<'_> {
        fn drop(&mut self) {
            let deleted = get_ipc_response(
                self.webview,
                invoke_request(
                    "credential_delete",
                    serde_json::json!({"request": {"provider": "ai", "profileId": self.profile_id}}),
                ),
            );
            if deleted.is_err() {
                eprintln!("LIVE_TEST_CREDENTIAL_CLEANUP_FAILED");
            }
        }
    }

    let endpoint = std::env::var("HAVEN_AGENT_LIVE_ENDPOINT").expect("explicit endpoint required");
    let safe = haven_common::network::parse_http_url(
        &endpoint,
        haven_common::network::HttpUrlPolicy::AiProviderEndpoint,
    )
    .expect("valid local endpoint required");
    assert!(
        safe.host()
            .parse()
            .is_ok_and(|ip| haven_common::network::ai_provider_loopback_allowed(safe.host(), ip)),
        "this opt-in test only permits local literal endpoints"
    );
    let models: Vec<String> = serde_json::from_str(
        &std::env::var("HAVEN_AGENT_LIVE_MODELS").expect("explicit models required"),
    )
    .expect("model array required");
    assert!(
        !models.is_empty() && models.len() <= 2,
        "bounded live verification"
    );
    let secret = SecretString::new(
        std::env::var("HAVEN_AGENT_LIVE_KEY").expect("transient credential required"),
    );
    assert!(!secret.expose().is_empty(), "nonempty credential required");

    for model_id in models {
        haven_domain::ai_provider::validate_model_id(&model_id).expect("valid opaque model id");
        let db = Arc::new(Db::open_in_memory().unwrap());
        let app = haven_tauri_lib::register_invoke_handler(mock_builder())
            .manage(AppState::new(db.clone()))
            .build(real_acl_context())
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let profile_id = format!("live-agent-{}", uuid::Uuid::new_v4());
        let mut body = upsert_body(&profile_id, &endpoint, true, None);
        body["request"]["selectedModelId"] = serde_json::json!(model_id);
        checked_ipc(&webview, "ai_provider_profile_upsert", body, &secret);
        let cleanup = CredentialCleanup {
            webview: &webview,
            profile_id: profile_id.clone(),
        };
        checked_ipc(
            &webview,
            "credential_set",
            serde_json::json!({"request": {
                "provider": "ai", "profileId": profile_id, "secret": secret.expose(),
            }}),
            &secret,
        );

        let catalog = checked_ipc(
            &webview,
            "ai_provider_models_list",
            serde_json::json!({"request": {"profileId": profile_id}}),
            &secret,
        );
        assert_eq!(catalog["state"], "ready");
        let model = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["modelId"] == model_id)
            .expect("explicit model must remain in catalog");
        assert_ne!(model["chat"], "unsupported");

        let skill = checked_ipc(
            &webview,
            "agent_skill_set_enabled",
            serde_json::json!({"request": {
                "skillId": "haven-agent-proposal", "enabled": true,
            }}),
            &secret,
        );
        assert_eq!(skill["state"], "enabled");
        let context = checked_ipc(
            &webview,
            "agent_settings_context_get",
            serde_json::json!({}),
            &secret,
        );
        let session_id = AgentSessionId::new();
        let started = std::time::Instant::now();
        let result = checked_ipc(
            &webview,
            "ai_settings_recommendation_generate",
            serde_json::json!({"request": {
                "profileId": profile_id, "sessionId": session_id.to_string(), "requestId": AgentRequestId::new().to_string(),
                "userIntent": "将全局阅读字号设置为 large、行高设置为 airy。只修改这两个字段，其余字段保持 null。",
                "contextId": context["contextId"], "contextHash": context["contextHash"], "baseRevision": context["revision"],
            }}),
            &secret,
        );
        let latency_ms = started.elapsed().as_millis();
        assert_eq!(result["modelId"], model_id);
        assert_eq!(result["proposal"]["status"], "pending");
        assert_eq!(result["recommendedPatch"]["fontSize"], "large");
        assert_eq!(result["recommendedPatch"]["lineHeight"], "airy");
        let after_pending = checked_ipc(
            &webview,
            "agent_settings_context_get",
            serde_json::json!({}),
            &secret,
        );
        assert_eq!(
            after_pending["revision"], context["revision"],
            "generating must not write settings"
        );
        assert_eq!(after_pending["reading"], context["reading"]);

        let approve_body = serde_json::json!({"request": {
            "proposalId": result["proposal"]["proposalId"], "expectedDigest": result["proposal"]["digest"],
        }});
        let approved = checked_ipc(
            &webview,
            "agent_settings_proposal_approve",
            approve_body.clone(),
            &secret,
        );
        assert_eq!(approved["proposal"]["status"], "applied");
        assert_eq!(approved["receipt"]["status"], "applied");
        assert_eq!(approved["receipt"]["changed"], true);
        let after_approve = checked_ipc(
            &webview,
            "agent_settings_context_get",
            serde_json::json!({}),
            &secret,
        );
        assert_eq!(
            after_approve["revision"],
            approved["receipt"]["appliedRevision"]
        );
        assert_eq!(after_approve["reading"]["fontSize"], "large");
        assert_eq!(after_approve["reading"]["lineHeight"], "airy");
        let stored: serde_json::Value = db
            .with_tx(|tx| {
                let raw: String = tx
                    .query_row(
                        "SELECT data_json FROM settings WHERE section = 'reading'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|_| {
                        haven_common::AppError::new(
                            "LIVE_DB_ERROR",
                            haven_common::ErrorKind::Database,
                            "live test read failed",
                            false,
                        )
                    })?;
                Ok(serde_json::from_str(&raw).unwrap())
            })
            .unwrap();
        assert_eq!(stored["fontSize"], "large");
        assert_eq!(stored["lineHeight"], "airy");
        let receipt = checked_ipc(
            &webview,
            "agent_setting_change_receipt_get",
            serde_json::json!({"request": {
                "proposalId": result["proposal"]["proposalId"],
            }}),
            &secret,
        );
        assert_eq!(receipt, approved["receipt"]);
        assert!(get_ipc_response(
            &webview,
            invoke_request("agent_settings_proposal_approve", approve_body)
        )
        .is_err());
        let after_replay = checked_ipc(
            &webview,
            "agent_settings_context_get",
            serde_json::json!({}),
            &secret,
        );
        assert_eq!(after_replay["revision"], after_approve["revision"]);
        let trace = app.state::<AppState>().agent_trace.snapshot(session_id);
        assert!(
            trace.len() >= 8,
            "real request and approval must produce trace events"
        );

        checked_ipc(
            &webview,
            "ai_provider_profile_delete",
            serde_json::json!({"request": {"profileId": profile_id, "expectedRevision": null}}),
            &secret,
        );
        let credential = checked_ipc(
            &webview,
            "credential_status",
            serde_json::json!({"request": {"provider": "ai", "profileId": profile_id}}),
            &secret,
        );
        assert_eq!(credential["configured"], false);
        drop(cleanup);
        println!(
            "{}",
            serde_json::json!({
                "model": model_id, "chatDeclaration": model["chat"], "latencyMs": latency_ms,
                "nativeSkillEnabled": true, "pendingOnly": true, "approvalApplied": true,
                "sqliteReadback": true, "receiptReadback": true, "replayDenied": true, "credentialCleaned": true,
            })
        );
    }
}

fn upsert_body(
    profile_id: &str,
    endpoint: &str,
    enabled: bool,
    expected_revision: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "request": {
            "profileId": profile_id,
            "displayName": "自建网关",
            "kind": "openai_compatible",
            "endpoint": endpoint,
            "enabled": enabled,
            "selectedModelId": null,
            "expectedRevision": expected_revision,
        }
    })
}

/// 递归收集投影中的全部字符串，用于断言没有夹带凭据材料。
fn collect_strings(value: &serde_json::Value, collected: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => collected.push(text.clone()),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_strings(item, collected);
            }
        }
        serde_json::Value::Object(fields) => {
            for field in fields.values() {
                collect_strings(field, collected);
            }
        }
        _ => {}
    }
}

fn assert_no_credential_material(value: &serde_json::Value) {
    let mut strings = Vec::new();
    collect_strings(value, &mut strings);
    for text in &strings {
        for forbidden in [
            "haven:ai:",
            "haven:",
            "secret",
            "apiKey",
            "api_key",
            "Bearer",
        ] {
            assert!(
                !text.contains(forbidden),
                "AI Provider 响应不得包含 {forbidden}: {text}"
            );
        }
    }
}

#[test]
fn empty_state_lists_no_profiles_and_no_models() {
    let app = app();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let response = get_ipc_response(
        &webview,
        invoke_request("ai_provider_profile_list", serde_json::json!({})),
    )
    .expect("main 标签必须能够调用已授权的 ai_provider_profile_list");
    let json: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(json["schemaVersion"], 1);
    assert_eq!(json["profiles"].as_array().unwrap().len(), 0);
    assert_no_credential_material(&json);

    // 没有 profile 时模型目录请求返回稳定 NOT_FOUND，而不是伪造一个空目录。
    let response = get_ipc_response(
        &webview,
        invoke_request(
            "ai_provider_models_list",
            serde_json::json!({ "request": { "profileId": "gw" } }),
        ),
    );
    let message = response.expect_err("未知 profile 必须报错").to_string();
    assert!(
        message.contains("AI_PROVIDER_PROFILE_NOT_FOUND"),
        "错误码必须稳定，实际: {message}"
    );
    assert!(
        message.contains("\"retryable\":false"),
        "未知 profile 不可重试，实际: {message}"
    );
}

#[test]
fn invalid_endpoints_are_rejected_with_zero_writes() {
    let app = app();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    for endpoint in [
        "http://127.0.0.2:11434/v1",
        "http://127.1:3123/v1",
        "http://192.168.1.10/v1",
        "https://localhost/v1",
        "C:\\gateway",
        "file:///etc/passwd",
        "ftp://gateway.example.invalid",
        "https://gateway.example.invalid/v1?key=secret",
    ] {
        let response = get_ipc_response(
            &webview,
            invoke_request(
                "ai_provider_profile_upsert",
                upsert_body("gw", endpoint, true, None),
            ),
        );
        let message = response.expect_err("非法端点必须被拒绝").to_string();
        assert!(
            message.contains("AI_PROVIDER_ENDPOINT_INVALID"),
            "端点 {endpoint} 的错误码必须稳定，实际: {message}"
        );
        assert!(
            message.contains("\"retryable\":false"),
            "端点校验错误不可重试，实际: {message}"
        );
        assert!(
            !message.contains(endpoint),
            "错误消息不得回显被拒绝的端点，实际: {message}"
        );
    }

    let response = get_ipc_response(
        &webview,
        invoke_request("ai_provider_profile_list", serde_json::json!({})),
    )
    .unwrap();
    let json: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(
        json["profiles"].as_array().unwrap().len(),
        0,
        "全部非法输入必须零写入"
    );
}

#[cfg(windows)]
#[test]
fn local_endpoint_and_namespaced_model_round_trip_through_real_dispatch_and_sqlite() {
    let app = app();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let profile_id = format!("test-local-{}", uuid::Uuid::new_v4());
    for endpoint in ["http://127.0.0.1:3123/v1", "http://[::1]:11434/v1"] {
        let mut body = upsert_body(&profile_id, endpoint, true, None);
        body["request"]["selectedModelId"] = serde_json::json!("provider/reader-model");
        let response =
            get_ipc_response(&webview, invoke_request("ai_provider_profile_upsert", body)).unwrap();
        let created: serde_json::Value = response.deserialize().unwrap();
        assert_eq!(created["endpoint"], endpoint);
        assert_eq!(created["selectedModelId"], "provider/reader-model");
        assert_eq!(created["credentialConfigured"], false);
        let response = get_ipc_response(
            &webview,
            invoke_request(
                "ai_provider_profile_get",
                serde_json::json!({"request": {"profileId": profile_id}}),
            ),
        )
        .unwrap();
        let fetched: serde_json::Value = response.deserialize().unwrap();
        assert_eq!(created, fetched);
        assert_no_credential_material(&fetched);
        get_ipc_response(&webview, invoke_request("ai_provider_profile_delete",
            serde_json::json!({"request": {"profileId": profile_id, "expectedRevision": created["revision"]}}))).unwrap();
    }
}

/// 非 Windows 专属契约：仓库在该平台装配 `UnsupportedCredentialStore`（ADR-001
/// 验证项 3：不静默降级到文件后端），profile 行写入后读取凭据存在性必然失败。
/// 所以 upsert 必须**诚实失败**，断言稳定错误码、不可重试，且错误不回显端点。
#[cfg(not(windows))]
fn assert_credential_store_unsupported(webview: &tauri::WebviewWindow<MockRuntime>) {
    const ENDPOINT: &str = "https://gateway.example.invalid/v1/";
    let response = get_ipc_response(
        webview,
        invoke_request(
            "ai_provider_profile_upsert",
            upsert_body("gw-main", ENDPOINT, true, None),
        ),
    );
    let message = response
        .expect_err("非 Windows 平台没有系统凭据存储，upsert 不得伪造成功")
        .to_string();
    assert!(
        message.contains("CREDENTIAL_UNSUPPORTED"),
        "非 Windows 平台的错误码必须稳定，实际: {message}"
    );
    assert!(
        message.contains("\"retryable\":false"),
        "凭据存储不可用不可重试，实际: {message}"
    );
    assert!(
        !message.contains(ENDPOINT),
        "错误消息不得回显端点，实际: {message}"
    );
}

/// Windows 专属：真实 Windows Credential Manager 存在，因此可以跑完整生命周期
/// （CAS 写入 → 陈旧版本冲突 → 无凭据空目录 → 停用空目录 → 删除并如实上报凭据清理）。
#[cfg(windows)]
fn assert_windows_profile_lifecycle(webview: &tauri::WebviewWindow<MockRuntime>) {
    let response = get_ipc_response(
        webview,
        invoke_request(
            "ai_provider_profile_upsert",
            upsert_body("gw-main", "https://gateway.example.invalid/v1/", true, None),
        ),
    )
    .expect("合法 profile 必须能写入");
    let created: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(created["profileId"], "gw-main");
    assert_eq!(created["kind"], "openai_compatible");
    assert_eq!(
        created["endpoint"], "https://gateway.example.invalid/v1",
        "端点必须被规范化为无尾斜杠"
    );
    assert_eq!(created["credentialConfigured"], false, "尚未配置凭据");
    assert_no_credential_material(&created);
    let revision = created["revision"].as_str().unwrap().to_owned();
    assert!(!revision.is_empty());

    // 过期版本写入 → 稳定冲突且不改内容。
    let response = get_ipc_response(
        webview,
        invoke_request(
            "ai_provider_profile_upsert",
            upsert_body(
                "gw-main",
                "https://other.example.invalid/v1",
                true,
                Some("stale"),
            ),
        ),
    );
    let message = response.expect_err("过期版本必须冲突").to_string();
    assert!(
        message.contains("AI_PROVIDER_PROFILE_REVISION_CONFLICT"),
        "冲突错误码必须稳定，实际: {message}"
    );
    assert!(
        message.contains("\"retryable\":true"),
        "CAS 冲突可重试，实际: {message}"
    );

    let response = get_ipc_response(
        webview,
        invoke_request(
            "ai_provider_profile_get",
            serde_json::json!({ "request": { "profileId": "gw-main" } }),
        ),
    )
    .unwrap();
    let fetched: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(fetched["endpoint"], "https://gateway.example.invalid/v1");
    assert_eq!(fetched["revision"], revision);

    // 没有密钥 → 诚实空目录，不发请求。
    let response = get_ipc_response(
        webview,
        invoke_request(
            "ai_provider_models_list",
            serde_json::json!({ "request": { "profileId": "gw-main" } }),
        ),
    )
    .expect("无凭据是诚实空态而不是错误");
    let catalog: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(catalog["state"], "no_credential");
    assert_eq!(catalog["models"].as_array().unwrap().len(), 0);
    assert_no_credential_material(&catalog);

    // 停用后同样是空目录，且原因是 disabled。
    let response = get_ipc_response(
        webview,
        invoke_request(
            "ai_provider_profile_upsert",
            upsert_body(
                "gw-main",
                "https://gateway.example.invalid/v1",
                false,
                Some(&revision),
            ),
        ),
    )
    .unwrap();
    let updated: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(updated["enabled"], false);
    assert_ne!(updated["revision"], created["revision"]);

    let response = get_ipc_response(
        webview,
        invoke_request(
            "ai_provider_models_list",
            serde_json::json!({ "request": { "profileId": "gw-main" } }),
        ),
    )
    .unwrap();
    let catalog: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(catalog["state"], "disabled");
    assert_eq!(catalog["models"].as_array().unwrap().len(), 0);

    // 删除：凭据本来就不存在，`credentialDeleted` 如实为 false。
    let response = get_ipc_response(
        webview,
        invoke_request(
            "ai_provider_profile_delete",
            serde_json::json!({
                "request": { "profileId": "gw-main", "expectedRevision": null }
            }),
        ),
    )
    .expect("删除必须成功");
    let deleted: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(deleted["profileId"], "gw-main");
    assert_eq!(deleted["credentialDeleted"], false);
    assert_no_credential_material(&deleted);

    let response = get_ipc_response(
        webview,
        invoke_request("ai_provider_profile_list", serde_json::json!({})),
    )
    .unwrap();
    let json: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(
        json["profiles"].as_array().unwrap().len(),
        0,
        "删除后不得残留行"
    );
}

#[test]
fn profile_lifecycle_is_cas_and_credential_cleanup_is_reported() {
    let app = app();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    // 平台决定断言：Windows 有真实 Windows Credential Manager，非 Windows 只有
    // `UnsupportedCredentialStore`。两条分支各自断言本平台可验证的事实，不共享断言。
    #[cfg(windows)]
    assert_windows_profile_lifecycle(&webview);

    #[cfg(not(windows))]
    assert_credential_store_unsupported(&webview);
}

#[test]
fn unauthorized_window_is_denied_for_ai_provider_commands() {
    let app = app();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main2", Default::default())
        .build()
        .unwrap();
    for command in [
        "ai_provider_profile_list",
        "ai_provider_profile_get",
        "ai_provider_profile_upsert",
        "ai_provider_profile_delete",
        "ai_provider_models_list",
    ] {
        let response = get_ipc_response(&webview, invoke_request(command, serde_json::json!({})));
        let err = response.expect_err("main2 标签调用 AI Provider 命令必须被 ACL 拒绝");
        let text = err.to_string();
        assert!(
            text.contains("not allowed"),
            "{command} 的拒绝必须是 ACL deny，实际: {text}"
        );
    }
}

#[test]
fn there_is_no_free_form_ai_invocation_command() {
    // A2 的核心约束：不存在把任意提示词/工具直通模型的入口。
    // 名单来自单一事实源（与 build.rs / lib.rs 同一 include）。
    let manifest = haven_tauri_lib::COMMAND_MANIFEST;
    let names: Vec<&str> = manifest.iter().map(|(name, _)| *name).collect();
    for command in &names {
        assert!(
            !command.starts_with("agent_invoke")
                && !command.starts_with("ai_invoke")
                && !command.starts_with("ai_chat")
                && !command.starts_with("ai_complete"),
            "不得注册自由调用入口: {command}"
        );
    }
    assert!(names.contains(&"ai_provider_profile_upsert"));
    assert!(names.contains(&"ai_provider_models_list"));
}
