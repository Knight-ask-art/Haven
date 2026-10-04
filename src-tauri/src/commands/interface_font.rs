//! InterfaceFont Commands（BE-INTERFACE-FONT-001）。
//!
//! 路径授权模型与 `storage_location` 一致：
//! - WebView **从不提交或接收路径**。字体文件选择在 **Rust 侧 Native 对话框**（rfd）
//!   完成，读取与校验也在 Rust 侧；前端只拿到 opaque `id` 与展示用的元数据。
//! - 命令返回的 `fileName` 只用于展示（去掉目录成分后的单段文本），
//!   任何路径拼接都不使用它，资源协议只认 opaque id。
//! - 导入在**读取之前**先用 metadata 挡掉空文件与超限文件，避免把任意大文件读进内存。
//!
//! 命令层 DTO 与 `SettingsChangedDto` 同一处理方式：属于命令边界形状，
//! 不由 ts-rs 生成（生成物只覆盖 haven-application DTO），前端以运行时守卫镜像。

use std::io::Read;
use std::path::Path;

use tauri::State;

use haven_application::services::interface_fonts::{
    InterfaceFontImportOutcome, InterfaceFontService,
};
use haven_application::wire::ErrorDto;
use haven_common::UtcMillis;
use haven_domain::contracts::{InterfaceFontAsset, SystemFontFamily};
use haven_infrastructure::system_fonts::MAX_FONT_FILE_BYTES;

use crate::ipc::{invalid_argument, run_blocking, to_error_dto};
use crate::state::AppState;

/// 本机字体族（只含族名，不含路径）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceFontFamilyDto {
    pub family: String,
    pub localized_family: Option<String>,
}

/// 已导入字体资产的元数据投影（**不含字节、不含路径**）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceFontAssetDto {
    pub id: String,
    pub family_name: String,
    pub file_name: String,
    pub extension: String,
    pub mime_type: String,
    pub byte_size: u64,
    pub created_at: i64,
}

/// 一次导入的结果（`deduplicated` = 同一份文件此前已导入，复用既有资产 id）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceFontImportResultDto {
    pub asset: InterfaceFontAssetDto,
    pub deduplicated: bool,
}

impl From<SystemFontFamily> for InterfaceFontFamilyDto {
    fn from(value: SystemFontFamily) -> Self {
        Self {
            family: value.family,
            localized_family: value.localized_family,
        }
    }
}

impl From<InterfaceFontAsset> for InterfaceFontAssetDto {
    fn from(value: InterfaceFontAsset) -> Self {
        Self {
            id: value.id,
            family_name: value.family_name,
            file_name: value.file_name,
            extension: value.extension,
            mime_type: value.mime_type,
            byte_size: value.byte_size,
            created_at: value.created_at.0,
        }
    }
}

impl From<InterfaceFontImportOutcome> for InterfaceFontImportResultDto {
    fn from(value: InterfaceFontImportOutcome) -> Self {
        Self {
            asset: value.asset.into(),
            deduplicated: value.deduplicated,
        }
    }
}

fn font_service(state: &AppState) -> &InterfaceFontService {
    &state.interface_fonts
}

/// 本机已安装字体族（枚举失败按空列表处理，不把系统字体目录问题放大成整页错误）。
pub async fn run_interface_font_system_list(
    state: &AppState,
) -> Result<Vec<InterfaceFontFamilyDto>, ErrorDto> {
    font_service(state)
        .list_system_families()
        .await
        .map(|families| families.into_iter().map(Into::into).collect())
        .map_err(|error| to_error_dto(&error))
}

/// 已导入字体资产（新的在前）。
pub async fn run_interface_font_asset_list(
    state: &AppState,
) -> Result<Vec<InterfaceFontAssetDto>, ErrorDto> {
    font_service(state)
        .list_assets()
        .await
        .map(|assets| assets.into_iter().map(Into::into).collect())
        .map_err(|error| to_error_dto(&error))
}

/// 删除一份导入字体。正在被界面设置引用时返回 `FONT_ASSET_IN_USE`（事务内判定）。
pub async fn run_interface_font_asset_delete(
    state: &AppState,
    asset_id: String,
) -> Result<(), ErrorDto> {
    if asset_id.trim().is_empty() {
        return Err(invalid_argument("字体标识不能为空"));
    }
    font_service(state)
        .delete(&asset_id)
        .await
        .map_err(|error| to_error_dto(&error))
}

