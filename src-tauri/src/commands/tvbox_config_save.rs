//! `tvbox_config_save`：TVBox / FongMi 配置的保存/导入（Film/TV Provider 基础切片）。
//!
//! 命令只做三件事：不可信输入校验 → Application 用例 → 把稳定错误映射成 `ErrorDto`。
//!
//! 边界（ADR-002：Command 不得写 SQL、不得自建服务）：
//! - 不自己生成 sourceId、不直接读写 settings，也不碰 SQLite：来源身份与启用状态由
//!   `SourceRegistryService` 唯一拥有，原文缓存由 Application 经端口写入。
//! - 响应用的是与预览同一份形态摘要（计数、分类、白名单字段名、第三方实现种类），
//!   外加一个稳定 sourceId。配置地址、原始 JSON、站点端点、header 值、`ext` 内容、
//!   Cookie 与 Token 都不在线上类型里。
//! - 不打印、不 Debug 请求里的地址（它可能带 token）；校验文案固定，不回显输入。
//! - 不下载、不加载、不执行配置声明的 JS / JAR / Python，也不调用外部解析服务。
//! - 登记后的来源**默认停用**，且本切片不提供启用入口。

use tauri::State;

use haven_application::services::tvbox_config_import::TvboxConfigSaveService;
use haven_application::services::tvbox_config_preview::MAX_CONFIG_URL_CHARS;
use haven_application::wire::{ErrorDto, TvboxConfigSaveRequest, TvboxConfigSaveResult};

use crate::ipc::{invalid_argument, run_blocking, to_error_dto};
use crate::state::AppState;

/// 显示名上限（与其它自定义来源一致）。
const MAX_DISPLAY_NAME_CHARS: usize = 100;

/// 命令核心逻辑（脱离 Tauri runtime 可测：校验、用例调用与错误映射都在这里）。
pub async fn run_tvbox_config_save(
    service: &TvboxConfigSaveService,
    request: TvboxConfigSaveRequest,
) -> Result<TvboxConfigSaveResult, ErrorDto> {
    // 廉价输入校验：拒绝空白与超长输入，错误文案固定、不回显输入。权威判定仍在
    // Application（那里还会用注册表与 URL 策略再校验一次）。
    if request.display_name.trim().is_empty()
        || request.display_name.trim().chars().count() > MAX_DISPLAY_NAME_CHARS
    {
        return Err(invalid_argument("显示名不能为空且不超过 100 字符"));
    }
    let url = request.url.trim();
    if url.is_empty() {
        return Err(invalid_argument("TVBox 配置地址不能为空"));
    }
    if url.chars().count() > MAX_CONFIG_URL_CHARS {
        return Err(invalid_argument("TVBox 配置地址过长"));
    }

    service
        .save(&request)
        .await
        .map_err(|error| to_error_dto(&error))
}

