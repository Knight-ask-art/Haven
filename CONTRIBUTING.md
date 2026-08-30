# 参与贡献

感谢你为栖阅 Haven 提交改进。项目目前以 Windows 本地优先能力为重点，欢迎提交可复现的 bug、文档修正和小范围功能改进。

## 提交前准备

1. 使用仓库提供的 Node.js 与 Rust 版本：Node.js 22、Rust 1.94.1。`rust-toolchain.toml` 会让 rustup 自动选择该版本。
2. 在 `前端/app` 安装依赖并运行 `npm run ci:check`。
3. 在 `后端` 运行 `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings` 和 `cargo build --workspace`。
4. 如果修改了 Tauri 界面或 IPC，在前端构建完成后从 `src-tauri` 运行 `cargo build --locked --features custom-protocol`。

公共 CI 会重复执行这些检查。生成的 `dist/`、`target/`、日志、诊断导出和本地测试资料不应提交。

## 代码与架构边界

- 持久化业务事实由 Rust + SQLite 负责；前端通过 Typed HavenClient 使用受控 IPC。
- 页面组件不直接访问文件系统、数据库、来源 Provider 或任意网络地址。
- 大型媒体和图片使用受控资源协议或有界上传，不放入 JSON IPC。
- 错误需要稳定错误码和安全用户文案；日志不得包含 Token、Cookie、正文、Signed URL、完整 URL 或完整本地路径。
- 新增来源时必须同时提供真实搜索实现、来源清单条目和面向用户的能力说明。

## Issue 与 Pull Request

- 先搜索已有 Issue，尽量提供最小复现步骤、Haven 版本、Windows 版本和运行模式。
- 不要上传数据库、媒体文件、书籍正文、Cookie、Token、完整本地路径、完整远程 URL 或原始日志。
- UI 变更请描述受影响状态；只有在不含个人数据时才附加截图。
- PR 应说明修改目的、验证命令、未验证的环境差异和已知限制。
- 安全问题请遵循 [SECURITY.md](SECURITY.md)，不要在公开 Issue 中披露敏感细节。

## 许可

提交代码即表示你同意该贡献按仓库的 MIT License 发布。第三方资源和运行时仍受各自 Notice 与许可证约束。
