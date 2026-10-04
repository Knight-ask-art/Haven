//! TVBox / FongMi 配置的**保存/导入**用例（Film/TV Provider 基础切片第二步）。
//!
//! 它把三件已经存在的事情按固定顺序接起来：
//! 1. **[`TvboxConfigImportPort`]**：仓库既有的受控 HTTP 获取 + 解析器校验。端口返回
//!    原始字节与安全摘要；URL 策略、逐跳 DNS 固定、重定向上限、大小上限与解析规则
//!    全部在端口实现里，本模块不复制第二份。
//! 2. **[`SourceRegistryService`]**：来源身份与启用状态。`sourceId` 由注册表生成，
//!    本模块不自己拼 ID、不写 settings；注册后的来源**默认停用**（fail closed）。
//! 3. **[`SourceConfigCache`]**：把「最后一次成功获取的原始配置」写进 Rust/SQLite。
//!    缓存只认 `sourceId`，不参与身份判定，也不进共享 settings JSON。
//!
//! 顺序是有意的：
//! - 取回与解析**先于任何写入**。失败时不会注册来源，也不会覆盖已有的 last-known-good。
//! - 注册先于缓存。缓存写入失败时回滚刚完成的注册，而不是留下一个「已登记但没有配置」
//!   的来源——那种状态会让用户重试时撞上「该端点已存在」，永久卡住。
//!
//! 边界（本切片不越界）：
//! - 没有刷新、ETag、定时重取，也没有任何自动重新获取路径。
//! - 返回给 IPC 的只有 [`crate::wire::TvboxConfigPreviewDto`] 形态摘要与稳定 `sourceId`；
//!   配置地址、原始 JSON、站点端点、header 值、`ext` 内容与凭据都不在线上类型里。
//! - 不解释数字 `type`，不下载/加载/执行配置声明的 JS / JAR / Python，也不调用任何
//!   外部解析服务；这些结论仍然只由解析层给出。

use std::sync::Arc;

use async_trait::async_trait;

use haven_common::{AppError, ErrorKind, UtcMillis};

use crate::services::source_config_cache::SourceConfigCache;
use crate::services::source_registry::SourceRegistryService;
use crate::services::tvbox_config_preview::{
    MAX_CONFIG_URL_CHARS, TvboxConfigPreviewFacts, to_wire_preview,
};
use crate::wire::{TvboxConfigSaveRequest, TvboxConfigSaveResult};

/// 显示名上限（与其它自定义来源一致）。
pub const MAX_TVBOX_DISPLAY_NAME_CHARS: usize = 100;

/// 一次成功获取的结果：原始字节 + 安全摘要。
///
/// 刻意不实现 `Debug`：`body` 是不可信原文，任何 `{:?}` 都可能把它写进日志。
pub struct TvboxConfigFetch {
    body: Vec<u8>,
    facts: TvboxConfigPreviewFacts,
}

impl TvboxConfigFetch {
    pub fn new(body: Vec<u8>, facts: TvboxConfigPreviewFacts) -> Self {
        Self { body, facts }
    }

    /// 原始配置字节。仅后端使用；不得写入日志、错误或 IPC。
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn facts(&self) -> &TvboxConfigPreviewFacts {
        &self.facts
    }
}

/// 有界获取**并解析**一个配置地址的端口（导入路径专用）。
///
/// 与预览端口共用同一套网络与解析规则，但额外把原始字节交回调用方——保存需要那份
/// 原文，而预览只需要摘要。实现必须保证：只有取回与解析都成功时才返回 `Ok`；失败时
/// 返回带稳定错误码、且不含地址、重定向目标或正文片段的 [`AppError`]。
#[async_trait]
pub trait TvboxConfigImportPort: Send + Sync {
    async fn fetch_config(&self, url: &str) -> Result<TvboxConfigFetch, AppError>;
}

/// TVBox / FongMi 配置保存用例。
#[derive(Clone)]
pub struct TvboxConfigSaveService {
    importer: Arc<dyn TvboxConfigImportPort>,
    registry: SourceRegistryService,
    cache: Arc<dyn SourceConfigCache>,
}