/// Tauri Command 薄包装。
#[tauri::command]
pub async fn tvbox_config_save(
    state: State<'_, AppState>,
    request: TvboxConfigSaveRequest,
) -> Result<TvboxConfigSaveResult, ErrorDto> {
    let service = state.tvbox_config_save.clone();
    run_blocking(move || async move { run_tvbox_config_save(&service, request).await }).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use haven_application::services::source_config_cache::InMemorySourceConfigCache;
    use haven_application::services::source_registry::SourceRegistryService;
    use haven_application::services::tvbox_config_import::{
        TvboxConfigFetch, TvboxConfigImportPort,
    };
    use haven_application::services::tvbox_config_preview::TvboxConfigPreviewFacts;
    use haven_infrastructure::db::repos::SqliteRepositories;
    use haven_infrastructure::Db;

    use super::*;

    struct FakeImporter {
        result: Result<TvboxConfigFetch, haven_common::AppError>,
    }

    impl FakeImporter {
        fn returning(body: &str) -> Arc<dyn TvboxConfigImportPort> {
            Arc::new(Self {
                result: Ok(TvboxConfigFetch::new(
                    body.as_bytes().to_vec(),
                    TvboxConfigPreviewFacts {
                        site_count: 1,
                        live_count: 0,
                        parser_count: 0,
                        skipped_site_rows: 0,
                        skipped_live_rows: 0,
                        skipped_parser_rows: 0,
                        spider_configured: false,
                        spider_kind: None,
                        spider_has_integrity_digest: false,
                        http_endpoint_site_count: 1,
                        spider_site_count: 0,
                        unclassified_site_count: 0,
                        opaque_top_level_field_names: Vec::new(),
                        unrecognized_top_level_field_count: 0,
                        withheld_top_level_field_count: 0,
                    },
                )),
            })
        }

        fn failing() -> Arc<dyn TvboxConfigImportPort> {
            Arc::new(Self {
                result: Err(haven_common::AppError::new(
                    "SOURCE_UNAVAILABLE",
                    haven_common::ErrorKind::Network,
                    "TVBox 配置暂时不可达",
                    true,
                )),
            })
        }
    }

    #[async_trait]
    impl TvboxConfigImportPort for FakeImporter {
        async fn fetch_config(
            &self,
            _url: &str,
        ) -> Result<TvboxConfigFetch, haven_common::AppError> {
            match &self.result {
                Ok(fetched) => Ok(TvboxConfigFetch::new(
                    fetched.body().to_vec(),
                    fetched.facts().clone(),
                )),
                Err(error) => Err(error.clone()),
            }
        }
    }

    fn service(importer: Arc<dyn TvboxConfigImportPort>) -> TvboxConfigSaveService {
        let db = Arc::new(Db::open_in_memory().expect("内存库必须可迁移"));
        let repos = Arc::new(SqliteRepositories::new(db));
        TvboxConfigSaveService::new(
            importer,
            SourceRegistryService::new(repos),
            Arc::new(InMemorySourceConfigCache::new()),
        )
    }

    fn request(display_name: &str, url: &str) -> TvboxConfigSaveRequest {
        TvboxConfigSaveRequest {
            display_name: display_name.into(),
            url: url.into(),
        }
    }

    #[tokio::test]
    async fn maps_a_successful_save_into_the_wire_response() {
        let service = service(FakeImporter::returning(r#"{"sites":[]}"#));
        let result = run_tvbox_config_save(
            &service,
            request("我的电视源", "https://config.example.invalid/tvbox.json"),
        )
        .await
        .expect("成功路径应返回来源 ID 与摘要");

        assert_eq!(result.schema_version, 1);
        assert!(result.source_id.starts_with("custom_tvbox_"));
        assert_eq!(result.preview.site_count, 1);
    }

    #[tokio::test]
    async fn rejects_blank_and_oversized_input_without_reaching_the_service() {
        // 端口必然失败：这些输入必须在更早的地方被拦下。
        let service = service(FakeImporter::failing());
        for bad in [
            request("", "https://config.example.invalid/a.json"),
            request("   ", "https://config.example.invalid/a.json"),
            request("电视源", ""),
            request("电视源", "   "),
            request(
                &"名".repeat(MAX_DISPLAY_NAME_CHARS + 1),
                "https://e.invalid/a.json",
            ),
            request("电视源", &format!("https://e.invalid/{}", "a".repeat(3000))),
        ] {
            let error = run_tvbox_config_save(&service, bad)
                .await
                .expect_err("非法输入必须被拒绝");
            assert_eq!(error.code, "INVALID_ARGUMENT");
            assert!(!error.retryable);
            // 校验错误不得回显地址。
            assert!(!format!("{error:?}").contains("e.invalid"));
        }
    }

    #[tokio::test]
    async fn maps_stable_backend_errors_into_error_dto() {
        let service = service(FakeImporter::failing());
        let error = run_tvbox_config_save(
            &service,
            request("电视源", "https://config.example.invalid/a.json"),
        )
        .await
        .expect_err("后端失败必须映射成 ErrorDto");
        assert_eq!(error.code, "SOURCE_UNAVAILABLE");
        assert!(error.retryable);
        assert!(!format!("{error:?}").contains("example.invalid"));
    }

    #[test]
    fn the_request_type_never_prints_the_address() {
        let rendered = format!(
            "{:?}",
            request(
                "电视源",
                "https://config.example.invalid/a.json?token=secret-value"
            )
        );
        assert!(rendered.contains("<redacted>"), "请求 Debug 必须脱敏");
        assert!(!rendered.contains("secret-value"));
        assert!(!rendered.contains("example.invalid"));
    }
}
