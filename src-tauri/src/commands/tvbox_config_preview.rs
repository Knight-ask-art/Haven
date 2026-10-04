//! `tvbox_config_preview`：TVBox / FongMi 配置的只读预览（Film/TV Provider 基础切片）。
//!
//! 命令只做三件事：转发请求 → Application 用例 → 把稳定错误映射成 `ErrorDto`。
//! 边界（ADR-002：Command 不得写 SQL、不得自建服务）：
//! - 不落库、不缓存、不注册来源、不改 SourceRegistry，也没有任何保存入口。
//! - 响应只有 Application 已经过滤过的形态摘要；配置地址、原始 JSON、站点端点、
//!   header 值、`ext` 内容、Cookie 与 Token 都不在线上类型里。
//! - 不打印、不 Debug 请求里的地址（它可能带 token）；校验与文案都由 Application
//!   用例给出，命令层不复制第二份规则。
//! - 不下载、不加载、不执行配置声明的 JS / JAR / Python，也不调用外部解析服务。

use tauri::State;

use haven_application::services::tvbox_config_preview::TvboxConfigPreviewService;
use haven_application::wire::{ErrorDto, TvboxConfigPreviewDto, TvboxConfigPreviewRequest};

use crate::ipc::{run_blocking, to_error_dto};
use crate::state::AppState;

/// 命令核心逻辑（脱离 Tauri runtime 可测：校验、端口调用与错误映射都在这里）。
pub async fn run_tvbox_config_preview(
    service: &TvboxConfigPreviewService,
    request: TvboxConfigPreviewRequest,
) -> Result<TvboxConfigPreviewDto, ErrorDto> {
    service
        .preview(&request)
        .await
        .map_err(|error| to_error_dto(&error))
}

/// Tauri Command 薄包装。
#[tauri::command]
pub async fn tvbox_config_preview(
    state: State<'_, AppState>,
    request: TvboxConfigPreviewRequest,
) -> Result<TvboxConfigPreviewDto, ErrorDto> {
    let service = state.tvbox_config_preview.clone();
    run_blocking(move || async move { run_tvbox_config_preview(&service, request).await }).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use haven_application::services::tvbox_config_preview::{
        TvboxConfigImplementationKind, TvboxConfigPreviewFacts, TvboxConfigPreviewPort,
    };
    use haven_application::wire::TvboxConfigImplementationKindDto;
    use haven_common::{AppError, ErrorKind};

    use super::*;

    struct FakeProvider {
        result: Result<TvboxConfigPreviewFacts, AppError>,
    }

    impl FakeProvider {
        fn returning(facts: TvboxConfigPreviewFacts) -> Arc<dyn TvboxConfigPreviewPort> {
            Arc::new(Self { result: Ok(facts) })
        }

        fn failing(error: AppError) -> Arc<dyn TvboxConfigPreviewPort> {
            Arc::new(Self { result: Err(error) })
        }
    }

    #[async_trait]
    impl TvboxConfigPreviewPort for FakeProvider {
        async fn fetch_preview(&self, _url: &str) -> Result<TvboxConfigPreviewFacts, AppError> {
            // 有意忽略入参：本文件的测试只验证命令层的映射，不依赖地址内容。
            match &self.result {
                Ok(facts) => Ok(facts.clone()),
                Err(error) => Err(error.clone()),
            }
        }
    }

    fn facts() -> TvboxConfigPreviewFacts {
        TvboxConfigPreviewFacts {
            site_count: 2,
            live_count: 1,
            parser_count: 0,
            skipped_site_rows: 0,
            skipped_live_rows: 0,
            skipped_parser_rows: 0,
            spider_configured: true,
            spider_kind: Some(TvboxConfigImplementationKind::Jar),
            spider_has_integrity_digest: false,
            http_endpoint_site_count: 1,
            spider_site_count: 1,
            unclassified_site_count: 0,
            opaque_top_level_field_names: Vec::new(),
            unrecognized_top_level_field_count: 0,
            withheld_top_level_field_count: 0,
        }
    }

    fn service(result: Result<TvboxConfigPreviewFacts, AppError>) -> TvboxConfigPreviewService {
        let provider = match result {
            Ok(facts) => FakeProvider::returning(facts),
            Err(error) => FakeProvider::failing(error),
        };
        TvboxConfigPreviewService::new(provider)
    }

    fn request(url: &str) -> TvboxConfigPreviewRequest {
        TvboxConfigPreviewRequest { url: url.into() }
    }

    #[tokio::test]
    async fn maps_the_service_result_into_the_wire_response() {
        let service = service(Ok(facts()));
        let dto =
            run_tvbox_config_preview(&service, request("https://config.example.invalid/a.json"))
                .await
                .expect("成功路径应返回投影");

        assert_eq!(dto.schema_version, 1);
        assert_eq!(dto.site_count, 2);
        assert_eq!(dto.spider_kind, Some(TvboxConfigImplementationKindDto::Jar));
    }

    #[tokio::test]
    async fn rejects_invalid_input_with_a_structured_error_that_never_echoes_the_address() {
        let blank_service = service(Ok(facts()));
        for url in ["", "   "] {
            let error = run_tvbox_config_preview(&blank_service, request(url))
                .await
                .expect_err("空地址必须被拒绝");
            assert_eq!(error.code, "INVALID_ARGUMENT");
            assert!(!error.retryable);
            assert!(!format!("{error:?}").contains("example.invalid"));
        }

        // 超长地址同样在进入端口之前被拒。
        let oversized_service = service(Ok(facts()));
        let error = run_tvbox_config_preview(&oversized_service, request(&"a".repeat(4096)))
            .await
            .expect_err("超长地址必须被拒绝");
        assert_eq!(error.code, "INVALID_ARGUMENT");
    }

    #[tokio::test]
    async fn maps_stable_backend_errors_into_error_dto() {
        for (code, kind, retryable) in [
            ("SECURITY_POLICY_DENIED", ErrorKind::Security, false),
            ("SOURCE_UNAVAILABLE", ErrorKind::Network, true),
            ("TVBOX_CONFIG_INVALID_JSON", ErrorKind::Parse, false),
        ] {
            let service = service(Err(AppError::new(code, kind, "固定的安全文案", retryable)));
            let error = run_tvbox_config_preview(
                &service,
                request("https://config.example.invalid/a.json"),
            )
            .await
            .expect_err("后端失败必须映射成 ErrorDto");
            assert_eq!(error.code, code);
            assert_eq!(error.retryable, retryable);
            // 稳定文案里不得出现被拒绝的地址。
            assert!(!format!("{error:?}").contains("example.invalid"));
        }
    }

    #[test]
    fn the_request_type_never_prints_the_address() {
        let rendered = format!(
            "{:?}",
            request("https://config.example.invalid/a.json?token=secret-value")
        );
        assert!(rendered.contains("<redacted>"), "请求 Debug 必须脱敏");
        assert!(!rendered.contains("secret-value"));
    }
}
