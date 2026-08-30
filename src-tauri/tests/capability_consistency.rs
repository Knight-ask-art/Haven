//! Capability / 命令清单一致性测试（IPC-TAURI-001A 验收 + R-MAIN-06 + 阻塞一/二复审修复）。
//!
//! 真实来源（单一事实源 `command-manifest.rs`，与 build.rs / lib.rs 同一 include）：
//! 1. 注册集合**直接**从 `TARGET_COMMAND_NAMES` 派生（不再是手写第三份名单）。
//! 2. **真实生成 ACL 产物**（build.rs 输出到 `$OUT_DIR/capabilities.json`）的 allow-* 授权
//!    集合必须 == `TARGET_PERMISSIONS`（读真实文件，不重算）。
//! 3. **真实 runtime authority ACL enforcement**（阻塞二）：用真实 ACL manifest 产物 +
//!    capabilities 产物解析 `tauri::utils::acl::resolved::Resolved`，构造
//!    `tauri::ipc::RuntimeAuthority`；`main` 标签调用已注册命令 → allow（control）；
//!    `main2` 标签调用同一命令 → deny（tauri 对 None 生成 "… not allowed on window …"）；
//!    未注册命令 → dispatch deny。
//! 4. capability 只声明 core 默认能力、经评审的 updater 默认权限和命令权限，
//!    无 broad fs/shell/process 权限。
//! 5. 配置层负向断言（无通配授权、无 remote origin、CSP 最小化）。

include!("../command-manifest.rs");

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use tauri::ipc::{CallbackFn, Origin, RuntimeAuthority};
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
use haven_tauri_lib::commands::storage_location::run_register_local;
use haven_tauri_lib::state::AppState;

const CAPABILITIES_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/capabilities/main.json");
const TAURI_CONFIG_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json");

/// build.rs 生成的 ACL manifest 产物（OUT_DIR；编译期嵌入路径）。
const BUILT_ACL_PATH: &str = concat!(env!("OUT_DIR"), "/acl-manifests.json");
/// build.rs 生成的 capabilities 产物（OUT_DIR；编译期嵌入路径）。
const BUILT_CAPABILITIES_PATH: &str = concat!(env!("OUT_DIR"), "/capabilities.json");

/// 模拟 IPC 请求（envelope 由各测试构造）。
/// URL 遵循 Tauri 平台规则：Windows/Android 用 `http://tauri.localhost`（本地 origin），
/// 其余平台用 `tauri://localhost`。否则 Windows 上 `tauri://localhost` 会被判为 remote，
/// 使已授权命令被 ACL 误拒。
fn invoke_request(cmd: &str, body: serde_json::Value) -> InvokeRequest {
    let url = if cfg!(any(windows, target_os = "android")) {
        Url::parse("http://tauri.localhost").unwrap()
    } else {
        Url::parse("tauri://localhost").unwrap()
    };
    InvokeRequest {
        cmd: cmd.into(),
        callback: CallbackFn(1),
        error: CallbackFn(2),
        url,
        body: body.into(),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    }
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

/// 用真实 build 产物 ACL/capabilities 构造 authority 的 context（阻塞二）。
/// `Context::runtime_authority_mut`（doc(hidden) pub）直接替换为
/// `RuntimeAuthority::new(acl, resolved)`。
fn real_acl_context() -> Context<MockRuntime> {
    let mut context = mock_context(noop_assets());
    let acl_json =
        std::fs::read_to_string(BUILT_ACL_PATH).expect("build.rs 生成 acl-manifests.json");
    let caps_json =
        std::fs::read_to_string(BUILT_CAPABILITIES_PATH).expect("build.rs 生成 capabilities.json");
    let acl: BTreeMap<String, Manifest> = serde_json::from_str(&acl_json).unwrap();
    let capabilities: BTreeMap<String, Capability> = serde_json::from_str(&caps_json).unwrap();
    let resolved = Resolved::resolve(&acl, capabilities, Target::current())
        .expect("真实 ACL/capabilities 必须可解析");
    *context.runtime_authority_mut() = RuntimeAuthority::new(acl, resolved);
    context
}

/// 空 authority 的普通 context（dispatch deny 测试用：未知命令不应先被 ACL 拒绝）。
fn empty_context() -> Context<MockRuntime> {
    mock_context(noop_assets())
}

/// 组装带真实 authority 的 app。
fn app_with_authority(context: Context<MockRuntime>) -> tauri::App<MockRuntime> {
    let db = Arc::new(Db::open_in_memory().unwrap());
    app_with_authority_and_state(context, AppState::new(db))
}

fn app_with_authority_and_state(
    context: Context<MockRuntime>,
    state: AppState,
) -> tauri::App<MockRuntime> {
    haven_tauri_lib::register_invoke_handler(mock_builder())
        .manage(state)
        .build(context)
        .unwrap()
}

/// 由唯一事实源派生的生成 ACL 权限 id（与 build.rs 的 AppManifest::commands 同规则）。
fn generated_acl_permissions() -> HashSet<&'static str> {
    TARGET_COMMANDS
        .iter()
        .map(|(_, permission)| *permission)
        .collect()
}

