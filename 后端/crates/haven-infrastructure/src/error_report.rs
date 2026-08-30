//! 本地诊断报告 Infrastructure（V02-OPEN-SOURCE-DIAGNOSTICS-001）。
//!
//! 这里只提供受控的运行时摘要、固定数据目录下的原子导出和固定 Haven Issue
//! 页面启动。报告正文已经由 Application 脱敏，Infrastructure 不读取数据库内容、
//! 日志正文或用户文件，也不会把绝对路径返回给 IPC。

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use haven_application::services::app_info::AppInfoPorts;
use haven_application::services::error_report::{
    ErrorReportFacts, ErrorReportIssueDraft, ErrorReportPorts,
};
use haven_common::{AppError, ErrorKind};
use uuid::Uuid;

const REPORT_DIRECTORY_NAME: &str = "Reports";
const MAX_REPORT_BYTES: usize = 256 * 1024;
const ISSUE_URL_BASE: &str = "https://github.com/Knight-ask-art/Haven/issues/new";

/// 本地诊断报告提供器。组合根注入固定应用数据目录和 AppInfo 端口；不会接受
/// 来自前端的目录、URL、Header 或其它文件系统参数。
pub struct LocalErrorReportProvider {
    app_info: Arc<dyn AppInfoPorts>,
    data_dir: PathBuf,
    issue_launcher: Arc<dyn IssueLauncher>,
}

/// 固定 Issue 页面启动器端口；测试注入 Fake，生产环境只打开固定 URL。
pub trait IssueLauncher: Send + Sync {
    fn launch(&self, url: &str) -> std::io::Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PlatformIssueLauncher;

impl IssueLauncher for PlatformIssueLauncher {
    fn launch(&self, url: &str) -> std::io::Result<()> {
        open_url_with_platform(url)
    }
}

impl LocalErrorReportProvider {
    pub fn new(app_info: Arc<dyn AppInfoPorts>, data_dir: PathBuf) -> Self {
        Self {
            app_info,
            data_dir,
            issue_launcher: Arc::new(PlatformIssueLauncher),
        }
    }

    #[cfg(test)]
    fn with_launcher(
        app_info: Arc<dyn AppInfoPorts>,
        data_dir: PathBuf,
        issue_launcher: Arc<dyn IssueLauncher>,
    ) -> Self {
        Self {
            app_info,
            data_dir,
            issue_launcher,
        }
    }
}

impl ErrorReportPorts for LocalErrorReportProvider {
    fn collect(&self) -> Result<ErrorReportFacts, AppError> {
        let info = self.app_info.get()?;
        Ok(ErrorReportFacts {
            app_version: info.app_version,
            operating_system: operating_system_label(),
            runtime_mode: info.build_channel,
            protocol_version: info.protocol_version,
            database_version: info.database_version,
            source_pack_version: info.source_pack_version,
            // 当前工程没有统一的持久化日志事实源；不伪造日志，只返回空摘要。
            stable_error_codes: Vec::new(),
            diagnostic_lines: Vec::new(),
        })
    }

