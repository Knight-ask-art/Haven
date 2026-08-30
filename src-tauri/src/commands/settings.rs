//! Settings Commands（IPC-TAURI-001B；P1-8/P1-9 修复）。
//!
//! - `section` 为闭合枚举字符串（未知拒绝 INVALID_ARGUMENT）。
//! - `patch` 以原始 JSON 接收并手动反序列化：未知字段/非法枚举/错误类型 →
//!   稳定 `INVALID_ARGUMENT`（而非 Tauri 默认 args 错误）。
//! - `expected_revision` 不匹配 → 稳定 `REVISION_CONFLICT`（服务层原子 CAS）。
//! - 实际变化（changed=true）→ 发布一次 `settings.changed`（revision 与 Result 同源）。
//! - 命令在 blocking worker 上执行（同步 SQLite 不阻塞 async runtime）。

use tauri::{Emitter, Manager, Runtime, State};

use haven_application::services::settings::{SettingsSnapshot, SettingsUpdateResult};
use haven_application::wire::ErrorDto;
use haven_domain::settings::{SettingsPatch, SettingsSection};

use crate::ipc::{
    run_blocking, to_error_dto, SettingsChangedDto, SETTINGS_CHANGED_TRANSPORT_EVENT,
};
use crate::state::AppState;

/// 命令核心结果：成功响应 + 待发布事件（changed=true 才有，否则 None）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsUpdateOutcome {
    pub result: SettingsUpdateResult,
    pub event: Option<SettingsChangedDto>,
}

/// 读取指定 Section（默认值 + 状态版本）。
pub async fn run_settings_get(
    state: &AppState,
    section: String,
) -> Result<SettingsSnapshot, ErrorDto> {
    let section = parse_section(&section)?;
    state
        .settings
        .get(section)
        .await
        .map_err(|e| to_error_dto(&e))
}

/// 部分更新（revision 并发控制；patch 经类型层校验；changed=true 产生事件）。
pub async fn run_settings_update(
    state: &AppState,
    section: String,
    expected_revision: Option<String>,
    patch_json: serde_json::Value,
) -> Result<SettingsUpdateOutcome, ErrorDto> {
    let section = parse_section(&section)?;
    let patch: SettingsPatch = serde_json::from_value(patch_json).map_err(|_| ErrorDto {
        code: "INVALID_ARGUMENT".into(),
        user_message: "设置字段非法（未知字段/非法枚举/类型错误）".into(),
        retryable: false,
    })?;
    let result = state
        .settings
        .update(section, expected_revision.as_deref(), patch)
        .await
        .map_err(|e| to_error_dto(&e))?;

    let event = if result.changed {
        Some(SettingsChangedDto {
            schema_version: 1,
            at: chrono::Utc::now().to_rfc3339(),
            operation_id: format!("set-{}", uuid::Uuid::new_v4()),
            sequence: 1,
            section: section.as_str().to_owned(),
            revision: result.revision.clone().unwrap_or_default(),
        })
    } else {
        None
    };

    Ok(SettingsUpdateOutcome { result, event })
}

fn parse_section(raw: &str) -> Result<SettingsSection, ErrorDto> {
    SettingsSection::parse(raw).ok_or_else(|| ErrorDto {
        code: "INVALID_ARGUMENT".into(),
        user_message: "未知设置分区".into(),
        retryable: false,
    })
}

#[tauri::command]
pub async fn settings_get(
    state: State<'_, AppState>,
    section: String,
) -> Result<SettingsSnapshot, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_settings_get(&state, section).await }).await
}

#[tauri::command]
pub async fn settings_update<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    section: String,
    expected_revision: Option<String>,
    patch: serde_json::Value,
) -> Result<SettingsUpdateResult, ErrorDto> {
    let state = (*state.inner()).clone();
    let outcome = run_blocking(move || async move {
        run_settings_update(&state, section, expected_revision, patch).await
    })
    .await?;
    if let Some(event) = outcome.event {
        let _ = app.emit(SETTINGS_CHANGED_TRANSPORT_EVENT, event);
    }
    Ok(outcome.result)
}

/// `settings_export`：将指定分区导出到 `{app_data}/config/{section}.json`（A 方案三文件）。
pub async fn run_settings_export(
    state: &AppState,
    section: String,
    app_data: std::path::PathBuf,
) -> Result<String, ErrorDto> {
    let section = parse_section(&section)?;
    let path = haven_application::services::settings_file::export_section(
        &state.settings,
        &app_data,
        section,
    )
    .await
    .map_err(|e| to_error_dto(&e))?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn settings_export<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    section: String,
) -> Result<String, ErrorDto> {
    let state = (*state.inner()).clone();
    let app_data = app.path().app_data_dir().map_err(|_| ErrorDto {
        code: "INTERNAL_ERROR".into(),
        user_message: "无法获取应用数据目录".into(),
        retryable: false,
    })?;
    run_blocking(move || async move { run_settings_export(&state, section, app_data).await }).await
}

/// `settings_import`：从 `{app_data}/config/{section}.json` 导入（经校验落库）。
pub async fn run_settings_import(
    state: &AppState,
    section: String,
    app_data: std::path::PathBuf,
) -> Result<SettingsUpdateResult, ErrorDto> {
    let section = parse_section(&section)?;
    haven_application::services::settings_file::import_section(&state.settings, &app_data, section)
        .await
        .map_err(|e| to_error_dto(&e))?;
    // 导入后读取最新快照作为结果（changed 语义由 import 内部 CAS 决定，此处简化为再读）
    state
        .settings
        .get(section)
        .await
        .map(|snap| SettingsUpdateResult {
            value: snap.value,
            revision: snap.revision,
            changed: true,
        })
        .map_err(|e| to_error_dto(&e))
}

#[tauri::command]
pub async fn settings_import<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    section: String,
) -> Result<SettingsUpdateResult, ErrorDto> {
    let state = (*state.inner()).clone();
    let app_data = app.path().app_data_dir().map_err(|_| ErrorDto {
        code: "INTERNAL_ERROR".into(),
        user_message: "无法获取应用数据目录".into(),
        retryable: false,
    })?;
    run_blocking(move || async move { run_settings_import(&state, section, app_data).await }).await
}