/// capability 文件实际声明的权限集合。
fn capability_permissions() -> HashSet<String> {
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(CAPABILITIES_PATH).unwrap()).unwrap();
    json["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_owned())
        .collect()
}

/// 阻塞一：**真实生成 ACL 产物**（build.rs 输出 `$OUT_DIR/capabilities.json`）的 allow-*
/// 授权集合必须与唯一事实源 `TARGET_PERMISSIONS` 完全一致。读真实文件，不重算。
#[test]
fn built_capabilities_acl_matches_single_source() {
    let built: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(BUILT_CAPABILITIES_PATH).unwrap())
            .expect("build.rs 必须在 OUT_DIR 生成 capabilities.json");
    let mut built_allow = HashSet::new();
    for cap in built.as_object().unwrap().values() {
        for p in cap["permissions"].as_array().unwrap() {
            let s = p.as_str().unwrap();
            if s.starts_with("allow-") {
                built_allow.insert(s.to_owned());
            }
        }
    }
    let expected: HashSet<String> = TARGET_PERMISSIONS.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        built_allow, expected,
        "真实生成 ACL 的 allow-* 集合必须与单真源一致（含缺失与多余）"
    );
}

/// 阻塞二（低层单测）：真实 ACL `resolve_access` 按窗口标签判定——
/// `main` 允许、`main2` 拒绝。**这不是 invoke 路径证据**；真实 invoke/dispatch 由
/// `real_invoke_*` 三测试承担。此单测不宣称 dispatch deny。
#[test]
fn runtime_authority_acl_enforces_capability_by_window() {
    let acl_json =
        std::fs::read_to_string(BUILT_ACL_PATH).expect("build.rs 必须生成 acl-manifests.json");
    let caps_json = std::fs::read_to_string(BUILT_CAPABILITIES_PATH)
        .expect("build.rs 必须生成 capabilities.json");
    let acl: BTreeMap<String, Manifest> = serde_json::from_str(&acl_json).unwrap();
    let capabilities: BTreeMap<String, Capability> = serde_json::from_str(&caps_json).unwrap();

    let resolved = Resolved::resolve(&acl, capabilities, Target::current())
        .expect("真实 ACL/capabilities 必须可解析");
    let authority = RuntimeAuthority::new(acl, resolved);

    // allow control：main 标签调用已注册命令 → Some（允许）。
    let main_allowed = authority.resolve_access("library_list", "main", "main", &Origin::Local);
    assert!(
        main_allowed.is_some(),
        "main 标签必须被真实 capability 授权（allow control）"
    );

    // deny：main2 标签调用同一命令 → None。
    let main2_allowed = authority.resolve_access("library_list", "main2", "main2", &Origin::Local);
    assert!(
        main2_allowed.is_none(),
        "main2 标签必须被真实 capability 拒绝"
    );

    // deny 根源：capabilities.json 的 main capability 未授权 main2 窗口。
    let caps_json: serde_json::Value = serde_json::from_str(&caps_json).unwrap();
    for cap in caps_json.as_object().unwrap().values() {
        let windows: Vec<&str> = cap["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap())
            .collect();
        assert!(
            !windows.contains(&"main2"),
            "main2 不得出现在任何真实 capability 的 windows 授权中"
        );
    }
}

/// 阻塞二（真实 invoke）：带真实 build-产物 authority 的 app + label `main` 调用
/// 已注册 `library_list` → **allow control 成功**，响应反序列化出 schemaVersion。
#[test]
fn real_invoke_allow_control_main_window() {
    let app = app_with_authority(real_acl_context());
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
    .expect("main 标签调用已注册命令必须允许（allow control）");
    let json: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(
        json["schemaVersion"], 1,
        "成功响应必须反序列化出 schemaVersion"
    );
}

/// 真实公开 invoke：位置已由 Rust 的受控本地流程注册后，`storage_location_list`
/// 只能输出四个安全字段，且不得把测试目录路径送回 WebView。
#[test]
fn real_invoke_storage_location_list_redacts_internal_paths() {
    let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
    let media = tempfile::TempDir::new().unwrap();
    let sentinel_path = media.path().to_string_lossy().into_owned();
    let location_id = tauri::async_runtime::block_on(run_register_local(
        &state,
        "安全媒体库".into(),
        media.path().to_path_buf(),
    ))
    .expect("测试目录必须能经受控路径注册")
    .to_string();

    let app = app_with_authority_and_state(real_acl_context(), state);
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let response = get_ipc_response(
        &webview,
        invoke_request("storage_location_list", serde_json::json!({})),
    )
    .expect("main 标签必须能够调用已授权的 storage_location_list");
    let json: serde_json::Value = response.deserialize().unwrap();
    let locations = json.as_array().expect("storage_location_list 必须返回数组");
    assert_eq!(locations.len(), 1);
    let fields = locations[0]
        .as_object()
        .expect("StorageLocationDto 必须是对象");
    assert_eq!(fields.len(), 4, "公开 DTO 必须保持四字段白名单");
    assert_eq!(locations[0]["locationId"], location_id.as_str());
    assert_eq!(locations[0]["displayName"], "安全媒体库");
    assert_eq!(locations[0]["providerType"], "local");
    assert_eq!(locations[0]["status"], "connected");
    for sensitive in [
        "rootPath",
        "rootRef",
        "root_ref",
        "credentialRef",
        "credential_ref",
    ] {
        assert!(
            !fields.contains_key(sensitive),
            "真实 IPC 响应不得包含 {sensitive}"
        );
    }
    assert!(
        !json_contains_string(&json, &sentinel_path),
        "真实 IPC 响应不得包含已注册目录路径: {json}"
    );
}

/// 阻塞二（真实 invoke）：带真实 authority 的 app + label `main2` 调用同一已注册
/// `library_list` → 返回错误 payload，**文本必须包含 `not allowed` 且包含 `main2`**。
#[test]
fn real_invoke_acl_deny_other_window() {
    let app = app_with_authority(real_acl_context());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main2", Default::default())
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
    );
    let err = response.expect_err("main2 标签调用已注册命令必须被 ACL 拒绝");
    let text = err.to_string();
    assert!(
        text.contains("not allowed"),
        "ACL deny 错误文本必须包含 'not allowed'，实际: {text}"
    );
    assert!(
        text.contains("main2"),
        "ACL deny 错误文本必须指明未授权窗口 main2，实际: {text}"
    );
}

