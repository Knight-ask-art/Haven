//! 外观资产的桌面 CSP 边界（Appearance Stage 1B 收口）。
//!
//! 事实源：`tauri.conf.json` 的 `app.security.csp`。本测试只读该文件，不重算策略，
//! 断言四个消费边界都显式列出了外观受控资源，且既有指令/来源没有被改写或放宽。
//!
//! 受控外观形态只有两种，前后端必须一致：
//! - 前端 `isControlledAppearanceUri`（`features/settings/lib/appearance-runtime.ts`）
//!   的正则只接受 `haven-resource://appearance/<uuid>` 与
//!   `http://haven-resource.appearance/<uuid>`；
//! - 后端 `resource_protocol.rs::parse_request_resource` 只认 `appearance` authority
//!   （原生 scheme 与 Windows WebView 的 `http://haven-resource.appearance` 兼容形态）。
//!
//! 三个资产种类落在三个不同的 CSP 边界上：静态壁纸 → `img-src`，动态壁纸 →
//! `media-src`，自定义字体（`FontFace("url(...)")`）→ `font-src`；`connect-src` 是
//! 防御性对齐，避免将来出现 fetch/预取路径时静默失败。

use serde_json::Value;

const TAURI_CONFIG_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json");

/// 自定义协议 scheme 来源：覆盖非 Windows WebView 的 `haven-resource://appearance/<uuid>`。
const APPEARANCE_SCHEME_SOURCE: &str = "haven-resource:";
/// Windows WebView 兼容形态的 host 来源：覆盖 `http://haven-resource.appearance/<uuid>`。
const APPEARANCE_WINDOWS_AUTHORITY_SOURCE: &str = "http://haven-resource.appearance";

/// 前端测试同款规范资产 ID（小写 UUID），用于还原两种受控地址的真实形状。
const SAMPLE_ASSET_ID: &str = "0196f0d2-0000-7000-8000-00000000a001";

fn csp() -> String {
    let config: Value =
        serde_json::from_str(&std::fs::read_to_string(TAURI_CONFIG_PATH).unwrap()).unwrap();
    config["app"]["security"]["csp"]
        .as_str()
        .expect("tauri.conf.json 必须声明 app.security.csp")
        .to_owned()
}

/// 取出指令的来源 token 列表；指令名必须精确匹配（不能靠前缀命中别的指令）。
fn directive_sources<'a>(csp: &'a str, directive_name: &str) -> Vec<&'a str> {
    csp.split(';')
        .map(str::trim)
        .filter(|directive| !directive.is_empty())
        .find_map(|directive| {
            let mut tokens = directive.split_whitespace();
            (tokens.next() == Some(directive_name)).then(|| tokens.collect::<Vec<_>>())
        })
        .unwrap_or_else(|| panic!("CSP 缺少 {directive_name} 指令"))
}

/// 拆出 `scheme` 与 `scheme://authority`，用于按浏览器规则判断来源是否覆盖地址。
fn split_scheme_and_origin(uri: &str) -> (&str, &str) {
    let (scheme, rest) = uri.split_once("://").expect("受控地址必须带 scheme");
    let authority_len = rest.find('/').unwrap_or(rest.len());
    (scheme, &uri[..scheme.len() + 3 + authority_len])
}

/// 该指令的来源列表是否覆盖一个具体请求地址。
///
/// 只实现本测试需要的两条匹配规则：`scheme:` 只匹配 scheme，
/// `scheme://host[:port]` 匹配同 origin（外观形态没有路径前缀）。
fn directive_allows(csp: &str, directive_name: &str, uri: &str) -> bool {
    let (scheme, origin) = split_scheme_and_origin(uri);
    directive_sources(csp, directive_name)
        .iter()
        .any(|source| match source.strip_suffix(':') {
            Some(source_scheme) => !source_scheme.contains('/') && source_scheme == scheme,
            None => *source == origin,
        })
}

#[test]
fn appearance_assets_are_allowed_on_every_boundary_they_use() {
    let csp = csp();
    let native = format!("haven-resource://appearance/{SAMPLE_ASSET_ID}");
    let windows = format!("{APPEARANCE_WINDOWS_AUTHORITY_SOURCE}/{SAMPLE_ASSET_ID}");

    for (directive_name, consumer) in [
        ("img-src", "静态壁纸"),
        ("media-src", "动态壁纸"),
        ("font-src", "自定义字体"),
        ("connect-src", "防御性对齐"),
    ] {
        let sources = directive_sources(&csp, directive_name);
        assert!(
            sources.contains(&APPEARANCE_SCHEME_SOURCE),
            "{directive_name} 必须保留 haven-resource scheme 来源（{consumer}）"
        );
        assert!(
            sources.contains(&APPEARANCE_WINDOWS_AUTHORITY_SOURCE),
            "{directive_name} 必须显式允许 Windows custom-protocol host（{consumer}）"
        );
        assert!(
            directive_allows(&csp, directive_name, &native),
            "{directive_name} 未覆盖 {native}"
        );
        assert!(
            directive_allows(&csp, directive_name, &windows),
            "{directive_name} 未覆盖 {windows}"
        );
    }
}

#[test]
fn csp_keeps_existing_directives_and_grants_nothing_broader() {
    let csp = csp();

    for directive_name in [
        "default-src",
        "script-src",
        "style-src",
        "font-src",
        "frame-src",
        "worker-src",
        "media-src",
        "img-src",
        "connect-src",
        "base-uri",
        "object-src",
    ] {
        // 指令存在性 + 原有来源不被删除。
        directive_sources(&csp, directive_name);
    }
    assert!(csp.contains("default-src 'self'"));
    assert!(csp.contains("script-src 'self'"));
    assert!(csp.contains("base-uri 'none'"));
    assert!(csp.contains("object-src 'none'"));
    assert!(directive_sources(&csp, "connect-src").contains(&"ipc:"));
    assert!(directive_sources(&csp, "connect-src").contains(&"http://ipc.localhost"));
    // 外观收口只新增来源，不引入通配或 https origin。
    assert!(!csp.contains('*'), "CSP 不得使用 wildcard");
    assert!(!csp.contains("https://"), "CSP 不得允许 remote origin");
}
