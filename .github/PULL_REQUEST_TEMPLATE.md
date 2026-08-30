## 变更说明

<!-- 用几句话说明用户问题、解决方式和不在本 PR 范围内的内容。 -->

## 验证

- [ ] `npm run ci:check`（如涉及前端）
- [ ] `cargo fmt --all -- --check`（如涉及 Rust）
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`（如涉及后端）
- [ ] `cargo build --locked --features custom-protocol`（如涉及 Tauri 或前端资源）

未运行的检查及原因：

## 隐私与发布边界

- [ ] 未提交数据库、媒体文件、书籍正文、Cookie、Token、完整 URL 或完整本地路径。
- [ ] 未提交 `dist/`、`target/`、日志、诊断导出或本地测试资料。
- [ ] 如修改第三方资源，已更新对应 Notice 或说明其许可证来源。

## UI 变更（如适用）

<!-- 只附加不含个人信息的截图或说明受影响的 loading/empty/error 状态。 -->
