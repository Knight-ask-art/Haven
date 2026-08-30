//! 视频截图本地存储（V02-PLAYBACK-HARDWARE-SCREENSHOT-001）。
//!
//! 这里是唯一接触系统保存对话框、下载目录和图片解码器的层。上层只看到
//! `Saved/Cancelled` 和稳定错误码，绝不拿到用户选择的绝对路径。

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use haven_application::services::video_screenshot::{
    VideoScreenshotSaveOutcome, VideoScreenshotStoragePort,
};
use haven_common::{AppError, ErrorKind};
use image::{ImageFormat, ImageReader};
use uuid::Uuid;

const MAX_EDGE: u32 = 8192;
const MAX_PIXELS: u64 = 40_000_000;

/// Save Dialog 的结果必须把“用户取消”和“对话框无法启动”分开。
///
/// Windows 原生适配通过 `IFileSaveDialog::Show` 的 HRESULT 提供这一区分；
/// 非 Windows 的 `rfd` fallback 只能把 `None` 视为用户取消，因为该平台 API
/// 本身不暴露启动失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
enum SaveDialogResult {
    Selected(PathBuf),
    Cancelled,
}

trait SaveDialogPort: Send + Sync {
    fn choose_path(
        &self,
        default_dir: &Path,
        suggested_name: &str,
    ) -> Result<SaveDialogResult, AppError>;
}

/// 生产环境的截图存储器。`downloads_dir` 由环境目录解析，不能由 IPC 传入。
#[derive(Clone)]
pub struct LocalVideoScreenshotProvider {
    downloads_dir: Option<PathBuf>,
    dialog: Arc<dyn SaveDialogPort>,
}

impl Default for LocalVideoScreenshotProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalVideoScreenshotProvider {
    pub fn new() -> Self {
        Self {
            downloads_dir: resolve_downloads_dir(),
            dialog: default_save_dialog(),
        }
    }

    #[cfg(test)]
    fn with_downloads_dir(path: PathBuf) -> Self {
        Self::with_downloads_dir_and_dialog(path, default_save_dialog())
    }

    #[cfg(test)]
    fn with_downloads_dir_and_dialog(path: PathBuf, dialog: Arc<dyn SaveDialogPort>) -> Self {
        Self {
            downloads_dir: Some(path),
            dialog,
        }
    }

    fn default_screenshot_dir(&self) -> Result<PathBuf, AppError> {
        self.downloads_dir
            .as_ref()
            .map(|dir| dir.join("栖阅").join("截图"))
            .ok_or_else(|| {
                AppError::new(
                    "SCREENSHOT_DIRECTORY_UNAVAILABLE",
                    ErrorKind::Storage,
                    "默认截图目录不可用",
                    true,
                )
            })
    }
}

impl VideoScreenshotStoragePort for LocalVideoScreenshotProvider {
    fn save_jpeg(&self, bytes: Vec<u8>) -> Result<VideoScreenshotSaveOutcome, AppError> {
        validate_jpeg(&bytes)?;
        let default_dir = self.default_screenshot_dir()?;
        std::fs::create_dir_all(&default_dir).map_err(|error| {
            AppError::new(
                "SCREENSHOT_DIRECTORY_UNAVAILABLE",
                ErrorKind::Storage,
                "默认截图目录不可用",
                true,
            )
            .with_source(error)
        })?;

        // 文件名不使用标题、URL 或用户输入；仅作为系统对话框的安全建议名。
        let suggested_name = format!("haven-screenshot-{}.jpg", Uuid::new_v4());
        let selection = self.dialog.choose_path(&default_dir, &suggested_name)?;
        let SaveDialogResult::Selected(path) = selection else {
            return Ok(VideoScreenshotSaveOutcome::Cancelled);
        };
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| {
                AppError::new(
                    "SCREENSHOT_DIRECTORY_UNAVAILABLE",
                    ErrorKind::Storage,
                    "截图保存目录不可用",
                    true,
                )
            })?;
        std::fs::create_dir_all(parent).map_err(|error| {
            AppError::new(
                "SCREENSHOT_DIRECTORY_UNAVAILABLE",
                ErrorKind::Storage,
                "截图保存目录不可用",
                true,
            )
            .with_source(error)
        })?;
        write_atomic_jpeg(parent, &path, &bytes)
    }
}

fn default_save_dialog() -> Arc<dyn SaveDialogPort> {
    #[cfg(windows)]
    {
        Arc::new(WindowsSaveDialog)
    }
    #[cfg(not(windows))]
    {
        Arc::new(RfdSaveDialog)
    }
}

#[cfg(not(windows))]
struct RfdSaveDialog;