/// 选择并导入一份字体文件。
///
/// 步骤与边界：
/// 1. Native 选择器（用户取消 → `OPERATION_CANCELLED`）；
/// 2. `metadata` 先挡空文件 / 超限文件 / 非普通文件，**再**读取字节；
/// 3. 交给服务层做扩展名 + 签名 + 摘要 + 族名安全化 + 配额 + 去重。
pub async fn run_interface_font_asset_import(
    state: &AppState,
) -> Result<InterfaceFontImportResultDto, ErrorDto> {
    let picked = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("选择字体文件")
            .add_filter("字体文件", &["ttf", "otf", "woff2"])
            .pick_file()
    })
    .await
    .map_err(|_| ErrorDto {
        code: "INTERNAL_ERROR".into(),
        user_message: "文件选择器执行失败".into(),
        retryable: false,
    })?;
    let Some(path) = picked else {
        return Err(ErrorDto {
            code: "OPERATION_CANCELLED".into(),
            user_message: "已取消选择字体文件".into(),
            retryable: false,
        });
    };

    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let metadata = tauri::async_runtime::spawn_blocking({
        let path = path.clone();
        move || std::fs::metadata(&path)
    })
    .await
    .map_err(|_| ErrorDto {
        code: "INTERNAL_ERROR".into(),
        user_message: "读取字体文件失败".into(),
        retryable: false,
    })?
    .map_err(|error| ErrorDto {
        code: "FONT_FILE_INVALID".into(),
        user_message: font_io_message(&error),
        retryable: false,
    })?;
    if !metadata.is_file() {
        return Err(invalid_argument("请选择字体文件，而不是目录"));
    }
    if metadata.len() == 0 {
        return Err(invalid_argument("字体文件为空"));
    }
    if metadata.len() > MAX_FONT_FILE_BYTES as u64 {
        return Err(invalid_argument("字体文件超过 32 MiB 上限"));
    }

    let bytes = tauri::async_runtime::spawn_blocking(move || {
        read_font_file_limited(&path, MAX_FONT_FILE_BYTES)
    })
    .await
    .map_err(|_| ErrorDto {
        code: "INTERNAL_ERROR".into(),
        user_message: "读取字体文件失败".into(),
        retryable: false,
    })?
    .map_err(|error| ErrorDto {
        code: "FONT_FILE_INVALID".into(),
        user_message: font_io_message(&error),
        retryable: false,
    })?;
    if bytes.len() > MAX_FONT_FILE_BYTES {
        // metadata 与实际读取之间文件可能被替换；以真实字节数为准再挡一次。
        return Err(invalid_argument("字体文件超过 32 MiB 上限"));
    }

    font_service(state)
        .import(&file_name, &bytes, UtcMillis::now())
        .await
        .map(Into::into)
        .map_err(|error| to_error_dto(&error))
}

/// 文件系统错误 → 稳定用户文案（**不回显路径**）。
fn font_io_message(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => "字体文件不存在".to_owned(),
        std::io::ErrorKind::PermissionDenied => "没有权限读取该字体文件".to_owned(),
        _ => "无法读取字体文件".to_owned(),
    }
}

/// 即使文件在 metadata 检查后被替换或增长，实际读取也最多只分配上限加 1 字节。
fn read_font_file_limited(path: &Path, max_bytes: usize) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// 供命令层测试与诊断使用的路径校验（不读取内容）。
pub fn is_supported_font_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("ttf" | "otf" | "woff2")
    )
}

// ---- Tauri Command 薄包装（签名不含任何路径参数）----

#[tauri::command]
pub async fn interface_font_system_list(
    state: State<'_, AppState>,
) -> Result<Vec<InterfaceFontFamilyDto>, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_interface_font_system_list(&state).await }).await
}

#[tauri::command]
pub async fn interface_font_asset_list(
    state: State<'_, AppState>,
) -> Result<Vec<InterfaceFontAssetDto>, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_interface_font_asset_list(&state).await }).await
}

#[tauri::command]
pub async fn interface_font_asset_import(
    state: State<'_, AppState>,
) -> Result<InterfaceFontImportResultDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_interface_font_asset_import(&state).await }).await
}

#[tauri::command]
pub async fn interface_font_asset_delete(
    state: State<'_, AppState>,
    asset_id: String,
) -> Result<(), ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_interface_font_asset_delete(&state, asset_id).await })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn font_path_extension_is_closed_set() {
        assert!(is_supported_font_path(&PathBuf::from("C:/f/a.ttf")));
        assert!(is_supported_font_path(&PathBuf::from("/f/a.OTF")));
        assert!(is_supported_font_path(&PathBuf::from("a.woff2")));
        assert!(!is_supported_font_path(&PathBuf::from("a.ttc")));
        assert!(!is_supported_font_path(&PathBuf::from("a.exe")));
        assert!(!is_supported_font_path(&PathBuf::from("a")));
    }

    #[test]
    fn assets_project_without_paths_or_bytes() {
        let dto: InterfaceFontAssetDto = InterfaceFontAsset {
            id: "2f1c0f4e-6f4f-4a2c-9b3f-6c2a1d4e5f60".to_owned(),
            family_name: "示例黑体".to_owned(),
            file_name: "example.ttf".to_owned(),
            extension: "ttf".to_owned(),
            mime_type: "font/ttf".to_owned(),
            byte_size: 12,
            sha256: "0".repeat(64),
            created_at: UtcMillis(7),
        }
        .into();
        assert_eq!(dto.family_name, "示例黑体");
        assert_eq!(dto.byte_size, 12);
        assert_eq!(dto.created_at, 7);
        // 序列化结果里不得出现 sha256 或 bytes 字段。
        let json = serde_json::to_string(&dto).unwrap();
        assert!(!json.contains("sha256"));
        assert!(!json.contains("bytes"));
        assert!(json.contains("\"familyName\""));
    }

    #[test]
    fn io_errors_never_echo_paths() {
        let error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "C:\\secret\\a.ttf");
        let message = font_io_message(&error);
        assert!(!message.contains("secret"));
        assert_eq!(message, "没有权限读取该字体文件");
    }

    #[test]
    fn font_file_read_is_bounded_to_limit_plus_one() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("example.ttf");
        std::fs::write(&path, b"0123456789").unwrap();

        let bytes = read_font_file_limited(&path, 4).unwrap();
        assert_eq!(bytes, b"01234");
    }
}
