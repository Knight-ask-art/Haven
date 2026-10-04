//! 云盘（Google Drive 只读切片）Commands。
//! 账户 / 目录 / 快照决策见内部 ADR-0001-cloud-drive-readonly-boundaries。
//!
//! 每条命令只有三段：有界输入校验 → 调用 Application 用例 → 把稳定错误映射成 `ErrorDto`。
//! 边界：
//! - 没有 SQL、没有 HTTP、没有第二个 Provider；真实 Drive 目录 / 文件 ID、原始分页
//!   token、授权地址与凭据引用只存在于 Application 的租约注册表与仓储里。
//! - 不解释、不拼接、不推断远端标识：目录只能由浏览句柄或内部位置 UUID 指代。
//! - 异步用例直接 await，受控 IO 与生命周期由 Application / Port 管理。

use tauri::{Emitter, Runtime, State};

use haven_application::services::cloud_storage::views::CloudConnectBeginRequest as CloudConnectBeginInput;
use haven_application::wire::{
    CloudAccountDisconnectRequest, CloudAccountDto, CloudBrowsePageDto, CloudConnectAttemptDto,
    CloudConnectBeginRequest, CloudConnectPollDto, CloudConnectStatusDto, CloudFolderDto,
    CloudImportPdfRequest, CloudObjectDto, CloudRegisterFolderRequest, CloudStorageListDto,
    ErrorDto, LibraryChangedDto,
};
use haven_domain::ids::StorageLocationId;

use crate::ipc::{invalid_argument, invalid_id, to_error_dto, LIBRARY_CHANGED_TRANSPORT_EVENT};
use crate::state::AppState;

/// 不透明 ID / 句柄的命令层长度上限（Application 另有 UUID 校验与租约注册表）。
const CLOUD_INPUT_MAX_CHARS: usize = 64;
/// 目录显示名命令层上限（Application 规则更严：非空、≤120 字符、无控制字符）。
const CLOUD_DISPLAY_NAME_MAX_CHARS: usize = 120;

fn invalidate_library<R: Runtime>(app: &tauri::AppHandle<R>) {
    let event = LibraryChangedDto {
        schema_version: 1,
        at: chrono::Utc::now().to_rfc3339(),
        operation_id: format!("cloud-{}", uuid::Uuid::new_v4()),
        sequence: 1,
        revision: None,
    };
    let _ = app.emit(LIBRARY_CHANGED_TRANSPORT_EVENT, event);
}

/// 有界的不透明 ID / 句柄：非空且不超长；含义不在命令层解释。
fn bounded_input(value: String, label: &str) -> Result<String, ErrorDto> {
    if value.is_empty() || value.chars().count() > CLOUD_INPUT_MAX_CHARS {
        return Err(invalid_argument(format!("{label}非法")));
    }
    Ok(value)
}

/// 内部位置 ID → UUID 类型；格式非法时返回稳定 `INVALID_ID`。
fn parse_location_id(raw: &str) -> Result<StorageLocationId, ErrorDto> {
    bounded_input(raw.to_owned(), "云盘位置 ID")?
        .parse()
        .map_err(|_| invalid_id())
}

/// 本机是否已配置云盘授权 + 全部账户（含断开墓碑）。
#[tauri::command]
pub async fn cloud_storage_list(
    state: State<'_, AppState>,
) -> Result<CloudStorageListDto, ErrorDto> {
    let oauth_available = state.cloud_storage.oauth_available();
    let accounts = state
        .cloud_storage
        .list_accounts()
        .await
        .map_err(|error| to_error_dto(&error))?;
    Ok(CloudStorageListDto {
        oauth_available,
        accounts: accounts.into_iter().map(CloudAccountDto::from).collect(),
    })
}

/// 发起一次云盘授权（`accountId` 省略或 `null` 即新连接）。
#[tauri::command]
pub async fn cloud_account_connect_begin(
    state: State<'_, AppState>,
    request: CloudConnectBeginRequest,
) -> Result<CloudConnectAttemptDto, ErrorDto> {
    let account_id = match request.account_id {
        Some(raw) => Some(bounded_input(raw, "云盘账户 ID")?),
        None => None,
    };
    let attempt = state
        .cloud_storage
        .begin_connect(CloudConnectBeginInput { account_id })
        .await
        .map_err(|error| to_error_dto(&error))?;
    Ok(CloudConnectAttemptDto::from(attempt))
}

/// 只读轮询一次授权尝试。
#[tauri::command]
pub async fn cloud_account_connect_poll(
    state: State<'_, AppState>,
    attempt_id: String,
) -> Result<CloudConnectPollDto, ErrorDto> {
    let attempt_id = bounded_input(attempt_id, "授权尝试 ID")?;
    state
        .cloud_storage
        .poll_connect(&attempt_id)
        .await
        .map(CloudConnectPollDto::from)
        .map_err(|error| to_error_dto(&error))
}

/// 完成一次授权（领取恰好一次；取消在最终提交前始终有效）。
#[tauri::command]
pub async fn cloud_account_connect_complete<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    attempt_id: String,
) -> Result<CloudAccountDto, ErrorDto> {
    let attempt_id = bounded_input(attempt_id, "授权尝试 ID")?;
    let account = state
        .cloud_storage
        .complete_connect(&attempt_id)
        .await
        .map(CloudAccountDto::from)
        .map_err(|error| to_error_dto(&error))?;
    invalidate_library(&app);
    Ok(account)
}

