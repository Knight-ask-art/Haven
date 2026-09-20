## 变更说明

<!-- 用几句话说明用户问题、解决方式和不在本 PR 范围内的内容。 -->

关联 Issue / 任务 ID：

基线与当前 Head SHA：

## 范围与边界

Allowed Paths：

Forbidden Paths / 不在本 PR 范围内：

风险：`LOW | MEDIUM | HIGH | CRITICAL`

## 变更范围（用于选择性 PR Gate）

- [ ] 仅文档/README（不包含 `.github/`、依赖锁文件或发布工具）
- [ ] Frontend（`前端/`、Node 依赖或前端构建配置）
- [ ] Backend Rust（`后端/`、Rust workspace 或锁文件）
- [ ] Tauri（`src-tauri/` 或 Tauri 构建配置）
- [ ] Film/TV（`contracts/film-tv/`、`tools/film-tv/` 或影视实现/fixture）
- [ ] 跨模块/高影响（workflow、IPC contract、发布工具、依赖或构建脚本）

<!--
PR Gate 会根据实际 diff 再次分类；此处勾选用于审查说明，不能绕过自动检查。
所有 PR 都运行公共策略与仓库完整性检查。相关模块的严格 lint/build/test
以及 Film/TV evidence 会阻塞合并；CodeQL Gate 只分析受影响语言。
当前 `Protect main` 规则集保持 0 个必需 approval；未来改为 1 个 approval
只需调整规则集参数，`CODEOWNERS` 与这些自动化 Gate 无需改动。
-->

## 验证

- [ ] 已在上面记录的精确 Head SHA 上执行验证
- [ ] `npm run ci:check`（如涉及前端）
- [ ] `cargo fmt --all -- --check`（如涉及 Rust）
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`（如涉及后端）
- [ ] `cargo build --locked --features custom-protocol`（如涉及 Tauri 或前端资源）

- [ ] `python tools/release/public-snapshot-check.py`
- [ ] `python tools/docs/check.py`（如涉及文档、工作流或治理）
- [ ] `python tools/film-tv/evidence-check.py --layer contract`（如涉及 Film/TV）

未运行的检查及原因：

## 证据状态

- [ ] `CODE_PRESENT`
- [ ] `AUTOMATED_TESTED`
- [ ] `INDEPENDENTLY_REVIEWED`
- [ ] `DESKTOP_RUNTIME_VERIFIED`（不适用时说明原因）
- [ ] `RELEASE_VERIFIED`（不适用时说明原因）

运行时证据、已知限制与回滚/恢复路径：

## 隐私与发布边界

- [ ] 未提交数据库、媒体文件、书籍正文、Cookie、Token、完整 URL 或完整本地路径。
- [ ] 未提交 `dist/`、`target/`、日志、诊断导出或本地测试资料。
- [ ] 未提交 `.env`、私钥、凭据配置或其他本地 secret marker。
- [ ] 如修改第三方资源，已更新对应 Notice 或说明其许可证来源。

## UI 变更（如适用）

<!-- 只附加不含个人信息的截图或说明受影响的 loading/empty/error 状态。 -->
