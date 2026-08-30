# permissions

本目录为 Tauri 2 权限声明目录。`tauri-build` 依据 `build.rs` 中的
`AppManifest::commands(...)` 为每个自定义命令自动生成 `allow-<command>` /
`deny-<command>` 权限，输出到 `target/.../build/haven-tauri-*/out/`（经由
`generate_context!` 并入 App Manifest）。

最小权限策略（IPC-TAURI-001A/B，P0-2 修复后）：

- **名单一致**：`build.rs AppManifest::commands` / `lib.rs register_invoke_handler`
  / `capabilities/main.json permissions` 由同一份命令清单生成，避免权限漂移。
- capability 只包含 `core:default` + `core:event:default` + `app:allow-*`（每个自定义命令显式授予）。
- **禁止**声明 `fs:*`、`shell:*`、`process:*` 等 broad 权限；WebView 不获得任意
  文件系统/进程/Shell 能力。
- 不允许 remote origin 调用 Tauri API（CSP `connect-src ipc: http://ipc.localhost`，无 remote 列表）。
- 目录选择走 Native 对话框（rfd），WebView 无路径输入入口（P0-1）。

发布构建会重新生成权限产物并检查命令清单；本公开快照不包含本机验收或测试源码。
