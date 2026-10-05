<#
.SYNOPSIS
把随安装包分发的 Haven MCP 运行时组装进 Tauri 的 bundle resources。

.DESCRIPTION
自动配置写进客户端的是「安装后真实存在」的那条命令，因此安装包里必须**真的**有这个运行时：
编译好的 MCP server、生产依赖，以及一个能跑的 Node 运行时。这一步把三者组装成
`<out>/haven-mcp/`，也就是 `src-tauri/tauri.conf.json` 的 `bundle.resources` 声明的目录
（映射到安装后的 `<resource_dir>/haven-mcp`，与 `src-tauri/src/commands/mcp_client.rs` 的
`RUNTIME_BUNDLE_DIR` 同名）。

**必须在 `tauri build` 之前运行**：bundle 阶段只是把已经存在的目录搬进 MSI / NSIS，
它不会替我们编译 server 或下载 Node。

为什么要有这个脚本而不是把命令内联进每个 workflow：CI 与发布流程都要做同一件事，
两份内联的 `npm ci / build / prune / package-runtime` 迟早会分叉——而分叉的表现是
"CI 绿、发布出来的安装包缺文件"，那是最难查的一类失败。

产物**不进 Git**：`resources/haven-mcp/` 下只有一份占位 README 被跟踪，其余由本脚本生成
（见仓库根 `.gitignore`）。

.PARAMETER Out
bundle resources 目录（相对于仓库根，默认 `src-tauri/resources`）。

.PARAMETER McpRoot
MCP server 包目录（相对于仓库根，默认 `mcp/haven-mcp`）。

.PARAMETER NodeBinary
要随包分发的 Node 可执行文件；默认取当前 PATH 上的 `node`（CI 用 `.node-version` 装好）。
#>
[CmdletBinding()]
param(
    [string]$Out = "src-tauri/resources",
    [string]$McpRoot = "mcp/haven-mcp",
    [string]$NodeBinary = ""
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$mcpDirectory = Join-Path $repoRoot $McpRoot
if (-not (Test-Path -LiteralPath $mcpDirectory -PathType Container)) {
    throw "找不到 MCP server 目录：$mcpDirectory"
}

if ([string]::IsNullOrWhiteSpace($NodeBinary)) {
    $NodeBinary = (Get-Command node -ErrorAction Stop).Source
}
if (-not (Test-Path -LiteralPath $NodeBinary -PathType Leaf)) {
    throw "找不到要随包分发的 Node 可执行文件：$NodeBinary"
}

# 1) 编译 server，并只保留生产依赖：产物里不该出现 typescript / vitest 这类开发依赖。
Push-Location $mcpDirectory
try {
    npm ci --no-audit --no-fund
    if ($LASTEXITCODE -ne 0) { throw "npm ci 失败" }
    npm run build
    if ($LASTEXITCODE -ne 0) { throw "npm run build 失败" }
    # 裁剪安装树，不让 npm 的锁文件序列化版本改写受跟踪的依赖输入。
    npm prune --omit=dev --no-save
    if ($LASTEXITCODE -ne 0) { throw "npm prune 失败" }
}
finally {
    Pop-Location
}

# 2) 确定性打包：固定布局、固定顺序、无时间戳，因此同一份输入得到逐字节相同的清单。
$outPath = if ([System.IO.Path]::IsPathRooted($Out)) { $Out } else { Join-Path $repoRoot $Out }
python (Join-Path $repoRoot "tools/mcp/package-runtime.py") `
    --root $repoRoot `
    --out $outPath `
    --node $NodeBinary
if ($LASTEXITCODE -ne 0) { throw "package-runtime.py 失败" }

$bundle = Join-Path $outPath "haven-mcp"
$entry = Join-Path $bundle "dist/index.js"
if (-not (Test-Path -LiteralPath $entry -PathType Leaf)) {
    throw "组装后的产物缺少 $entry"
}
$runtime = Join-Path $bundle "runtime/node.exe"
if (-not (Test-Path -LiteralPath $runtime -PathType Leaf)) {
    throw "组装后的产物缺少 $runtime"
}

Write-Host "Haven MCP 运行时已组装到 $bundle"
