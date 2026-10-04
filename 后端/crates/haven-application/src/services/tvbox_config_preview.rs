//! TVBox / FongMi 配置预览用例（Film/TV Provider 基础切片）。
//!
//! 这个用例只回答一个问题：**这个地址上的配置长什么样**。它不保存、不注册来源、
//! 不缓存、不改 SourceRegistry，也不给设置页提供任何写入入口。
//!
//! 依赖方向（ADR-002/ADR-003 §6）：Application → 端口 ← Infrastructure。真正的
//! 网络访问收在 [`TvboxConfigPreviewPort`] 后面，生产实现由 Infrastructure 提供
//! （受控 HTTP + `haven_common::network` URL 策略 + DNS 固定 + 大小上限 + 解析），
//! 测试注入进程内 fake。Application 不依赖任何 Infrastructure 类型。
//!
//! 边界（与仓库既有的来源/网络切片一致）：
//! - 校验只做「非空 + 长度上限」两件事，错误文案固定、**不回显地址**。scheme、
//!   userinfo、私网/回环/单标签主机、端口白名单、重定向与 DNS 固定仍由端口实现
//!   统一执行——这里不复制第二份 URL 策略，避免两处策略漂移。
//! - 端口返回的是 Application 自有的事实结构；本模块把它映射成 IPC 只读投影。
//!   投影里没有地址、原始 JSON、端点、header 值、`ext` 内容、Cookie 或 Token。
//! - 不解释数字 `type` 语义，不下载/加载/执行配置声明的 JS / JAR / Python，也不
//!   调用任何外部解析服务；未确认的形态只计为 unclassified。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::{AppError, ErrorKind};

use crate::wire::{
    TvboxConfigImplementationKindDto, TvboxConfigPreviewDto, TvboxConfigPreviewRequest,
};

/// 配置地址的最大字符数。
///
/// 这是 IPC 入口的廉价上限（拒绝超长输入），不是 URL 策略本身；真实的主机、端口与
/// 重定向判定仍在端口实现里。
pub const MAX_CONFIG_URL_CHARS: usize = 2048;

/// 配置声明的第三方实现种类。
///
/// 与解析层同语义：识别到即代表「运行需要第三方代码」，本切片只标记不执行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TvboxConfigImplementationKind {
    /// `.jar`（TVBox Spider 容器）。
    Jar,
    /// `.js`（远程 JavaScript）。
    JavaScript,
    /// `.py`（远程 Python）。
    Python,
    /// `.json`（spider 清单，不是可执行代码）。
    JsonManifest,
    /// 有 references 但无法从形态判断；保持未识别，不猜。
    Unidentified,
}

impl TvboxConfigImplementationKind {
    fn to_wire(self) -> TvboxConfigImplementationKindDto {
        match self {
            Self::Jar => TvboxConfigImplementationKindDto::Jar,
            Self::JavaScript => TvboxConfigImplementationKindDto::JavaScript,
            Self::Python => TvboxConfigImplementationKindDto::Python,
            Self::JsonManifest => TvboxConfigImplementationKindDto::JsonManifest,
            Self::Unidentified => TvboxConfigImplementationKindDto::Unidentified,
        }
    }
}

/// 一次预览的安全事实。
///
/// 全部字段都是计数、布尔或固定白名单里的字段名。实现**不得**把配置地址、正文、
/// 站点名、header 名/值、`ext` 内容或未识别字段名放进这里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TvboxConfigPreviewFacts {
    pub site_count: usize,
    pub live_count: usize,
    pub parser_count: usize,
    pub skipped_site_rows: usize,
    pub skipped_live_rows: usize,
    pub skipped_parser_rows: usize,
    pub spider_configured: bool,
    pub spider_kind: Option<TvboxConfigImplementationKind>,
    pub spider_has_integrity_digest: bool,
    pub http_endpoint_site_count: usize,
    pub spider_site_count: usize,
    pub unclassified_site_count: usize,
    pub opaque_top_level_field_names: Vec<String>,
    pub unrecognized_top_level_field_count: usize,
    pub withheld_top_level_field_count: usize,
}

/// 有界获取并解析一个配置地址的端口。
///
/// 实现负责 URL 策略、每一跳的 DNS 固定、重定向上限、响应大小上限与解析；失败必须
/// 返回带稳定错误码的 [`AppError`]，且错误里不能出现地址、重定向目标或正文片段。
#[async_trait]
pub trait TvboxConfigPreviewPort: Send + Sync {
    async fn fetch_preview(&self, url: &str) -> Result<TvboxConfigPreviewFacts, AppError>;
}

/// TVBox / FongMi 配置预览用例。
#[derive(Clone)]
pub struct TvboxConfigPreviewService {
    provider: Arc<dyn TvboxConfigPreviewPort>,
}