impl TvboxConfigSaveService {
    pub fn new(
        importer: Arc<dyn TvboxConfigImportPort>,
        registry: SourceRegistryService,
        cache: Arc<dyn SourceConfigCache>,
    ) -> Self {
        Self {
            importer,
            registry,
            cache,
        }
    }

    /// 校验 → 取回并解析 → 注册来源（默认停用）→ 写入 last-known-good 原文。
    ///
    /// 回滚走 [`SourceRegistryService::discard_fresh_registration`]：刚登记的来源没有
    /// 任何凭据（`credential_ref` 为 `None`），因此这条路径不接触系统凭据库，也不会
    /// 改变 ADR-001 的删除顺序——真正带凭据的来源只走 `remove_custom_source`。
    pub async fn save(
        &self,
        request: &TvboxConfigSaveRequest,
    ) -> Result<TvboxConfigSaveResult, AppError> {
        let name = validate_display_name(&request.display_name)?;
        let url = validate_config_url(&request.url)?;

        // 1. 取回 + 解析。任何失败都在写入之前返回。
        let fetched = self.importer.fetch_config(url).await?;

        // 2. 注册来源。身份、前缀与 enabled 语义由注册表唯一决定（默认停用）。
        let added = self.registry.add_tvbox_source(name, url).await?;

        // 3. 落 last-known-good。失败则回滚注册，避免留下无法重试的半成品来源。
        if let Err(error) = self
            .cache
            .put(&added.source_id, fetched.body(), UtcMillis::now())
            .await
        {
            if let Err(rollback_error) = self
                .registry
                .discard_fresh_registration(&added.source_id)
                .await
            {
                return Err(AppError::new(
                    "DATABASE_ERROR",
                    ErrorKind::Database,
                    "配置缓存写入失败，且来源登记未能自动撤销；请检查来源列表后再重试",
                    true,
                )
                .with_source(rollback_error));
            }
            return Err(error);
        }

        Ok(TvboxConfigSaveResult {
            schema_version: 1,
            source_id: added.source_id,
            preview: to_wire_preview(fetched.facts),
        })
    }
}

/// 显示名：非空且不超上限。文案固定，不回显原文。
fn validate_display_name(raw: &str) -> Result<&str, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(invalid_argument("显示名不能为空"));
    }
    if trimmed.chars().count() > MAX_TVBOX_DISPLAY_NAME_CHARS {
        return Err(invalid_argument("显示名不能超过 100 字符"));
    }
    Ok(trimmed)
}

/// 配置地址：只做「非空 + 长度上限」。scheme、userinfo、私网/回环/单标签主机、
/// 端口白名单、重定向与 DNS 固定仍由端口实现统一执行——这里不复制第二份 URL 策略。
fn validate_config_url(raw: &str) -> Result<&str, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(invalid_argument("TVBox 配置地址不能为空"));
    }
    if trimmed.chars().count() > MAX_CONFIG_URL_CHARS {
        return Err(invalid_argument("TVBox 配置地址过长"));
    }
    Ok(trimmed)
}

