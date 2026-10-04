# 随安装包分发的 Haven MCP 运行时（占位目录）

这个目录是 `src-tauri/tauri.conf.json` 里 `bundle.resources` 的映射源：

```json
"resources": { "resources/haven-mcp": "haven-mcp" }
```

安装后它出现在 `<安装目录>/haven-mcp/`——也就是栖阅「设置 → 智能功能 → 外部 Agent 接入 →
一键配置外部 MCP 客户端」写进客户端配置的那条命令所指向的位置（路径常量见
`src-tauri/src/commands/mcp_client.rs` 的 `RUNTIME_BUNDLE_DIR`）。

**目录里只有这份 README 进 Git。** 真正的产物由

```powershell
pwsh tools/mcp/assemble-runtime.ps1
```

组装（编译 MCP server → 只留生产依赖 → 随包分发一个 Node 运行时 → 确定性打包），
`tauri build` 之前必须跑过它；CI 与发布流程都是这么接的。产物（`runtime/`、`dist/`、
`node_modules/`、启动器与清单）都在 `.gitignore` 里，不进版本库。

保留这个占位是为了让**没有**组装过运行时的 checkout 仍然能通过 `tauri build` 的配置校验：
路径存在、内容为空，安装包里就会少这个目录，而自动配置会如实报告「运行时未就绪」，
不会写出一条连不上的客户端配置。