/// 取消一次授权尝试；返回值如实反映终态。
#[tauri::command]
pub async fn cloud_account_connect_cancel(
    state: State<'_, AppState>,
    attempt_id: String,
) -> Result<CloudConnectStatusDto, ErrorDto> {
    let attempt_id = bounded_input(attempt_id, "授权尝试 ID")?;
    state
        .cloud_storage
        .cancel_connect(&attempt_id)
        .await
        .map(CloudConnectStatusDto::from)
        .map_err(|error| to_error_dto(&error))
}

/// 断开账户（CAS：代际不符即失败，不静默覆盖）。
#[tauri::command]
pub async fn cloud_account_disconnect<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    request: CloudAccountDisconnectRequest,
) -> Result<CloudAccountDto, ErrorDto> {
    let account_id = bounded_input(request.account_id, "云盘账户 ID")?;
    let account = state
        .cloud_storage
        .disconnect_account(&account_id, request.expected_generation)
        .await
        .map(CloudAccountDto::from)
        .map_err(|error| to_error_dto(&error))?;
    invalidate_library(&app);
    Ok(account)
}

/// 读取一个已登记云盘目录的绑定投影（未登记 → null）。
#[tauri::command]
pub async fn cloud_folder_binding_get(
    state: State<'_, AppState>,
    location_id: String,
) -> Result<Option<CloudFolderDto>, ErrorDto> {
    let location_id = bounded_input(location_id, "云盘位置 ID")?;
    state
        .cloud_storage
        .folder_binding(&location_id)
        .await
        .map(|binding| binding.map(CloudFolderDto::from))
        .map_err(|error| to_error_dto(&error))
}

/// 移除目录绑定（只删应用内索引与统一内容；不触碰远端文件与账户凭据）。
#[tauri::command]
pub async fn cloud_folder_remove<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    location_id: String,
) -> Result<bool, ErrorDto> {
    let location_id = bounded_input(location_id, "云盘位置 ID")?;
    let removed = state
        .cloud_storage
        .remove_folder(&location_id)
        .await
        .map_err(|error| to_error_dto(&error))?;
    if removed {
        invalidate_library(&app);
    }
    Ok(removed)
}

/// 列出账户根目录（不透明句柄 + 有界分页）。
#[tauri::command]
pub async fn cloud_browse_root(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<CloudBrowsePageDto, ErrorDto> {
    let account_id = bounded_input(account_id, "云盘账户 ID")?;
    state
        .cloud_browse
        .browse_root(&account_id)
        .await
        .map(CloudBrowsePageDto::from)
        .map_err(|error| to_error_dto(&error))
}

/// 列出句柄指向的目录。
#[tauri::command]
pub async fn cloud_browse_folder(
    state: State<'_, AppState>,
    folder_handle: String,
) -> Result<CloudBrowsePageDto, ErrorDto> {
    let folder_handle = bounded_input(folder_handle, "目录句柄")?;
    state
        .cloud_browse
        .browse_folder(&folder_handle)
        .await
        .map(CloudBrowsePageDto::from)
        .map_err(|error| to_error_dto(&error))
}

/// 翻页：光标句柄一次性消费。
#[tauri::command]
pub async fn cloud_browse_next_page(
    state: State<'_, AppState>,
    cursor: String,
) -> Result<CloudBrowsePageDto, ErrorDto> {
    let cursor = bounded_input(cursor, "分页光标")?;
    state
        .cloud_browse
        .next_page(&cursor)
        .await
        .map(CloudBrowsePageDto::from)
        .map_err(|error| to_error_dto(&error))
}

/// 列出已登记目录（内部位置 UUID）。
#[tauri::command]
pub async fn cloud_browse_location(
    state: State<'_, AppState>,
    location_id: String,
) -> Result<CloudBrowsePageDto, ErrorDto> {
    let location_id = parse_location_id(&location_id)?;
    state
        .cloud_browse
        .browse_location(location_id)
        .await
        .map(CloudBrowsePageDto::from)
        .map_err(|error| to_error_dto(&error))
}

/// 登记目录：返回内部位置 UUID（再用 `cloud_folder_binding_get` 读回投影）。
/// 登记会新增一个云盘 StorageLocation，成功后与连接 / 导入一样通知库变更。
#[tauri::command]
pub async fn cloud_register_folder<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    request: CloudRegisterFolderRequest,
) -> Result<String, ErrorDto> {
    let folder_handle = bounded_input(request.folder_handle, "目录句柄")?;
    if request.display_name.chars().count() > CLOUD_DISPLAY_NAME_MAX_CHARS {
        return Err(invalid_argument("目录名称过长"));
    }
    let location_id = state
        .cloud_browse
        .register_folder(&folder_handle, &request.display_name)
        .await
        .map_err(|error| to_error_dto(&error))?;
    invalidate_library(&app);
    Ok(location_id.to_string())
}

/// 导入目录下的一份 PDF（内部位置 UUID + 不透明文件句柄）。
#[tauri::command]
pub async fn cloud_import_pdf<R: Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppState>,
    request: CloudImportPdfRequest,
) -> Result<CloudObjectDto, ErrorDto> {
    let location_id = parse_location_id(&request.location_id)?;
    let file_handle = bounded_input(request.file_handle, "文件句柄")?;
    let object = state
        .cloud_browse
        .import_pdf(location_id, &file_handle)
        .await
        .map(CloudObjectDto::from)
        .map_err(|error| to_error_dto(&error))?;
    invalidate_library(&app);
    Ok(object)
}