#[cfg(not(windows))]
impl SaveDialogPort for RfdSaveDialog {
    fn choose_path(
        &self,
        default_dir: &Path,
        suggested_name: &str,
    ) -> Result<SaveDialogResult, AppError> {
        let selected = rfd::FileDialog::new()
            .set_directory(default_dir)
            .set_file_name(suggested_name)
            .add_filter("JPEG 图片", &["jpg", "jpeg"])
            .save_file();
        Ok(selected
            .map(SaveDialogResult::Selected)
            .unwrap_or(SaveDialogResult::Cancelled))
    }
}

#[cfg(windows)]
struct WindowsSaveDialog;

#[cfg(windows)]
impl SaveDialogPort for WindowsSaveDialog {
    fn choose_path(
        &self,
        default_dir: &Path,
        suggested_name: &str,
    ) -> Result<SaveDialogResult, AppError> {
        choose_windows_save_path(default_dir, suggested_name)
    }
}

#[cfg(windows)]
fn choose_windows_save_path(
    default_dir: &Path,
    suggested_name: &str,
) -> Result<SaveDialogResult, AppError> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
        CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx,
    };
    use windows::Win32::UI::Shell::{
        Common::COMDLG_FILTERSPEC, FileSaveDialog, IFileSaveDialog, IShellItem,
        SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
    };
    use windows::core::PCWSTR;

    struct ComGuard;

    impl Drop for ComGuard {
        fn drop(&mut self) {
            // CoInitializeEx succeeded (S_OK or S_FALSE), so this thread owns one
            // matching CoUninitialize call even when dialog setup/show fails.
            unsafe { CoUninitialize() };
        }
    }

    let init_result =
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    if init_result.is_err() {
        return Err(dialog_start_failed());
    }
    let _com = ComGuard;

    let dialog: IFileSaveDialog =
        unsafe { CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER) }
            .map_err(map_dialog_error)?;

    let default_dir_wide = wide_path(default_dir);
    let folder: IShellItem = unsafe {
        SHCreateItemFromParsingName::<_, _, IShellItem>(
            PCWSTR(default_dir_wide.as_ptr()),
            None::<&IBindCtx>,
        )
    }
    .map_err(map_dialog_error)?;
    unsafe { dialog.SetFolder(&folder) }.map_err(map_dialog_error)?;

    let suggested_name_wide = wide_string(suggested_name);
    unsafe { dialog.SetFileName(PCWSTR(suggested_name_wide.as_ptr())) }
        .map_err(map_dialog_error)?;
    let extension_wide = wide_string("jpg");
    unsafe { dialog.SetDefaultExtension(PCWSTR(extension_wide.as_ptr())) }
        .map_err(map_dialog_error)?;

    let filter_name_wide = wide_string("JPEG 图片");
    let filter_spec_wide = wide_string("*.jpg;*.jpeg");
    let filters = [COMDLG_FILTERSPEC {
        pszName: PCWSTR(filter_name_wide.as_ptr()),
        pszSpec: PCWSTR(filter_spec_wide.as_ptr()),
    }];
    unsafe { dialog.SetFileTypes(&filters) }.map_err(map_dialog_error)?;

    match unsafe { dialog.Show(None) } {
        Ok(()) => {}
        Err(error) if is_dialog_cancelled(&error) => {
            return Ok(SaveDialogResult::Cancelled);
        }
        Err(error) => return Err(map_dialog_error(error)),
    }

    let shell_item = unsafe { dialog.GetResult() }.map_err(map_dialog_error)?;
    let display_name =
        unsafe { shell_item.GetDisplayName(SIGDN_FILESYSPATH) }.map_err(map_dialog_error)?;
    let path_result = unsafe { display_name.to_string() };
    // GetDisplayName allocates with the COM task allocator. Free it regardless
    // of whether the UTF-16 conversion succeeds; the path is never logged.
    unsafe { CoTaskMemFree(Some(display_name.0.cast())) };
    let path = path_result.map_err(|error| dialog_start_failed().with_source(error))?;
    if path.trim().is_empty() {
        return Err(dialog_start_failed());
    }

    Ok(SaveDialogResult::Selected(PathBuf::from(path)))
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn wide_string(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn map_dialog_error(error: windows::core::Error) -> AppError {
    dialog_start_failed().with_source(error)
}

#[cfg(windows)]
fn is_dialog_cancelled(error: &windows::core::Error) -> bool {
    use windows::Win32::Foundation::ERROR_CANCELLED;
    use windows::core::HRESULT;

    error.code() == HRESULT::from_win32(ERROR_CANCELLED.0)
}

#[cfg(windows)]
fn dialog_start_failed() -> AppError {
    AppError::new(
        "SCREENSHOT_DIALOG_START_FAILED",
        ErrorKind::Io,
        "保存对话框无法启动，请重试。",
        true,
    )
}

fn resolve_downloads_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .map(|home| home.join("Downloads"))
}

