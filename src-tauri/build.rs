// R-MAIN-06 / Tauri 复审修复：命令名单唯一事实源（`command-manifest.rs`），
// 由三处 include，防止 capability 测试与实现漂移。
include!("command-manifest.rs");

fn main() {
    // 单真源文件变化时重跑 build（tauri-build 可能不监听它）。
    println!("cargo:rerun-if-changed=command-manifest.rs");

    // P0-2：为每个自定义命令生成 `allow-<command>` / `deny-<command>` 权限，
    // capability 通过 `allow-*` 形式授予；invoke_handler / AppManifest / Capability 同源。
    let manifest = tauri_build::AppManifest::new().commands(TARGET_COMMAND_NAMES);
    let windows_attributes = tauri_build::WindowsAttributes::new_without_app_manifest();
    if let Err(error) = tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(windows_attributes)
            .app_manifest(manifest),
    ) {
        panic!("tauri-build 失败: {error:#}");
    }

    // Windows **package target** 下，tauri-build/embed-resource 生成资源库；默认
    // Common-Controls manifest 在下方以链接指令对象重新注入（EXE 需它才能正确加载
    // 系统控件依赖，否则测试进程启动即 0xC0000139 STATUS_ENTRYPOINT_NOT_FOUND，缺
    // TaskDialogIndirect 入口）。
    //
    // 注意：`#[cfg(target_os = "windows")]` 在 build-script 中是 **build host** 的 cfg，
    // 不代表 Cargo package target；这里按运行时环境变量 `CARGO_CFG_TARGET_OS` 判断。
    // 产物在不同 host/toolchain 下可能是 `resource.lib` 或 `libresource.a`：
    // 在两个候选中**恰好一个**必须是 is_file()，0 个或 2 个都 panic 并给清楚错误。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let out_dir = std::env::var("OUT_DIR").expect("build script 运行时 OUT_DIR 必须存在");
        let mut candidates = Vec::new();
        for name in ["resource.lib", "libresource.a"] {
            let path = std::path::Path::new(&out_dir).join(name);
            if path.is_file() {
                candidates.push(path);
            }
        }
        let resource_lib = match candidates.len() {
            1 => candidates.pop().expect("恰好 1 个候选"),
            0 => panic!(
                "Windows target 下未在 OUT_DIR({out_dir}) 找到 resource.lib 或 libresource.a \
                 —— tauri-build/embed-resource 未生成 Windows 资源库，测试进程将因 \
                 缺少 Common-Controls v6 manifest 启动失败（0xC0000139）"
            ),
            n => panic!(
                "Windows target 下 OUT_DIR({out_dir}) 同时存在 {n} 个资源库候选: {candidates:?} \
                 —— 无法判定应链接哪一个"
            ),
        };
        // Integration-test targets receive the compiled icon/version resource
        // through the dedicated test link argument.  Package unit-test
        // harnesses do not receive that Cargo directive.  The complete
        // resource library is intentionally not sent through the generic
        // package link argument because that would duplicate the bin resource;
        // the unit-test path injects only the standard Common Controls v6
        // manifest dependency via the linker.  This gives every test harness
        // the activation context needed by WebView/Tauri startup code.
        println!(
            "cargo:rustc-link-arg-tests={}",
            resource_lib.to_string_lossy()
        );
        // Cargo's generic `rustc-link-arg` is the only stable way to reach a
        // package unit-test harness, but passing `/MANIFESTDEPENDENCY:"..."`
        // as a link argument is split by rustc/link.exe when the long link
        // command itself is placed in a response file.  A tiny COFF `.obj`
        // with a `.drectve` section carries the exact same linker directive
        // without a second RT_MANIFEST resource, so it can be linked into the
        // application binary and every test harness without CVTRES duplicates.
        let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH")
            .expect("Windows target 下 CARGO_CFG_TARGET_ARCH 必须存在");
        let machine = match target_arch.as_str() {
            "x86_64" => 0x8664u16,
            "x86" => 0x014cu16,
            "aarch64" => 0xaa64u16,
            "arm" => 0x01c4u16,
            other => panic!("Windows target 架构 {other} 不支持生成 Common-Controls 链接指令对象"),
        };
        let manifest_obj = std::path::Path::new(&out_dir).join("common-controls-manifest.obj");
        let directive = br#" /manifestdependency:"type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'""#;
        let raw_offset = 20u32 + 40;
        let mut object = Vec::with_capacity(raw_offset as usize + directive.len());
        object.extend_from_slice(&machine.to_le_bytes());
        object.extend_from_slice(&1u16.to_le_bytes()); // one .drectve section
        object.extend_from_slice(&0u32.to_le_bytes()); // timestamp
        object.extend_from_slice(&0u32.to_le_bytes()); // symbol table pointer
        object.extend_from_slice(&0u32.to_le_bytes()); // symbol count
        object.extend_from_slice(&0u16.to_le_bytes()); // no optional header
        object.extend_from_slice(&0u16.to_le_bytes()); // characteristics
        let mut section_name = [0u8; 8];
        section_name[..8].copy_from_slice(b".drectve");
        object.extend_from_slice(&section_name);
        object.extend_from_slice(&0u32.to_le_bytes()); // physical address
        object.extend_from_slice(&0u32.to_le_bytes()); // virtual address
        object.extend_from_slice(&(directive.len() as u32).to_le_bytes());
        object.extend_from_slice(&raw_offset.to_le_bytes());
        object.extend_from_slice(&0u32.to_le_bytes()); // relocations
        object.extend_from_slice(&0u32.to_le_bytes()); // line numbers
        object.extend_from_slice(&0u16.to_le_bytes());
        object.extend_from_slice(&0u16.to_le_bytes());
        object.extend_from_slice(&0x0010_0a00u32.to_le_bytes()); // INFO | REMOVE | align 1
        object.extend_from_slice(directive);
        std::fs::write(&manifest_obj, object).unwrap_or_else(|error| {
            panic!(
                "无法生成 Windows 测试链接指令对象: {} ({error})",
                manifest_obj.display()
            )
        });
        println!("cargo:rustc-link-arg={}", manifest_obj.to_string_lossy());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    }
    // 非 Windows package target 不链接资源库。
}