fn invalid_argument(message: &'static str) -> AppError {
    AppError::new("INVALID_ARGUMENT", ErrorKind::Validation, message, false)
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, atomic::AtomicUsize};

    use async_trait::async_trait;
    use haven_common::UtcMillis;
    use haven_domain::contracts::{SettingsRepository, SettingsRow};
    use haven_domain::credential::CredentialStore;
    use haven_domain::credential::SecretString;
    use haven_domain::ids::CredentialRef;
    use haven_infrastructure::Db;
    use haven_infrastructure::db::repos::SqliteRepositories;

    use super::*;
    use crate::services::source_config_cache::InMemorySourceConfigCache;
    use crate::services::source_registry::CustomSourceKind;

    const CONFIG_BODY: &str =
        r#"{"sites":[{"name":"示例站点","api":"https://api.example.invalid/vod"}]}"#;

    fn facts() -> TvboxConfigPreviewFacts {
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
        }
    }

    /// 记录调用参数的 fake 端口：不触网，返回预置结果或预置错误。
    struct FakeImporter {
        calls: Mutex<Vec<String>>,
        result: Result<TvboxConfigFetch, AppError>,
    }

    impl FakeImporter {
        fn returning(body: &str) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                result: Ok(TvboxConfigFetch::new(body.as_bytes().to_vec(), facts())),
            }
        }

        fn failing(code: &str, kind: ErrorKind) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                result: Err(AppError::new(code, kind, "固定的安全文案", false)),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().expect("calls lock").clone()
        }
    }

    #[async_trait]
    impl TvboxConfigImportPort for FakeImporter {
        async fn fetch_config(&self, url: &str) -> Result<TvboxConfigFetch, AppError> {
            self.calls.lock().expect("calls lock").push(url.to_owned());
            match &self.result {
                Ok(fetched) => Ok(TvboxConfigFetch::new(
                    fetched.body().to_vec(),
                    fetched.facts().clone(),
                )),
                Err(error) => Err(error.clone()),
            }
        }
    }

    /// 失败的缓存：`put` 永远返回错误，其余委托给内存实现。
    struct FailingPutCache(InMemorySourceConfigCache);

    #[async_trait]
    impl SourceConfigCache for FailingPutCache {
        async fn put(
            &self,
            _source_id: &str,
            _body: &[u8],
            _fetched_at: UtcMillis,
        ) -> Result<(), AppError> {
            Err(AppError::new(
                "DATABASE_ERROR",
                ErrorKind::Internal,
                "写入来源配置缓存失败",
                false,
            ))
        }

        async fn get(
            &self,
            source_id: &str,
        ) -> Result<Option<crate::services::source_config_cache::CachedSourceConfig>, AppError>
        {
            self.0.get(source_id).await
        }

        async fn delete(&self, source_id: &str) -> Result<bool, AppError> {
            self.0.delete(source_id).await
        }
    }

    /// 第二次 settings 写入失败，用来验证来源注册的补偿失败不会被吞掉。
    struct FailRollbackSettings {
        inner: Arc<SqliteRepositories>,
        writes: AtomicUsize,
    }

    #[async_trait]
    impl SettingsRepository for FailRollbackSettings {
        async fn get(&self, section: &str) -> Result<Option<SettingsRow>, AppError> {
            SettingsRepository::get(self.inner.as_ref(), section).await
        }

        async fn upsert(&self, row: &SettingsRow) -> Result<(), AppError> {
            if self
                .writes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                == 1
            {
                return Err(AppError::new(
                    "DATABASE_ERROR",
                    ErrorKind::Database,
                    "测试用持久化失败",
                    true,
                ));
            }
            SettingsRepository::upsert(self.inner.as_ref(), row).await
        }
    }

    struct MemoryStore;

    #[async_trait]
    impl CredentialStore for MemoryStore {
        async fn set(
            &self,
            _target: &CredentialRef,
            _secret: &SecretString,
        ) -> Result<(), AppError> {
            Ok(())
        }
        async fn get(&self, _target: &CredentialRef) -> Result<Option<SecretString>, AppError> {
            Ok(None)
        }
        async fn delete(&self, _target: &CredentialRef) -> Result<bool, AppError> {
            Ok(false)
        }
    }

    fn registry() -> SourceRegistryService {
        let db = Arc::new(Db::open_in_memory().expect("内存库必须可迁移"));
        SourceRegistryService::new(Arc::new(SqliteRepositories::new(db)))
    }

    fn service(
        importer: FakeImporter,
        cache: Arc<dyn SourceConfigCache>,
    ) -> (
        TvboxConfigSaveService,
        Arc<FakeImporter>,
        SourceRegistryService,
    ) {
        let registry = registry();
        let importer = Arc::new(importer);
        (
            TvboxConfigSaveService::new(importer.clone(), registry.clone(), cache),
            importer,
            registry,
        )
    }

    fn request(display_name: &str, url: &str) -> TvboxConfigSaveRequest {
        TvboxConfigSaveRequest {
            display_name: display_name.into(),
            url: url.into(),
        }
    }

    #[tokio::test]
    async fn a_valid_save_registers_a_disabled_source_and_caches_the_raw_config() {
        let cache = Arc::new(InMemorySourceConfigCache::new());
        let (service, importer, registry) =
            service(FakeImporter::returning(CONFIG_BODY), cache.clone());

        let result = service
            .save(&request(
                "我的电视源",
                "https://config.example.invalid/tvbox.json",
            ))
            .await
            .expect("合法配置应保存成功");

        assert_eq!(result.schema_version, 1);
        assert!(
            result.source_id.starts_with("custom_tvbox_"),
            "sourceId 必须由注册表按 TVBox 前缀生成: {}",
            result.source_id
        );
        assert_eq!(result.preview.site_count, 1);
        assert_eq!(result.preview.schema_version, 1);

        // 取回时地址被裁掉两侧空白后原样传给端口。
        assert_eq!(
            importer.calls(),
            vec!["https://config.example.invalid/tvbox.json".to_owned()]
        );

        // 注册表持有身份与启用状态：默认停用。
        let list = registry.list().await.expect("注册表必须可读");
        let descriptor = list
            .sources
            .iter()
            .find(|source| source.source_id == result.source_id)
            .expect("来源必须已登记");
        assert!(!descriptor.enabled, "新登记的 TVBox 来源必须默认停用");
        assert_eq!(descriptor.display_name, "我的电视源");

        // 原文按 sourceId 落在缓存里，且与取回字节逐字一致。
        let cached = cache
            .get(&result.source_id)
            .await
            .expect("缓存读取不应失败")
            .expect("保存成功后必须存在 last-known-good");
        assert_eq!(cached.body(), CONFIG_BODY.as_bytes());
        assert!(cached.fetched_at().0 > 0);
    }

    #[tokio::test]
    async fn a_fetch_failure_registers_nothing_and_leaves_the_cache_untouched() {
        let cache = Arc::new(InMemorySourceConfigCache::new());
        cache
            .put("custom_tvbox_existing", b"{}", UtcMillis(1))
            .await
            .unwrap();
        let (service, _, registry) = service(
            FakeImporter::failing("SOURCE_UNAVAILABLE", ErrorKind::Network),
            cache.clone(),
        );

        let error = service
            .save(&request(
                "电视源",
                "https://config.example.invalid/tvbox.json",
            ))
            .await
            .expect_err("取回失败必须整体失败");
        assert_eq!(error.code().0.as_str(), "SOURCE_UNAVAILABLE");
        assert!(!format!("{error:?}").contains("example.invalid"));

        // 没有新来源，且已存在的缓存没有被覆盖。
        let list = registry.list().await.unwrap();
        assert!(
            !list
                .sources
                .iter()
                .any(|source| source.source_id.starts_with("custom_tvbox_")),
            "失败路径不得注册来源"
        );
        let untouched = cache.get("custom_tvbox_existing").await.unwrap().unwrap();
        assert_eq!(untouched.body(), b"{}");
        assert_eq!(untouched.fetched_at(), UtcMillis(1));
    }

    #[tokio::test]
    async fn a_parse_failure_registers_nothing() {
        let cache = Arc::new(InMemorySourceConfigCache::new());
        // 解析失败由端口以解析层的稳定错误码返回；用例不依赖正文内容。
        let (service, _, registry) = service(
            FakeImporter::failing("TVBOX_CONFIG_INVALID_JSON", ErrorKind::Parse),
            cache,
        );

        let error = service
            .save(&request(
                "电视源",
                "https://config.example.invalid/tvbox.json",
            ))
            .await
            .expect_err("解析失败必须整体失败");
        assert_eq!(error.code().0.as_str(), "TVBOX_CONFIG_INVALID_JSON");
        assert!(registry.list().await.unwrap().sources.iter().all(|source| {
            !matches!(source.source_id.as_str(), id if id.starts_with("custom_tvbox_"))
        }));
    }

    #[tokio::test]
    async fn a_duplicate_endpoint_is_rejected_without_touching_the_first_cache_entry() {
        let cache = Arc::new(InMemorySourceConfigCache::new());
        let registry = registry();
        let importer = Arc::new(FakeImporter::returning(CONFIG_BODY));
        let service = TvboxConfigSaveService::new(importer, registry.clone(), cache.clone());
        let url = "https://config.example.invalid/tvbox.json";

        let first = service
            .save(&request("第一个", url))
            .await
            .expect("首次保存应成功");
        let second = service
            .save(&request("第二个", url))
            .await
            .expect_err("同一端点的第二次登记必须被拒绝");
        assert_eq!(second.code().0.as_str(), "INVALID_ARGUMENT");

        // 原有来源与它的 last-known-good 都保持第一次的样子。
        let list = registry.list().await.unwrap();
        let tvbox: Vec<&str> = list
            .sources
            .iter()
            .filter(|source| source.source_id.starts_with("custom_tvbox_"))
            .map(|source| source.source_id.as_str())
            .collect();
        assert_eq!(tvbox, vec![first.source_id.as_str()]);
        assert_eq!(
            cache.get(&first.source_id).await.unwrap().unwrap().body(),
            CONFIG_BODY.as_bytes()
        );
    }

    #[tokio::test]
    async fn a_cache_write_failure_rolls_the_registration_back() {
        let cache = Arc::new(FailingPutCache(InMemorySourceConfigCache::new()));
        let (service, _, registry) = service(FakeImporter::returning(CONFIG_BODY), cache);

        let error = service
            .save(&request(
                "电视源",
                "https://config.example.invalid/tvbox.json",
            ))
            .await
            .expect_err("缓存写入失败必须整体失败");
        assert_eq!(error.code().0.as_str(), "DATABASE_ERROR");

        // 回滚后没有留下半成品来源，用户可以用同一个地址重试。
        assert!(
            registry
                .list()
                .await
                .unwrap()
                .sources
                .iter()
                .all(|source| { !source.source_id.starts_with("custom_tvbox_") })
        );
    }

    #[tokio::test]
    async fn a_registry_rollback_failure_is_reported_and_leaves_the_source_recoverable() {
        let db = Arc::new(Db::open_in_memory().expect("内存库必须可迁移"));
        let inner = Arc::new(SqliteRepositories::new(db));
        let settings = Arc::new(FailRollbackSettings {
            inner,
            writes: AtomicUsize::new(0),
        });
        let registry = SourceRegistryService::new(settings);
        let cache = Arc::new(FailingPutCache(InMemorySourceConfigCache::new()));
        let importer = Arc::new(FakeImporter::returning(CONFIG_BODY));
        let service = TvboxConfigSaveService::new(importer, registry.clone(), cache.clone());

        let error = service
            .save(&request(
                "电视源",
                "https://config.example.invalid/tvbox.json?token=secret",
            ))
            .await
            .expect_err("补偿失败必须返回错误");
        assert_eq!(error.code().as_str(), "DATABASE_ERROR");
        assert!(error.user_message().contains("未能自动撤销"));
        assert!(!format!("{error:?}").contains("example.invalid"));
        assert!(!format!("{error:?}").contains("secret"));

        let list = registry.list().await.expect("失败后仍可读取来源列表");
        let tvbox_sources: Vec<_> = list
            .sources
            .iter()
            .filter(|source| source.source_id.starts_with("custom_tvbox_"))
            .collect();
        assert_eq!(tvbox_sources.len(), 1, "未成功回滚的来源必须仍可见并清理");
        assert!(!tvbox_sources[0].enabled, "失败路径中的来源仍须默认停用");

        registry
            .remove_custom_source(&tvbox_sources[0].source_id, &MemoryStore, cache.as_ref())
            .await
            .expect("用户仍可通过既有删除路径清理来源");
    }

    #[tokio::test]
    async fn rejects_blank_and_oversized_input_without_calling_the_port() {
        let cache = Arc::new(InMemorySourceConfigCache::new());
        let (service, importer, _) = service(FakeImporter::returning(CONFIG_BODY), cache);

        for (name, url) in [
            ("", "https://config.example.invalid/tvbox.json"),
            ("   ", "https://config.example.invalid/tvbox.json"),
            ("电视源", ""),
            ("电视源", "   "),
        ] {
            let error = service
                .save(&request(name, url))
                .await
                .expect_err("非法输入必须被拒绝");
            assert_eq!(error.code().0.as_str(), "INVALID_ARGUMENT");
        }

        let long_name = "名".repeat(MAX_TVBOX_DISPLAY_NAME_CHARS + 1);
        let error = service
            .save(&request(
                &long_name,
                "https://config.example.invalid/tvbox.json",
            ))
            .await
            .expect_err("超长显示名必须被拒绝");
        assert_eq!(error.code().0.as_str(), "INVALID_ARGUMENT");

        let prefix = "https://e.invalid/";
        let long_url = format!(
            "{prefix}{}",
            "a".repeat(MAX_CONFIG_URL_CHARS - prefix.len() + 1)
        );
        let error = service
            .save(&request("电视源", &long_url))
            .await
            .expect_err("超长地址必须被拒绝");
        assert_eq!(error.code().0.as_str(), "INVALID_ARGUMENT");
        assert!(
            !format!("{error:?}").contains("e.invalid"),
            "错误不得回显地址"
        );

        assert!(importer.calls().is_empty(), "被拒绝的输入不应触达端口");
    }

    #[tokio::test]
    async fn the_removal_path_cleans_the_cached_config() {
        let cache = Arc::new(InMemorySourceConfigCache::new());
        let (service, _, registry) = service(FakeImporter::returning(CONFIG_BODY), cache.clone());

        let saved = service
            .save(&request(
                "电视源",
                "https://config.example.invalid/tvbox.json",
            ))
            .await
            .expect("保存应成功");
        assert!(cache.get(&saved.source_id).await.unwrap().is_some());

        registry
            .remove_custom_source(&saved.source_id, &MemoryStore, cache.as_ref())
            .await
            .expect("删除应成功");

        assert!(
            cache.get(&saved.source_id).await.unwrap().is_none(),
            "来源删除后不得留下原始配置"
        );
        assert!(
            !registry
                .list()
                .await
                .unwrap()
                .sources
                .iter()
                .any(|source| source.source_id == saved.source_id)
        );
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
        for secret in ["example.invalid", "token", "secret-value"] {
            assert!(!rendered.contains(secret), "请求 Debug 泄露了 {secret}");
        }
    }

    #[tokio::test]
    async fn the_result_never_carries_the_address_or_the_raw_config() {
        let cache = Arc::new(InMemorySourceConfigCache::new());
        let (service, _, _) = service(FakeImporter::returning(CONFIG_BODY), cache);

        let result = service
            .save(&request(
                "电视源",
                "https://config.example.invalid/a.json?token=secret-value",
            ))
            .await
            .expect("保存应成功");

        let rendered = format!(
            "{result:?} {}",
            serde_json::to_string(&result).expect("结果必须可序列化")
        );
        for secret in [
            "example.invalid",
            "secret-value",
            "api.example.invalid",
            "示例站点",
            "sites",
        ] {
            assert!(
                !rendered.contains(secret),
                "结果泄露了 {secret}: {rendered}"
            );
        }
    }

    #[tokio::test]
    async fn the_tvbox_kind_is_registered_without_any_search_capability() {
        let cache = Arc::new(InMemorySourceConfigCache::new());
        let (service, _, registry) = service(FakeImporter::returning(CONFIG_BODY), cache);
        let saved = service
            .save(&request(
                "电视源",
                "https://config.example.invalid/tvbox.json",
            ))
            .await
            .unwrap();

        let list = registry.list().await.unwrap();
        let descriptor = list
            .sources
            .iter()
            .find(|source| source.source_id == saved.source_id)
            .unwrap();
        assert!(
            descriptor.kinds.is_empty(),
            "本切片没有任何已实现的搜索/读取能力，kinds 必须为空而不是伪造能力"
        );
        assert_eq!(
            descriptor.categories,
            vec![crate::wire::SourceCategoryDto::Video]
        );
        assert!(
            !descriptor.notes.contains("config.example.invalid"),
            "来源说明不得回显端点"
        );
        // 记录本身仍然带着 kind，删除/凭据等分支据此工作。
        assert_eq!(
            registry.custom_source_kind(&saved.source_id).await.unwrap(),
            Some(CustomSourceKind::Tvbox)
        );
    }
}