    fn export(&self, report_id: &str, payload: &[u8]) -> Result<(), AppError> {
        validate_report_id(report_id)?;
        if payload.is_empty() || payload.len() > MAX_REPORT_BYTES {
            return Err(AppError::new(
                "ERROR_REPORT_EXPORT_FAILED",
                ErrorKind::Validation,
                "诊断报告大小无效，请重试",
                true,
            ));
        }
        let directory = self.data_dir.join(REPORT_DIRECTORY_NAME);
        std::fs::create_dir_all(&directory).map_err(|error| {
            AppError::new(
                "ERROR_REPORT_EXPORT_FAILED",
                ErrorKind::Storage,
                "诊断报告目录不可用，请重试",
                true,
            )
            .with_source(error)
        })?;
        let final_path = directory.join(format!("haven-report-{report_id}.json"));
        // 重试同一报告时保持幂等：已存在的同名报告即视为导出成功。
        if final_path.is_file() {
            return Ok(());
        }
        let temporary_path =
            directory.join(format!(".haven-report-{report_id}-{}.tmp", Uuid::new_v4()));
        let write_result = (|| {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary_path)
                .map_err(|error| {
                    AppError::new(
                        "ERROR_REPORT_EXPORT_FAILED",
                        ErrorKind::Storage,
                        "诊断报告暂时无法写入，请重试",
                        true,
                    )
                    .with_source(error)
                })?;
            use std::io::Write;
            file.write_all(payload).map_err(|error| {
                AppError::new(
                    "ERROR_REPORT_EXPORT_FAILED",
                    ErrorKind::Storage,
                    "诊断报告暂时无法写入，请重试",
                    true,
                )
                .with_source(error)
            })?;
            file.sync_all().map_err(|error| {
                AppError::new(
                    "ERROR_REPORT_EXPORT_FAILED",
                    ErrorKind::Storage,
                    "诊断报告暂时无法保存，请重试",
                    true,
                )
                .with_source(error)
            })?;
            std::fs::rename(&temporary_path, &final_path).map_err(|error| {
                AppError::new(
                    "ERROR_REPORT_EXPORT_FAILED",
                    ErrorKind::Storage,
                    "诊断报告暂时无法完成保存，请重试",
                    true,
                )
                .with_source(error)
            })
        })();
        if write_result.is_err() {
            let _ = std::fs::remove_file(&temporary_path);
        }
        write_result
    }

    fn open_issue(&self, draft: ErrorReportIssueDraft) -> Result<(), AppError> {
        validate_report_id(&draft.report_id)?;
        if draft.title.is_empty() || draft.title.chars().count() > 160 {
            return Err(AppError::new(
                "ERROR_REPORT_ISSUE_OPEN_FAILED",
                ErrorKind::Validation,
                "问题报告标题无效，请重试",
                true,
            ));
        }
        if draft.body.is_empty() || draft.body.len() > MAX_REPORT_BYTES {
            return Err(AppError::new(
                "ERROR_REPORT_ISSUE_OPEN_FAILED",
                ErrorKind::Validation,
                "问题报告摘要无效，请重试",
                true,
            ));
        }
        // URL 的域名和路径是编译期常量；title/body 已由 Application 生成，不能
        // 由前端直接传 URL。编码只允许把有限的安全摘要带入 GitHub 预填表单。
        let url = format!(
            "{ISSUE_URL_BASE}?title={}&body={}",
            percent_encode(&draft.title),
            percent_encode(&draft.body)
        );
        self.issue_launcher.launch(&url).map_err(|error| {
            AppError::new(
                "ERROR_REPORT_ISSUE_OPEN_FAILED",
                ErrorKind::Io,
                "无法打开 GitHub 问题报告页面，请重试",
                true,
            )
            .with_source(error)
        })
    }
}

fn validate_report_id(report_id: &str) -> Result<(), AppError> {
    let parsed = Uuid::parse_str(report_id).map_err(|_| invalid_report_id())?;
    if parsed.to_string() != report_id {
        return Err(invalid_report_id());
    }
    Ok(())
}

fn invalid_report_id() -> AppError {
    AppError::new(
        "INVALID_ID",
        ErrorKind::Validation,
        "诊断报告标识无效",
        false,
    )
}

fn operating_system_label() -> String {
    let name = if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        "Other"
    };
    format!("{name} ({})", std::env::consts::ARCH)
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(hex(byte >> 4));
            encoded.push(hex(byte & 0x0f));
        }
    }
    encoded
}

fn hex(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + value - 10) as char,
        _ => unreachable!("nibble must be <= 15"),
    }
}

