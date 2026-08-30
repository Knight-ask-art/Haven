//! 脱敏错误诊断报告 Use Case（V02-OPEN-SOURCE-DIAGNOSTICS-001）。
//!
//! 诊断报告是一个短生命周期、用户主动确认的投影，不是业务数据存储。报告
//! 只由 Application 组装并执行最后一次安全裁剪；文件导出和固定 GitHub Issue
//! 页面打开由 Infrastructure Port 实现。没有 OAuth/PAT，因此“打开 Issue”
//! 只会打开固定仓库的预填页面，绝不在应用内直接提交。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use haven_common::{AppError, ErrorKind};
use serde::Serialize;
use uuid::Uuid;

use crate::wire::{
    ErrorReportActionRequest, ErrorReportActionResultDto, ErrorReportActionStatusDto,
    ErrorReportConfirmRequest, ErrorReportConfirmResultDto, ErrorReportDetailsDto,
    ErrorReportLevelDto, ErrorReportPreviewDto, ErrorReportPreviewRequest, ErrorReportRedactionDto,
    ErrorReportRedactionStatusDto,
};

/// 报告在内存中保留的时间。过期后必须重新生成，避免长期持有运行时上下文。
pub const ERROR_REPORT_TTL: Duration = Duration::from_secs(30 * 60);
const MAX_REPORTS: usize = 32;
const MAX_ERROR_CODES: usize = 8;
const MAX_ERROR_CODE_CHARS: usize = 64;
const MAX_DIAGNOSTIC_LINES: usize = 80;
const MAX_DIAGNOSTIC_LINE_CHARS: usize = 160;

/// Infrastructure 提供的安全运行时事实。实现方不得填入用户正文、Cookie、
/// Authorization、Signed URL、完整 URL 或绝对路径；Application 仍会再次裁剪。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorReportFacts {
    pub app_version: String,
    pub operating_system: String,
    pub runtime_mode: String,
    pub protocol_version: String,
    pub database_version: String,
    pub source_pack_version: Option<String>,
    pub stable_error_codes: Vec<String>,
    pub diagnostic_lines: Vec<String>,
}

/// 固定 Issue Launcher 的安全输入。`url` 不由调用方提供，Infrastructure 只会
/// 把这些脱敏字段编码到固定的 Haven issues/new 地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorReportIssueDraft {
    pub report_id: String,
    pub title: String,
    pub body: String,
}

/// 诊断报告的 IO 端口。导出参数已经是最终脱敏 JSON；实现方不能把路径回传
/// 给 UI，也不能记录报告正文。
pub trait ErrorReportPorts: Send + Sync {
    fn collect(&self) -> Result<ErrorReportFacts, AppError>;
    fn export(&self, report_id: &str, payload: &[u8]) -> Result<(), AppError>;
    fn open_issue(&self, draft: ErrorReportIssueDraft) -> Result<(), AppError>;
}

#[derive(Clone)]
struct PreparedReport {
    preview: ErrorReportPreviewDto,
    confirmed: bool,
    expires_at: Instant,
}

struct Inner {
    reports: Mutex<HashMap<String, PreparedReport>>,
    ports: Arc<dyn ErrorReportPorts>,
    ttl: Duration,
}

/// 短生命周期报告服务。`Clone` 只复制共享端口和状态，不复制报告内容。
#[derive(Clone)]
pub struct ErrorReportService {
    inner: Arc<Inner>,
}

impl ErrorReportService {
    pub fn new(ports: Arc<dyn ErrorReportPorts>) -> Self {
        Self::with_ttl(ports, ERROR_REPORT_TTL)
    }

