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
//! 本测试**不会**调用 `credential_set`：真实运行环境下那会写进用户的 Windows
//! 凭据管理器。凭据读写路径由 `CredentialAccessService` 与 `AiProviderProfileService`
//! 的内存替身单测覆盖。
//!
//! **平台差异**：非 Windows 平台按 ADR-001 装配 `UnsupportedCredentialStore`，
//! profile 行写入后读取凭据存在性必然失败。因此 `profile_lifecycle_*` 在非 Windows
//! 上断言稳定的 `CREDENTIAL_UNSUPPORTED`（不可重试、不回显端点）后提前返回；
//! 完整的 CAS / 空目录 / 停用 / 删除生命周期只在 Windows（真实 Credential Manager）
//! 上执行。测试不会为跑通而替换成伪造的凭据后端。

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
use tauri::{Context, Url};

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

fn app() -> tauri::App<MockRuntime> {
    let db = Arc::new(Db::open_in_memory().unwrap());
    haven_tauri_lib::register_invoke_handler(mock_builder())
        .manage(AppState::new(db))
        .build(real_acl_context())
        .unwrap()
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
        "http://127.0.0.1:11434/v1",
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