fn open_url_with_platform(url: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        Command::new("explorer.exe").arg(url).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn().map(|_| ())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(url).spawn().map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_application::services::app_info::{
        AppInfoFacts, DirectoryFacts, DirectoryKind, ThirdPartyNoticeFacts,
    };
    use std::sync::Mutex;

    struct FakeAppInfo;

    impl AppInfoPorts for FakeAppInfo {
        fn get(&self) -> Result<AppInfoFacts, AppError> {
            Ok(AppInfoFacts {
                app_version: "0.1.0-test".into(),
                build_channel: "development".into(),
                source_pack_version: Some("builtin-1".into()),
                protocol_version: "ipc-v1".into(),
                database_version: "028_work_relations".into(),
                app_license: Some("MIT".into()),
                third_party_notices: vec![ThirdPartyNoticeFacts {
                    name: "fixture".into(),
                    license: "MIT".into(),
                }],
                directories: vec![DirectoryFacts {
                    kind: DirectoryKind::Data,
                    display_name: "data".into(),
                    display_path: "%APPDATA%/com.haven.reader".into(),
                    exists: true,
                    can_open: true,
                }],
            })
        }

        fn open_directory(&self, _kind: DirectoryKind) -> Result<(), AppError> {
            Ok(())
        }
    }

    struct FakeLauncher {
        urls: Mutex<Vec<String>>,
        fail: bool,
    }

    impl IssueLauncher for FakeLauncher {
        fn launch(&self, url: &str) -> std::io::Result<()> {
            self.urls.lock().unwrap().push(url.to_owned());
            if self.fail {
                Err(std::io::Error::other("launcher unavailable"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn collects_only_safe_runtime_facts() {
        let provider =
            LocalErrorReportProvider::new(Arc::new(FakeAppInfo), PathBuf::from("unused"));
        let facts = provider.collect().unwrap();
        assert_eq!(facts.app_version, "0.1.0-test");
        assert!(facts.operating_system.contains(std::env::consts::ARCH));
        assert!(facts.stable_error_codes.is_empty());
        assert!(facts.diagnostic_lines.is_empty());
    }

    #[test]
    fn export_is_atomic_and_idempotent_without_returning_path() {
        let dir = tempfile::tempdir().unwrap();
        let provider =
            LocalErrorReportProvider::new(Arc::new(FakeAppInfo), dir.path().to_path_buf());
        let report_id = Uuid::new_v4().to_string();
        provider
            .export(&report_id, br#"{"schemaVersion":1}"#)
            .unwrap();
        provider
            .export(&report_id, br#"{"schemaVersion":1}"#)
            .unwrap();
        let reports = dir.path().join(REPORT_DIRECTORY_NAME);
        assert_eq!(std::fs::read_dir(reports).unwrap().count(), 1);
        assert!(
            provider
                .export(&report_id, &vec![0; MAX_REPORT_BYTES + 1])
                .is_err()
        );
    }

    #[test]
    fn issue_launcher_receives_only_fixed_github_url_and_safe_body() {
        let launcher = Arc::new(FakeLauncher {
            urls: Mutex::new(Vec::new()),
            fail: false,
        });
        let provider = LocalErrorReportProvider::with_launcher(
            Arc::new(FakeAppInfo),
            PathBuf::from("unused"),
            launcher.clone(),
        );
        let report_id = Uuid::new_v4().to_string();
        provider
            .open_issue(ErrorReportIssueDraft {
                report_id: report_id.clone(),
                title: "Haven 问题".into(),
                body: format!("报告 ID：{report_id}\n错误码：SOURCE_TIMEOUT"),
            })
            .unwrap();
        let url = launcher.urls.lock().unwrap()[0].clone();
        assert!(url.starts_with(ISSUE_URL_BASE));
        assert!(url.contains("title="));
        assert!(url.contains("body="));
        assert!(!url.contains("C:\\"));
        assert!(!url.contains("Authorization"));
    }

    #[test]
    fn launcher_failures_are_retryable_and_stable() {
        let launcher = Arc::new(FakeLauncher {
            urls: Mutex::new(Vec::new()),
            fail: true,
        });
        let provider = LocalErrorReportProvider::with_launcher(
            Arc::new(FakeAppInfo),
            PathBuf::from("unused"),
            launcher,
        );
        let error = provider
            .open_issue(ErrorReportIssueDraft {
                report_id: Uuid::new_v4().to_string(),
                title: "Haven 问题".into(),
                body: "脱敏摘要".into(),
            })
            .unwrap_err();
        assert_eq!(error.code().as_str(), "ERROR_REPORT_ISSUE_OPEN_FAILED");
        assert!(error.retryable());
    }
}