    fn with_ttl(ports: Arc<dyn ErrorReportPorts>, ttl: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                reports: Mutex::new(HashMap::new()),
                ports,
                ttl,
            }),
        }
    }

    /// 生成预览。报告仍处于未确认状态，导出和打开 Issue 前必须显式 confirm。
    pub fn preview(
        &self,
        request: ErrorReportPreviewRequest,
    ) -> Result<ErrorReportPreviewDto, AppError> {
        let requested_codes = validate_codes(&request.stable_error_codes)?;
        let facts = self.inner.ports.collect()?;
        let report_id = Uuid::new_v4().to_string();
        let stable_error_codes = merge_codes(requested_codes, facts.stable_error_codes);
        let details = match request.level {
            ErrorReportLevelDto::Basic => None,
            ErrorReportLevelDto::Standard | ErrorReportLevelDto::Detailed => {
                Some(ErrorReportDetailsDto {
                    protocol_version: safe_optional_text(Some(facts.protocol_version)),
                    database_version: safe_optional_text(Some(facts.database_version)),
                    source_pack_version: safe_optional_text(facts.source_pack_version),
                    diagnostic_lines: if request.level == ErrorReportLevelDto::Detailed {
                        sanitize_diagnostic_lines(facts.diagnostic_lines)
                    } else {
                        Vec::new()
                    },
                })
            }
        };
        let preview = ErrorReportPreviewDto {
            schema_version: 1,
            report_id: report_id.clone(),
            level: request.level,
            created_at: Utc::now().to_rfc3339(),
            app_version: safe_text(&facts.app_version),
            operating_system: safe_text(&facts.operating_system),
            runtime_mode: safe_text(&facts.runtime_mode),
            stable_error_codes: stable_error_codes.clone(),
            error_summary: error_summary(&stable_error_codes),
            redaction: ErrorReportRedactionDto {
                status: ErrorReportRedactionStatusDto::Passed,
                removed_fields: vec![
                    "absolute_paths".to_owned(),
                    "credentials".to_owned(),
                    "cookies".to_owned(),
                    "signed_urls".to_owned(),
                    "user_content".to_owned(),
                ],
                contains_sensitive_data: false,
            },
            details,
            requires_confirmation: true,
        };

        let mut reports = self.inner.reports.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        reports.retain(|_, report| report.expires_at > now);
        if reports.len() >= MAX_REPORTS {
            // 只淘汰最早到期的准备态报告；不会触及已经导出的文件。
            if let Some(oldest) = reports
                .iter()
                .min_by_key(|(_, report)| report.expires_at)
                .map(|(id, _)| id.clone())
            {
                reports.remove(&oldest);
            }
        }
        reports.insert(
            report_id,
            PreparedReport {
                preview: preview.clone(),
                confirmed: false,
                expires_at: now + self.inner.ttl,
            },
        );
        Ok(preview)
    }

    /// 用户确认后才允许执行导出或打开 Issue。
    pub fn confirm(
        &self,
        request: ErrorReportConfirmRequest,
    ) -> Result<ErrorReportConfirmResultDto, AppError> {
        let now = Instant::now();
        let mut reports = self.inner.reports.lock().unwrap_or_else(|e| e.into_inner());
        let report = reports
            .get_mut(&request.report_id)
            .ok_or_else(report_expired)?;
        if report.expires_at <= now {
            reports.remove(&request.report_id);
            return Err(report_expired());
        }
        report.confirmed = true;
        Ok(ErrorReportConfirmResultDto {
            schema_version: 1,
            report_id: request.report_id,
            confirmed: true,
        })
    }

    pub fn export(
        &self,
        request: ErrorReportActionRequest,
    ) -> Result<ErrorReportActionResultDto, AppError> {
        let report = self.authorized_report(&request.report_id)?;
        let payload = serde_json::to_vec_pretty(&report.preview).map_err(|error| {
            AppError::new(
                "ERROR_REPORT_EXPORT_FAILED",
                ErrorKind::Internal,
                "诊断报告暂时无法导出，请重试",
                true,
            )
            .with_source(error)
        })?;
        // 失败时保留 prepared report，前端可以安全重试；成功时也保留短时间，
        // 便于用户随后打开同一份 Issue 草稿。
        self.inner
            .ports
            .export(&report.preview.report_id, &payload)?;
        Ok(ErrorReportActionResultDto {
            schema_version: 1,
            report_id: report.preview.report_id,
            status: ErrorReportActionStatusDto::Exported,
        })
    }

    pub fn open_issue(
        &self,
        request: ErrorReportActionRequest,
    ) -> Result<ErrorReportActionResultDto, AppError> {
        let report = self.authorized_report(&request.report_id)?;
        let draft = issue_draft(&report.preview);
        self.inner.ports.open_issue(draft)?;
        Ok(ErrorReportActionResultDto {
            schema_version: 1,
            report_id: report.preview.report_id,
            status: ErrorReportActionStatusDto::Opened,
        })
    }

    fn authorized_report(&self, report_id: &str) -> Result<PreparedReport, AppError> {
        let _ = Uuid::parse_str(report_id).map_err(|_| invalid_report_id())?;
        let now = Instant::now();
        let mut reports = self.inner.reports.lock().unwrap_or_else(|e| e.into_inner());
        let Some(report) = reports.get(report_id) else {
            return Err(report_expired());
        };
        if report.expires_at <= now {
            reports.remove(report_id);
            return Err(report_expired());
        }
        if !report.confirmed {
            return Err(AppError::new(
                "ERROR_REPORT_CONFIRMATION_REQUIRED",
                ErrorKind::Unauthorized,
                "请先确认诊断报告内容",
                false,
            ));
        }
        Ok(report.clone())
    }
}