fn validate_jpeg(bytes: &[u8]) -> Result<(), AppError> {
    if bytes.len() < 4
        || bytes.first() != Some(&0xff)
        || bytes.get(1) != Some(&0xd8)
        || bytes.get(bytes.len() - 2) != Some(&0xff)
        || bytes.last() != Some(&0xd9)
    {
        return Err(AppError::new(
            "SCREENSHOT_PAYLOAD_INVALID",
            ErrorKind::Validation,
            "截图数据无效",
            false,
        ));
    }
    let reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Jpeg);
    let (width, height) = reader.into_dimensions().map_err(|error| {
        AppError::new(
            "SCREENSHOT_PAYLOAD_INVALID",
            ErrorKind::Validation,
            "截图数据无效",
            false,
        )
        .with_source(error)
    })?;
    if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
        return Err(AppError::new(
            "SCREENSHOT_TOO_LARGE",
            ErrorKind::Validation,
            "截图尺寸过大",
            false,
        ));
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(AppError::new(
            "SCREENSHOT_TOO_LARGE",
            ErrorKind::Validation,
            "截图尺寸过大",
            false,
        ));
    }
    // 读取尺寸后再完整解码，拒绝损坏 JPEG，同时不会把超大尺寸交给解码器。
    ImageReader::with_format(Cursor::new(bytes), ImageFormat::Jpeg)
        .decode()
        .map_err(|error| {
            AppError::new(
                "SCREENSHOT_PAYLOAD_INVALID",
                ErrorKind::Validation,
                "截图数据无效",
                false,
            )
            .with_source(error)
        })?;
    Ok(())
}