/// 阻塞二（真实 dispatch deny）：**空 authority** context（无 app ACL）的 app +
/// label `main` 调用未注册命令 `not_a_command` → 返回错误文本必须包含
/// `Command not_a_command not found`（Tauri dispatch 文案），证明不是 ACL deny。
#[test]
fn real_invoke_dispatch_deny_unknown_command() {
    let app = app_with_authority(empty_context());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let response = get_ipc_response(
        &webview,
        invoke_request("not_a_command", serde_json::json!({})),
    );
    let err = response.expect_err("未注册命令必须 dispatch deny");
    let text = err.to_string();
    assert!(
        text.contains("Command not_a_command not found"),
        "dispatch deny 文本必须包含 'Command not_a_command not found'，实际: {text}"
    );
}

/// capability 授权集合必须与生成 ACL 完全一致（防漂移：TARGET_COMMANDS 变化会直接失败）。
#[test]
fn capability_matches_generated_acl_exactly() {
    let acl = generated_acl_permissions();
    let caps = capability_permissions();
    let acl: HashSet<String> = acl.into_iter().map(|s| s.to_owned()).collect();
    let core: HashSet<String> = caps
        .iter()
        .filter(|p| p.starts_with("core:"))
        .cloned()
        .collect();
    let command_caps: HashSet<String> = caps
        .iter()
        .filter(|p| p.starts_with("allow-"))
        .cloned()
        .collect();
    assert_eq!(
        command_caps, acl,
        "capability 的命令权限集合必须与生成 ACL 完全一致（含缺失与多余）"
    );
    assert!(
        core.contains("core:default") && core.contains("core:event:default"),
        "capability 必须声明 core 默认能力: {core:?}"
    );
}

/// 注册的命令名：**直接从单真源 `TARGET_COMMAND_NAMES` 派生**（不再手写第三份名单）。
/// `lib.rs` 的 generate_handler 也由同一 manifest 的 `target_handler_entries!()` 驱动，
/// 因此本集合与真实注册天然同源。
fn registered_commands() -> HashSet<&'static str> {
    TARGET_COMMAND_NAMES.iter().copied().collect()
}

/// 注册名单 == 唯一事实源（结构上恒等：registered_commands 由 TARGET_COMMAND_NAMES 派生；
/// 真实注册由同一宏派生的 handler 列表驱动）。
#[test]
fn command_manifest_matches_registration() {
    let expected: HashSet<&'static str> = TARGET_COMMANDS
        .iter()
        .map(|(command, _)| *command)
        .collect();
    assert_eq!(
        registered_commands(),
        expected,
        "注册名单必须与唯一事实源一致"
    );
}

