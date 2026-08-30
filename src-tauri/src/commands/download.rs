use tauri::ipc::Channel;
use tauri::State;

use haven_application::wire::{
    DownloadCreateRequest, DownloadEvent, DownloadListRequest, DownloadMutationResultDto,
    DownloadRevealResultDto, DownloadTaskActionRequest, DownloadTaskDto, ErrorDto,
};
use haven_common::{AppError, ErrorKind};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

async fn run_create(
    state: &AppState,
    request: DownloadCreateRequest,
) -> Result<DownloadTaskDto, ErrorDto> {
    state
        .download
        .create(request)
        .await
        .map_err(|error| to_error_dto(&error))
}

async fn run_list(
    state: &AppState,
    request: DownloadListRequest,
) -> Result<Vec<DownloadTaskDto>, ErrorDto> {
    state
        .download
        .list(request)
        .await
        .map_err(|error| to_error_dto(&error))
}

#[tauri::command]
pub async fn download_create(
    state: State<'_, AppState>,
    request: DownloadCreateRequest,
) -> Result<DownloadTaskDto, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_create(&state, request).await }).await
}

#[tauri::command]
pub async fn download_list(
    state: State<'_, AppState>,
    request: DownloadListRequest,
) -> Result<Vec<DownloadTaskDto>, ErrorDto> {
    let state = (*state.inner()).clone();
    run_blocking(move || async move { run_list(&state, request).await }).await
}

macro_rules! action_command {
    ($command:ident, $method:ident) => {
        #[tauri::command]
        pub async fn $command(
            state: State<'_, AppState>,
            request: DownloadTaskActionRequest,
        ) -> Result<DownloadTaskDto, ErrorDto> {
            let state = (*state.inner()).clone();
            run_blocking(move || async move {
                state
                    .download
                    .$method(request)
                    .await
                    .map_err(|error| to_error_dto(&error))
            })
            .await
        }
    };
}

action_command!(download_pause, pause);
action_command!(download_resume, resume);
action_command!(download_cancel, cancel);
action_command!(download_retry, retry);

async fn run_remove_record(
    state: &AppState,
    request: DownloadTaskActionRequest,
) -> Result<DownloadMutationResultDto, ErrorDto> {
    state
        .download
        .remove_record(request)
        .await
        .map_err(|error| to_error_dto(&error))
}

async fn run_delete_offline(
    state: &AppState,
    request: DownloadTaskActionRequest,
) -> Result<DownloadMutationResultDto, ErrorDto> {
    state
        .download
        .delete_offline(request)
        .await
        .map_err(|error| to_error_dto(&error))
}

async fn run_reveal_offline(
    state: &AppState,
    request: DownloadTaskActionRequest,
) -> Result<DownloadRevealResultDto, ErrorDto> {
    state
        .download
        .reveal_offline(request)
        .await
        .map_err(|error| to_error_dto(&error))
}

macro_rules! management_command {
    ($command:ident, $run:ident, $result:ty) => {
        #[tauri::command]
        pub async fn $command(
            state: State<'_, AppState>,
            request: DownloadTaskActionRequest,
        ) -> Result<$result, ErrorDto> {
            let state = (*state.inner()).clone();
            run_blocking(move || async move { $run(&state, request).await }).await
        }
    };
}

management_command!(
    download_remove_record,
    run_remove_record,
    DownloadMutationResultDto
);
management_command!(
    download_delete_offline,
    run_delete_offline,
    DownloadMutationResultDto
);
management_command!(
    download_reveal_offline,
    run_reveal_offline,
    DownloadRevealResultDto
);

/// 绑定下载进度 Channel。订阅不创建任务，也不读取路径；事件由共享 sink
/// 从 Rust Worker 有序推送，并由同一订阅 ID 显式释放。
#[tauri::command]
pub fn download_subscribe(
    state: State<'_, AppState>,
    subscription_id: String,
    on_event: Channel<DownloadEvent>,
) -> Result<(), ErrorDto> {
    let subscription_id = validate_subscription_id(subscription_id)?;
    state.download_sink.bind(subscription_id, on_event);
    Ok(())
}

#[tauri::command]
pub fn download_unsubscribe(
    state: State<'_, AppState>,
    subscription_id: String,
) -> Result<(), ErrorDto> {
    let subscription_id = validate_subscription_id(subscription_id)?;
    state.download_sink.unbind(&subscription_id);
    Ok(())
}

fn validate_subscription_id(value: String) -> Result<String, ErrorDto> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 128 {
        let error = AppError::new(
            "INVALID_DOWNLOAD_SUBSCRIPTION_ID",
            ErrorKind::Validation,
            "下载订阅标识无效",
            false,
        );
        return Err(to_error_dto(&error));
    }
    Ok(trimmed.to_owned())
}