fn validate_codes(codes: &[String]) -> Result<Vec<String>, AppError> {
    if codes.len() > MAX_ERROR_CODES {
        return Err(invalid_argument("错误码数量过多"));
    }
    let mut result = Vec::with_capacity(codes.len());
    for code in codes {
        if code.is_empty()
            || code.chars().count() > MAX_ERROR_CODE_CHARS
            || !code.chars().all(|character| {
                character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
            })
        {
            return Err(invalid_argument("错误码格式无效"));
        }
        if !result.iter().any(|current| current == code) {
            result.push(code.clone());
        }
    }
    Ok(result)
}

fn merge_codes(mut requested: Vec<String>, provided: Vec<String>) -> Vec<String> {
    for code in provided {
        if requested.len() >= MAX_ERROR_CODES {
            break;
        }
        if validate_codes(std::slice::from_ref(&code)).is_ok()
            && !requested.iter().any(|current| current == &code)
        {
            requested.push(code);
        }
    }
    requested
}

fn safe_text(value: &str) -> String {
    let value = value
        .chars()
        .filter(|character| !character.is_control())
        .take(128)
        .collect::<String>();
    if value.is_empty() {
        "未知".to_owned()
    } else if contains_sensitive_marker(&value) {
        "[已脱敏]".to_owned()
    } else {
        value
    }
}

fn safe_optional_text(value: Option<String>) -> Option<String> {
    value.map(|value| safe_text(&value))
}

fn sanitize_diagnostic_lines(lines: Vec<String>) -> Vec<String> {
    lines
        .into_iter()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || contains_sensitive_marker(line) {
                return None;
            }
            let line = line
                .chars()
                .filter(|character| !character.is_control())
                .take(MAX_DIAGNOSTIC_LINE_CHARS)
                .collect::<String>();
            if line.is_empty() || contains_sensitive_marker(&line) {
                None
            } else {
                Some(line)
            }
        })
        .take(MAX_DIAGNOSTIC_LINES)
        .collect()
}

/// 只允许将不含路径、地址或凭据语义的短运行摘要放入报告。该检查是
/// Application 层的最后一道防线，不能替代 Infrastructure 对事实来源的约束。
fn contains_sensitive_marker(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.starts_with('/')
        || value.contains('\\')
        || lower.contains("http://")
        || lower.contains("https://")
        || lower.contains("file:")
        || lower.contains("cookie")
        || lower.contains("authorization")
        || lower.contains("bearer ")
        || lower.contains("token")
        || lower.contains("password")
        || lower.contains("secret")
        || lower.contains("signed_url")
        || lower.contains("signed url")
}

fn error_summary(codes: &[String]) -> String {
    if codes.is_empty() {
        "未捕获到稳定错误码；本报告只包含脱敏运行信息".to_owned()
    } else {
        format!("稳定错误码：{}", codes.join("、"))
    }
}

fn issue_draft(preview: &ErrorReportPreviewDto) -> ErrorReportIssueDraft {
    let codes = if preview.stable_error_codes.is_empty() {
        "无".to_owned()
    } else {
        preview.stable_error_codes.join(", ")
    };
    let body = format!(
        "### 栖阅诊断摘要\n\n- 报告 ID：`{}`\n- Haven 版本：`{}`\n- 系统：`{}`\n- 运行模式：`{}`\n- 稳定错误码：`{}`\n- 报告等级：`{:?}`\n\n已在应用内完成脱敏检查。请在提交前确认没有包含用户内容或其他隐私信息。",
        preview.report_id,
        preview.app_version,
        preview.operating_system,
        preview.runtime_mode,
        codes,
        preview.level,
    );
    ErrorReportIssueDraft {
        report_id: preview.report_id.clone(),
        title: format!("Haven 问题报告 {}", preview.report_id),
        body,
    }
}

fn invalid_argument(message: &'static str) -> AppError {
    AppError::new("INVALID_ARGUMENT", ErrorKind::Validation, message, false)
}

fn invalid_report_id() -> AppError {
    AppError::new(
        "INVALID_ID",
        ErrorKind::Validation,
        "诊断报告标识无效",
        false,
    )
}

fn report_expired() -> AppError {
    AppError::new(
        "ERROR_REPORT_EXPIRED",
        ErrorKind::NotFound,
        "诊断报告已失效，请重新生成",
        true,
    )
}