/// 权限 id 命名规则：allow-<command>（连字符）；与 ACL 生成规则自洽。
#[test]
fn permission_ids_follow_allow_command_rule() {
    for (command, permission) in TARGET_COMMANDS {
        let expected = format!("allow-{}", command.replace('_', "-"));
        assert_eq!(
            *permission, expected,
            "权限 id 必须遵循 allow-<连字符化命令名> 规则"
        );
    }
}

#[test]
fn capability_declares_no_broad_permissions() {
    let caps = capability_permissions();
    let expected: HashSet<String> = ["core:default", "core:event:default", "updater:default"]
        .into_iter()
        .map(str::to_owned)
        .chain(
            TARGET_PERMISSIONS
                .iter()
                .map(|permission| permission.to_string()),
        )
        .collect();
    assert_eq!(
        caps, expected,
        "capability 必须精确等于已批准 core 权限 + 命令 manifest；任何新增权限都需显式评审"
    );
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(CAPABILITIES_PATH).unwrap()).unwrap();
    let windows: Vec<String> = json["windows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap().to_owned())
        .collect();
    assert!(windows.contains(&"main".to_string()));
}

#[test]
fn csp_does_not_allow_remote_origins() {
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(TAURI_CONFIG_PATH).unwrap()).unwrap();
    let csp = config["app"]["security"]["csp"]
        .as_str()
        .expect("CSP 必须存在");
    assert!(csp.contains("ipc:"), "CSP 必须允许 ipc 协议");
    assert!(
        csp.contains("connect-src 'self'"),
        "CSP 必须允许同源静态资源请求"
    );
    assert!(
        csp.contains("base-uri 'none'"),
        "CSP 必须禁止 base URI 注入"
    );
    assert!(
        csp.contains("object-src 'none'"),
        "CSP 必须禁止 object/embed 资源"
    );
    assert!(
        !csp.contains("https://"),
        "CSP 不得允许 remote origin 调用 Tauri API"
    );
    let directive = |name: &str| {
        csp.split(';')
            .map(str::trim)
            .find(|value| value.starts_with(name))
            .unwrap_or("")
    };
    assert!(
        directive("img-src").contains("http://haven-resource.comic-page"),
        "漫画页面必须只在图片边界显式允许 Windows custom-protocol host"
    );
    assert!(
        directive("connect-src").contains("http://haven-resource.comic-page"),
        "漫画页面预取必须显式允许 Windows custom-protocol host"
    );
    assert!(
        directive("media-src").contains("http://haven-resource.stream"),
        "远端流播放必须在媒体边界显式允许 stream host"
    );
    assert!(
        directive("connect-src").contains("http://haven-resource.stream"),
        "hls.js 预取必须显式允许 stream host"
    );
    assert!(
        !directive("media-src").contains("haven-resource.comic-page"),
        "漫画页面 host 不得扩大到媒体播放边界"
    );
    // 流代理 host 不得放宽到任意远程 origin。
    for directive_name in ["media-src", "connect-src"] {
        let value = directive(directive_name);
        let remote = value
            .split(' ')
            .filter(|token| token.starts_with("http"))
            .any(|token| !token.contains("haven-resource") && !token.contains("ipc.localhost"));
        assert!(!remote, "{directive_name} 不得包含非 haven-resource 远程源");
    }
    assert!(!csp.contains('*'), "CSP 不得使用 wildcard");
}

#[test]
fn unknown_command_is_not_registered() {
    // "未授权 Command 被拒绝"的注册表层面证明：名单精确等于冻结矩阵，
    // 任何未列名命令都不在 invoke_handler 中（调用将返回 command-not-found）。
    let registered = registered_commands();
    // BE-SCAN-001 第三步（IPC-TAURI-001B 扫描部分）正式接入后翻转旧守卫：
    // 扫描命令现在必须在册（此前"未接入前不得注册"的负向断言随之退役）。
    assert!(
        registered.contains("library_scan_start"),
        "扫描命令已随 BE-SCAN-001 接入，必须在册"
    );
    assert!(
        registered.contains("scan_cancel"),
        "取消命令已随 BE-SCAN-001 接入，必须在册"
    );
    assert!(!registered.contains("shell"), "禁止 shell 相关命令");
    assert!(!registered.contains("fs"), "禁止 fs 相关命令");
}

#[test]
fn real_invoke_app_info_returns_safe_projection() {
    let app = app_with_authority(real_acl_context());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let response = get_ipc_response(
        &webview,
        invoke_request("app_info_get", serde_json::json!({})),
    )
    .expect("main 标签必须能够调用 app_info_get");
    let json: serde_json::Value = response.deserialize().unwrap();
    assert_eq!(json["schemaVersion"], 1);
    assert_eq!(json["sourcePackVersion"], "builtin-1");
    assert_eq!(json["directories"].as_array().unwrap().len(), 3);
    let encoded = serde_json::to_string(&json).unwrap();
    assert!(!encoded.contains("Users"));
    assert!(!encoded.contains("haven.db"));
}
