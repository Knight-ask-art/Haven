param(
    [switch]$SkipInstall
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot ".")).Path
$frontendRoot = Join-Path $repositoryRoot "前端\app"
$tauriRoot = Join-Path $repositoryRoot "src-tauri"
$frontendManifest = Join-Path $frontendRoot "package.json"
$tauriManifest = Join-Path $tauriRoot "tauri.conf.json"

if (-not (Test-Path -LiteralPath $frontendManifest -PathType Leaf)) {
    throw "找不到前端 package.json: $frontendManifest"
}
if (-not (Test-Path -LiteralPath $tauriManifest -PathType Leaf)) {
    throw "找不到 Tauri 配置: $tauriManifest"
}

$frontendVersion = (Get-Content -LiteralPath $frontendManifest -Raw | ConvertFrom-Json).version
$tauriVersion = (Get-Content -LiteralPath $tauriManifest -Raw | ConvertFrom-Json).version
if ($frontendVersion -ne $tauriVersion) {
    throw "前端版本 $frontendVersion 与 Tauri 版本 $tauriVersion 不一致"
}

Write-Host "构建栖阅 Haven v$tauriVersion（custom-protocol）"

Push-Location $frontendRoot
try {
    if (-not $SkipInstall) {
        npm ci --no-audit --no-fund
        if ($LASTEXITCODE -ne 0) { throw "npm ci 失败" }
    }
    npm run build
    if ($LASTEXITCODE -ne 0) { throw "前端构建失败" }
}
finally {
    Pop-Location
}

Push-Location $tauriRoot
try {
    cargo build --locked --features custom-protocol
    if ($LASTEXITCODE -ne 0) { throw "Tauri custom-protocol 构建失败" }
}
finally {
    Pop-Location
}

$debugOutput = Join-Path $tauriRoot "target\debug"
$executables = @(
    if (Test-Path -LiteralPath $debugOutput -PathType Container) {
        Get-ChildItem -LiteralPath $debugOutput -Filter "*.exe" -File | Select-Object -ExpandProperty FullName
    }
)

Write-Host "构建完成。Tauri 调试产物目录：$debugOutput"
if ($executables.Count -gt 0) {
    Write-Host "可执行文件："
    $executables | ForEach-Object { Write-Host "  $_" }
}