// 保证报告投影未来新增字段时仍可在单元测试中显式序列化检查。
#[allow(dead_code)]
fn _assert_serializable<T: Serialize>() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakePorts {
        exports: Mutex<Vec<(String, Vec<u8>)>>,
        issues: Mutex<Vec<ErrorReportIssueDraft>>,
        export_calls: AtomicUsize,
        fail_export_once: bool,
    }

    impl FakePorts {
        fn new() -> Self {
            Self {
                exports: Mutex::new(Vec::new()),
                issues: Mutex::new(Vec::new()),
                export_calls: AtomicUsize::new(0),
                fail_export_once: false,
            }
        }
    }

    impl ErrorReportPorts for FakePorts {
        fn collect(&self) -> Result<ErrorReportFacts, AppError> {
            Ok(ErrorReportFacts {
                app_version: "0.1.0-test".into(),
                operating_system: "Windows".into(),
                runtime_mode: "development".into(),
                protocol_version: "ipc-v1".into(),
                database_version: "028_work_relations".into(),
                source_pack_version: Some("builtin-1".into()),
                stable_error_codes: vec!["SOURCE_TIMEOUT".into()],
                diagnostic_lines: vec![
                    "source=opds code=SOURCE_TIMEOUT".into(),
                    r"C:\Users\secret\haven.db".into(),
                    "Authorization: Bearer secret".into(),
                    "/var/lib/haven/haven.db".into(),
                    "refresh_token=secret".into(),
                ],
            })
        }

        fn export(&self, report_id: &str, payload: &[u8]) -> Result<(), AppError> {
            let call = self.export_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_export_once && call == 0 {
                return Err(AppError::new(
                    "ERROR_REPORT_EXPORT_FAILED",
                    ErrorKind::Io,
                    "报告导出失败，请重试",
                    true,
                ));
            }
            self.exports
                .lock()
                .unwrap()
                .push((report_id.to_owned(), payload.to_vec()));
            Ok(())
        }

        fn open_issue(&self, draft: ErrorReportIssueDraft) -> Result<(), AppError> {
            self.issues.lock().unwrap().push(draft);
            Ok(())
        }
    }

    fn preview_request(level: ErrorReportLevelDto) -> ErrorReportPreviewRequest {
        ErrorReportPreviewRequest {
            level,
            stable_error_codes: Vec::new(),
        }
    }

    #[test]
    fn preview_is_redacted_and_level_controls_details() {
        let service = ErrorReportService::new(Arc::new(FakePorts::new()));
        let basic = service
            .preview(preview_request(ErrorReportLevelDto::Basic))
            .unwrap();
        assert!(basic.details.is_none());
        assert!(!basic.redaction.contains_sensitive_data);
        assert_eq!(basic.stable_error_codes, vec!["SOURCE_TIMEOUT"]);

        let detailed = service
            .preview(preview_request(ErrorReportLevelDto::Detailed))
            .unwrap();
        let details = detailed.details.as_ref().unwrap();
        assert_eq!(
            details.diagnostic_lines,
            vec!["source=opds code=SOURCE_TIMEOUT"]
        );
        let json = serde_json::to_string(&detailed).unwrap();
        assert!(!json.contains("Users"));
        assert!(!json.contains("Bearer"));
        assert!(!json.contains("secret"));
    }

    #[test]
    fn export_and_issue_require_confirmation_and_can_retry() {
        let ports = Arc::new(FakePorts {
            fail_export_once: true,
            ..FakePorts::new()
        });
        let service = ErrorReportService::new(ports.clone());
        let preview = service
            .preview(preview_request(ErrorReportLevelDto::Standard))
            .unwrap();
        let request = ErrorReportActionRequest {
            report_id: preview.report_id.clone(),
        };
        assert_eq!(
            service.export(request.clone()).unwrap_err().code().as_str(),
            "ERROR_REPORT_CONFIRMATION_REQUIRED"
        );
        service
            .confirm(ErrorReportConfirmRequest {
                report_id: preview.report_id.clone(),
            })
            .unwrap();
        assert_eq!(
            service.export(request.clone()).unwrap_err().code().as_str(),
            "ERROR_REPORT_EXPORT_FAILED"
        );
        assert_eq!(
            service.export(request.clone()).unwrap().status,
            ErrorReportActionStatusDto::Exported
        );
        assert_eq!(
            service.open_issue(request).unwrap().status,
            ErrorReportActionStatusDto::Opened
        );
        assert_eq!(ports.exports.lock().unwrap().len(), 1);
        assert_eq!(ports.issues.lock().unwrap().len(), 1);
        let draft = &ports.issues.lock().unwrap()[0];
        assert!(draft.body.contains(&preview.report_id));
        assert!(!draft.body.contains("Users"));
    }

    #[test]
    fn invalid_or_expired_report_is_safe() {
        let service = ErrorReportService::with_ttl(Arc::new(FakePorts::new()), Duration::ZERO);
        let preview = service
            .preview(preview_request(ErrorReportLevelDto::Basic))
            .unwrap();
        let error = service
            .confirm(ErrorReportConfirmRequest {
                report_id: preview.report_id,
            })
            .unwrap_err();
        assert_eq!(error.code().as_str(), "ERROR_REPORT_EXPIRED");
        let bad = service
            .preview(ErrorReportPreviewRequest {
                level: ErrorReportLevelDto::Basic,
                stable_error_codes: vec!["not-a-code".into()],
            })
            .unwrap_err();
        assert_eq!(bad.code().as_str(), "INVALID_ARGUMENT");
    }
}
