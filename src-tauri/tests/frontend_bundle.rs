//! Inspect the actual generate_context! assets, not just the dist directory.
#![cfg(feature = "custom-protocol")]

#[test]
fn desktop_embeds_the_current_settings_and_search_bundle() {
    let context: tauri::Context<tauri::Wry> = tauri::generate_context!();
    let assets = context.assets();
    let embedded = assets
        .get(&"build-info.json".into())
        .expect("desktop build provenance must be embedded");
    let disk = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../前端/app/dist/build-info.json"
    ))
    .unwrap();
    assert_eq!(
        embedded.as_ref(),
        disk.as_slice(),
        "desktop embedded a stale frontend build"
    );
    let info: serde_json::Value = serde_json::from_slice(&embedded).unwrap();
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    let mut css = String::new();
    for asset in info["assets"].as_array().unwrap() {
        let name = asset["path"].as_str().unwrap();
        if name.starts_with("assets/") && (name.ends_with(".css") || name.ends_with(".js")) {
            let bytes = assets
                .get(&name.into())
                .unwrap_or_else(|| panic!("missing embedded asset {name}"));
            if name.ends_with(".css") {
                css.push_str(std::str::from_utf8(&bytes).unwrap());
            }
        }
    }
    for selector in [
        ".settings-navigation__text",
        ".sources-settings",
        ".search-results",
    ] {
        assert!(
            css.contains(selector),
            "current production UI missing: {selector}"
        );
    }
}