impl TvboxConfigPreviewService {
    pub fn new(provider: Arc<dyn TvboxConfigPreviewPort>) -> Self {
        Self { provider }
    }

    /// 校验请求 → 走端口 → 映射成 IPC 只读投影。
    ///
    /// 任何失败都原样透传端口的稳定错误码；本层只新增两个输入校验错误
    /// （空地址、超长地址），文案固定，不回显地址本身。
    pub async fn preview(
        &self,
        request: &TvboxConfigPreviewRequest,
    ) -> Result<TvboxConfigPreviewDto, AppError> {
        let url = validate_request_url(&request.url)?;
        let facts = self.provider.fetch_preview(url).await?;
        Ok(to_wire_preview(facts))
    }
}

/// 只做「非空 + 长度上限」；不在这里判断 scheme/主机/端口（那是端口的单一策略）。
fn validate_request_url(raw: &str) -> Result<&str, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(invalid_argument("TVBox 配置地址不能为空"));
    }
    if trimmed.chars().count() > MAX_CONFIG_URL_CHARS {
        return Err(invalid_argument("TVBox 配置地址过长"));
    }
    Ok(trimmed)
}

/// usize → u32。解析层的上限（1000/1000/200）远小于 u32，这里只是不 panic 的收敛。
fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// 预览事实 → IPC 只读投影。
///
/// `pub(crate)`：导入用例在保存成功后返回**同一份**摘要，不能各自再实现一遍映射，
/// 否则两条路径的脱敏规则会分叉。
pub(crate) fn to_wire_preview(facts: TvboxConfigPreviewFacts) -> TvboxConfigPreviewDto {
    TvboxConfigPreviewDto {
        schema_version: 1,
        site_count: count(facts.site_count),
        live_count: count(facts.live_count),
        parser_count: count(facts.parser_count),
        skipped_site_rows: count(facts.skipped_site_rows),
        skipped_live_rows: count(facts.skipped_live_rows),
        skipped_parser_rows: count(facts.skipped_parser_rows),
        spider_configured: facts.spider_configured,
        spider_kind: facts
            .spider_kind
            .map(TvboxConfigImplementationKind::to_wire),
        spider_has_integrity_digest: facts.spider_has_integrity_digest,
        http_endpoint_site_count: count(facts.http_endpoint_site_count),
        spider_site_count: count(facts.spider_site_count),
        unclassified_site_count: count(facts.unclassified_site_count),
        opaque_top_level_field_names: facts.opaque_top_level_field_names,
        unrecognized_top_level_field_count: count(facts.unrecognized_top_level_field_count),
        withheld_top_level_field_count: count(facts.withheld_top_level_field_count),
    }
}