fn write_atomic_jpeg(
    parent: &Path,
    destination: &Path,
    bytes: &[u8],
) -> Result<VideoScreenshotSaveOutcome, AppError> {
    let temporary = parent.join(format!(".haven-screenshot-{}.tmp", Uuid::new_v4()));
    let write_result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, destination)
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temporary);
        return Err(AppError::new(
            "SCREENSHOT_SAVE_FAILED",
            ErrorKind::Io,
            "截图保存失败",
            true,
        )
        .with_source(error));
    }
    Ok(VideoScreenshotSaveOutcome::Saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgb};
    use std::fs;
    use tempfile::TempDir;

    struct FakeSaveDialog {
        result: Result<SaveDialogResult, AppError>,
    }

    impl SaveDialogPort for FakeSaveDialog {
        fn choose_path(
            &self,
            _default_dir: &Path,
            _suggested_name: &str,
        ) -> Result<SaveDialogResult, AppError> {
            self.result.clone()
        }
    }

    fn fake_dialog(result: Result<SaveDialogResult, AppError>) -> Arc<dyn SaveDialogPort> {
        Arc::new(FakeSaveDialog { result })
    }

    fn jpeg_bytes() -> Vec<u8> {
        let image = ImageBuffer::<Rgb<u8>, Vec<u8>>::from_pixel(3, 2, Rgb([220, 80, 60]));
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut output, ImageFormat::Jpeg)
            .unwrap();
        output.into_inner()
    }

    #[test]
    fn rejects_invalid_or_oversized_dimensions_before_save() {
        let error = validate_jpeg(b"not-jpeg").unwrap_err();
        assert_eq!(error.code().as_str(), "SCREENSHOT_PAYLOAD_INVALID");
        // A valid small JPEG remains accepted and has the expected magic.
        let bytes = jpeg_bytes();
        validate_jpeg(&bytes).unwrap();
        assert_eq!(&bytes[..2], &[0xff, 0xd8]);
    }

    #[test]
    fn rejects_truncated_jpeg_even_with_valid_markers() {
        let error = validate_jpeg(&[0xff, 0xd8, 0xff, 0xd9]).unwrap_err();
        assert_eq!(error.code().as_str(), "SCREENSHOT_PAYLOAD_INVALID");
    }

    #[test]
    fn rejects_dimensions_over_the_edge_limit_before_decode() {
        let image = ImageBuffer::<Rgb<u8>, Vec<u8>>::from_pixel(8_193, 1, Rgb([20, 30, 40]));
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut output, ImageFormat::Jpeg)
            .unwrap();

        let error = validate_jpeg(&output.into_inner()).unwrap_err();
        assert_eq!(error.code().as_str(), "SCREENSHOT_TOO_LARGE");
    }

    #[test]
    fn reports_unavailable_default_directory_without_opening_dialog() {
        let provider = LocalVideoScreenshotProvider {
            downloads_dir: None,
            dialog: default_save_dialog(),
        };
        let error = provider.save_jpeg(jpeg_bytes()).unwrap_err();
        assert_eq!(error.code().as_str(), "SCREENSHOT_DIRECTORY_UNAVAILABLE");
    }

    #[test]
    fn preserves_user_cancel_as_a_successful_cancelled_outcome() {
        let root = TempDir::new().unwrap();
        let provider = LocalVideoScreenshotProvider::with_downloads_dir_and_dialog(
            root.path().to_path_buf(),
            fake_dialog(Ok(SaveDialogResult::Cancelled)),
        );

        let result = provider.save_jpeg(jpeg_bytes()).unwrap();

        assert_eq!(result, VideoScreenshotSaveOutcome::Cancelled);
        let screenshot_dir = provider.default_screenshot_dir().unwrap();
        assert!(fs::read_dir(screenshot_dir).unwrap().next().is_none());
    }

    #[test]
    fn exposes_dialog_start_failure_without_collapsing_it_into_cancelled() {
        let root = TempDir::new().unwrap();
        let provider = LocalVideoScreenshotProvider::with_downloads_dir_and_dialog(
            root.path().to_path_buf(),
            fake_dialog(Err(AppError::new(
                "SCREENSHOT_DIALOG_START_FAILED",
                ErrorKind::Io,
                "保存对话框无法启动，请重试。",
                true,
            ))),
        );

        let error = provider.save_jpeg(jpeg_bytes()).unwrap_err();

        assert_eq!(error.code().as_str(), "SCREENSHOT_DIALOG_START_FAILED");
        assert!(error.retryable());
        assert!(!format!("{error:?}").contains(root.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn saves_selected_path_after_dialog_returns_success() {
        let root = TempDir::new().unwrap();
        let destination = root.path().join("selected.jpg");
        let provider = LocalVideoScreenshotProvider::with_downloads_dir_and_dialog(
            root.path().to_path_buf(),
            fake_dialog(Ok(SaveDialogResult::Selected(destination.clone()))),
        );

        let result = provider.save_jpeg(jpeg_bytes()).unwrap();

        assert_eq!(result, VideoScreenshotSaveOutcome::Saved);
        assert!(destination.is_file());
    }

    #[cfg(windows)]
    #[test]
    fn distinguishes_windows_cancel_hresult_from_dialog_start_failure() {
        use windows::Win32::Foundation::ERROR_CANCELLED;
        use windows::core::{Error, HRESULT};

        let cancelled: Error = HRESULT::from_win32(ERROR_CANCELLED.0).into();
        let startup_failure: Error = HRESULT(0x80004005_u32 as i32).into();

        assert!(is_dialog_cancelled(&cancelled));
        assert!(!is_dialog_cancelled(&startup_failure));
    }

    #[test]
    fn maps_default_directory_creation_failure_to_stable_error() {
        let root = TempDir::new().unwrap();
        let blocking_file = root.path().join("not-a-directory");
        fs::write(&blocking_file, b"block").unwrap();
        let provider = LocalVideoScreenshotProvider::with_downloads_dir(blocking_file);

        let error = provider.save_jpeg(jpeg_bytes()).unwrap_err();
        assert_eq!(error.code().as_str(), "SCREENSHOT_DIRECTORY_UNAVAILABLE");
    }

    #[test]
    fn maps_atomic_write_failure_and_removes_temporary_file() {
        let root = TempDir::new().unwrap();
        let destination = root.path().join("missing").join("output.jpg");
        let error = write_atomic_jpeg(root.path(), &destination, &jpeg_bytes()).unwrap_err();

        assert_eq!(error.code().as_str(), "SCREENSHOT_SAVE_FAILED");
        assert!(fs::read_dir(root.path()).unwrap().all(|entry| {
            entry
                .ok()
                .map(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
                .unwrap_or(false)
        }));
    }

    #[test]
    fn creates_default_directory_and_uses_atomic_file_name() {
        let root = TempDir::new().unwrap();
        let provider = LocalVideoScreenshotProvider::with_downloads_dir(root.path().to_path_buf());
        let dir = provider.default_screenshot_dir().unwrap();
        fs::create_dir_all(&dir).unwrap();
        let destination = dir.join("test.jpg");
        let result = write_atomic_jpeg(&dir, &destination, &jpeg_bytes()).unwrap();
        assert_eq!(result, VideoScreenshotSaveOutcome::Saved);
        assert!(destination.is_file());
        assert!(fs::read_dir(&dir).unwrap().all(|entry| {
            entry
                .ok()
                .map(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
                .unwrap_or(false)
        }));
    }
}