fn invalid_argument(message: &'static str) -> AppError {
    AppError::new("INVALID_ARGUMENT", ErrorKind::Validation, message, false)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// 记录调用参数的 fake 端口：不触网，返回预置事实或预置错误。
    struct FakeProvider {
        calls: Mutex<Vec<String>>,
        result: Result<TvboxConfigPreviewFacts, AppError>,
    }

    impl FakeProvider {
        fn returning(facts: TvboxConfigPreviewFacts) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                result: Ok(facts),
            }
        }

        fn failing(error: AppError) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                result: Err(error),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("calls lock").clone()
        }
    }

    #[async_trait]
    impl TvboxConfigPreviewPort for FakeProvider {
        async fn fetch_preview(&self, url: &str) -> Result<TvboxConfigPreviewFacts, AppError> {
            self.calls.lock().expect("calls lock").push(url.to_owned());
            match &self.result {
                Ok(facts) => Ok(facts.clone()),
                Err(error) => Err(error.clone()),
            }
        }
    }

    fn facts() -> TvboxConfigPreviewFacts {
        TvboxConfigPreviewFacts {
            site_count: 3,
            live_count: 1,
            parser_count: 2,
            skipped_site_rows: 1,
            skipped_live_rows: 0,
            skipped_parser_rows: 0,
            spider_configured: true,
            spider_kind: Some(TvboxConfigImplementationKind::JavaScript),
            spider_has_integrity_digest: true,
            http_endpoint_site_count: 1,
            spider_site_count: 1,
            unclassified_site_count: 1,
            opaque_top_level_field_names: vec!["wallpaper".into(), "epg".into()],
            unrecognized_top_level_field_count: 4,
            withheld_top_level_field_count: 2,
        }
    }

    fn service(provider: FakeProvider) -> (TvboxConfigPreviewService, Arc<FakeProvider>) {
        let provider = Arc::new(provider);
        (TvboxConfigPreviewService::new(provider.clone()), provider)
    }

    fn request(url: &str) -> TvboxConfigPreviewRequest {
        TvboxConfigPreviewRequest { url: url.into() }
    }

    #[tokio::test]
    async fn maps_preview_facts_into_the_wire_projection() {
        let (service, provider) = service(FakeProvider::returning(facts()));
        let dto = service
            .preview(&request("https://config.example.invalid/tvbox.json"))
            .await
            .expect("端口成功即应返回投影");

        assert_eq!(dto.schema_version, 1);
        assert_eq!(dto.site_count, 3);
        assert_eq!(dto.live_count, 1);
        assert_eq!(dto.parser_count, 2);
        assert_eq!(dto.skipped_site_rows, 1);
        assert!(dto.spider_configured);
        assert_eq!(
            dto.spider_kind,
            Some(TvboxConfigImplementationKindDto::JavaScript)
        );
        assert!(dto.spider_has_integrity_digest);
        assert_eq!(dto.http_endpoint_site_count, 1);
        assert_eq!(dto.spider_site_count, 1);
        assert_eq!(dto.unclassified_site_count, 1);
        assert_eq!(
            dto.opaque_top_level_field_names,
            vec!["wallpaper".to_owned(), "epg".to_owned()]
        );
        assert_eq!(dto.unrecognized_top_level_field_count, 4);
        assert_eq!(dto.withheld_top_level_field_count, 2);
        assert_eq!(
            provider.calls(),
            vec!["https://config.example.invalid/tvbox.json".to_owned()]
        );

        // 投影本身（含 Debug 与 JSON）不得出现地址或任何配置值。
        let rendered = format!(
            "{dto:?} {}",
            serde_json::to_string(&dto).expect("投影必须可序列化")
        );
        assert!(!rendered.contains("example.invalid"), "投影泄露了地址");
    }

    #[tokio::test]
    async fn trims_the_address_before_handing_it_to_the_port() {
        let (service, provider) = service(FakeProvider::returning(facts()));
        service
            .preview(&request("  https://config.example.invalid/a.json  "))
            .await
            .expect("两侧空白应被裁掉");
        assert_eq!(
            provider.calls(),
            vec!["https://config.example.invalid/a.json".to_owned()]
        );
    }

    #[tokio::test]
    async fn rejects_blank_input_without_calling_the_port() {
        let (service, provider) = service(FakeProvider::returning(facts()));
        for url in ["", "   ", "\t\n"] {
            let error = service
                .preview(&request(url))
                .await
                .expect_err("空地址必须被拒绝");
            assert_eq!(error.code().0.as_str(), "INVALID_ARGUMENT");
            // 校验错误不得回显输入。
            let rendered = format!("{error:?} {error}");
            assert!(
                !rendered.contains("config.example.invalid"),
                "错误回显了地址: {rendered}"
            );
        }
        assert!(provider.calls().is_empty(), "被拒绝的输入不应触达端口");
    }

    #[tokio::test]
    async fn rejects_oversized_input_without_calling_the_port() {
        let (service, provider) = service(FakeProvider::returning(facts()));
        let url = format!(
            "https://example.invalid/{}",
            "a".repeat(MAX_CONFIG_URL_CHARS)
        );
        let error = service
            .preview(&request(&url))
            .await
            .expect_err("超长地址必须被拒绝");
        assert_eq!(error.code().0.as_str(), "INVALID_ARGUMENT");
        assert!(provider.calls().is_empty());

        // 边界：恰好等于上限时必须放行（长度用字符数而不是字节数计算）。
        let prefix = "https://e.invalid/";
        let exactly = format!(
            "{prefix}{}",
            "a".repeat(MAX_CONFIG_URL_CHARS - prefix.len())
        );
        assert_eq!(exactly.chars().count(), MAX_CONFIG_URL_CHARS);
        service
            .preview(&request(&exactly))
            .await
            .expect("恰好等于上限的地址应被放行");
        assert_eq!(provider.calls().len(), 1);
    }

    #[tokio::test]
    async fn passes_through_stable_port_errors_unchanged() {
        let (service, _) = service(FakeProvider::failing(AppError::new(
            "SECURITY_POLICY_DENIED",
            ErrorKind::Security,
            "TVBox 配置地址不安全",
            false,
        )));
        let error = service
            .preview(&request("http://127.0.0.1/tvbox.json"))
            .await
            .expect_err("端口失败必须透传");
        assert_eq!(error.code().0.as_str(), "SECURITY_POLICY_DENIED");
        assert!(!format!("{error:?}").contains("127.0.0.1"));
    }

    #[test]
    fn the_request_debug_never_prints_the_address() {
        let rendered = format!(
            "{:?}",
            request("https://config.example.invalid/tvbox.json?token=secret-value")
        );
        assert!(rendered.contains("<redacted>"), "请求 Debug 必须脱敏");
        for secret in ["example.invalid", "token", "secret-value"] {
            assert!(!rendered.contains(secret), "请求 Debug 不得暴露敏感字段");
        }
    }
}
