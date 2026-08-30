//! Persistent Artwork Cache。
//!
//! `image_proxy.id` remains the stable opaque identity.  This module only owns
//! reproducible bytes under the application Cache root and the SQLite index;
//! it never exposes an absolute path or a remote URL to IPC.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use fast_webp::{WebpOptions, encode_dynamic_image};
use haven_application::services::cache::ArtworkCacheClearPort;
use haven_application::services::trending::ArtworkCachePort;
use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::contracts::ImageProxyRepository;
use image::{DynamicImage, ImageFormat, ImageReader, imageops::FilterType};
use reqwest::header::{ETAG, LAST_MODIFIED, LOCATION};
use tokio::io::AsyncWriteExt;
use tokio::time::sleep;

use crate::db::Db;
use crate::db::repos::image_proxy::{
    ImageProxyRecord, SqliteImageProxyRepository, legacy_source_for_url, validate_registration,
};

#[path = "artwork_network.rs"]
mod artwork_network;
use artwork_network::ArtworkNetwork;

pub const MAX_ORIGINAL_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_IMAGE_DIMENSION: u32 = 8_192;
pub const MAX_IMAGE_PIXELS: u64 = 40_000_000;
pub const ARTWORK_FRESH_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1_000;
pub const ARTWORK_STALE_IF_ERROR_MS: i64 = 90 * 24 * 60 * 60 * 1_000;
pub const ARTWORK_CACHE_SOFT_LIMIT_BYTES: i64 = 512 * 1024 * 1024;
const ARTWORK_VARIANT_WEBP_QUALITY: f32 = 82.0;

const MAX_REDIRECTS: usize = 3;
const MAX_FETCH_ATTEMPTS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtworkVariant {
    Original,
    Width200,
    Width400,
}

impl ArtworkVariant {
    pub fn key(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Width200 => "w200",
            Self::Width400 => "w400",
        }
    }

    pub fn width(self) -> Option<u32> {
        match self {
            Self::Original => None,
            Self::Width200 => Some(200),
            Self::Width400 => Some(400),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtworkResponse {
    pub bytes: Vec<u8>,
    pub mime: String,
}

#[derive(Debug, Clone)]
struct CacheRow {
    relative_path: String,
    mime: String,
    content_hash: String,
    byte_size: i64,
    expires_at: i64,
    stale_if_error_until: i64,
}

#[derive(Debug)]
struct RemotePayload {
    bytes: Vec<u8>,
    etag: Option<String>,
    last_modified: Option<String>,
}

struct CacheMetadata<'a> {
    mime: &'a str,
    etag: Option<&'a str>,
    last_modified: Option<&'a str>,
    now: i64,
}

#[derive(Clone)]
pub struct ArtworkCache {
    db: std::sync::Arc<Db>,
    proxy: std::sync::Arc<SqliteImageProxyRepository>,
    root: PathBuf,
    network: ArtworkNetwork,
}

impl ArtworkCache {
    pub fn new(db: std::sync::Arc<Db>, root: PathBuf) -> Result<Self, AppError> {
        Ok(Self {
            proxy: std::sync::Arc::new(SqliteImageProxyRepository::new(db.clone())),
            db,
            root,
            network: ArtworkNetwork::new()?,
        })
    }

    pub fn default_root(db: &Db) -> PathBuf {
        db.path()
            .and_then(|path| path.parent().map(|parent| parent.join("Cache")))
            .unwrap_or_else(|| std::env::temp_dir().join("haven-cache"))
    }

    /// 清理全部 Artwork Cache 文件与索引，但保留 `image_proxy` 稳定身份和来源映射。
    /// 缺失文件视为已清理；任何数据库/文件错误只返回稳定错误，不泄漏路径。
    pub async fn clear_all(&self) -> Result<u64, AppError> {
        let rows = {
            let conn = self.db.lock();
            let mut statement = conn
                .prepare("SELECT relative_path FROM image_cache_entries")
                .map_err(crate::db::repos::map_db_error("读取图片缓存索引失败"))?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(crate::db::repos::map_db_error("读取图片缓存索引失败"))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(crate::db::repos::map_db_error("读取图片缓存索引失败"))?
        };

        for relative in &rows {
            if let Some(path) = safe_cache_path(&self.root, relative) {
                match tokio::fs::remove_file(path).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(artwork_io(error)),
                }
            }
        }

        let conn = self.db.lock();
        conn.execute("DELETE FROM image_cache_entries", [])
            .map_err(crate::db::repos::map_db_error("清理图片缓存索引失败"))?;
        Ok(rows.len() as u64)
    }

    pub async fn load(
        &self,
        artwork_id: &str,
        variant: ArtworkVariant,
    ) -> Result<ArtworkResponse, AppError> {
        let record = self
            .proxy
            .resolve_record(artwork_id)?
            .ok_or_else(artwork_not_found)?;
        let record = self.recover_legacy_record(record)?;
        let now = UtcMillis::now().0;

        if let Some(cached) = self.read_cached(artwork_id, variant, now).await? {
            if cached.fresh {
                return Ok(cached.response);
            }
        }

        // Legacy rows created before migration 021 remain readable only when a
        // verified local file already exists.  Their unknown source policy must
        // never trigger a new network request.
        if record.source_id.is_none() {
            return self
                .read_cached(artwork_id, variant, now)
                .await?
                .filter(|cached| cached.within_stale_window)
                .map(|cached| cached.response)
                .ok_or_else(artwork_not_found);
        }

        // A missing variant can be rebuilt from a still-valid original without
        // touching the network.
        if variant != ArtworkVariant::Original {
            if let Some(original) = self
                .read_cached(artwork_id, ArtworkVariant::Original, now)
                .await?
            {
                if original.within_stale_window {
                    match self
                        .generate_variant(artwork_id, variant, &original.response.bytes)
                        .await
                    {
                        Ok(response) => return Ok(response),
                        Err(_) if !original.fresh => return Ok(original.response),
                        Err(_) => {}
                    }
                }
            }
        }

        match self.fetch_and_store(&record, artwork_id).await {
            Ok(original) => {
                if variant == ArtworkVariant::Original {
                    Ok(original)
                } else {
                    self.generate_variant(artwork_id, variant, &original.bytes)
                        .await
                }
            }
            Err(error) => {
                // stale-if-error: an already verified old file wins over a
                // transient provider outage or invalid refresh response.
                self.read_cached(artwork_id, variant, now)
                    .await?
                    .filter(|cached| cached.within_stale_window)
                    .map(|cached| cached.response)
                    .ok_or(error)
            }
        }
    }

    /// 让 021 之前的已知来源在运行时惰性自愈，覆盖未经过 022 迁移的
    /// 外部数据库副本/导入库，同时保持未知来源 fail closed。
    fn recover_legacy_record(
        &self,
        mut record: ImageProxyRecord,
    ) -> Result<ImageProxyRecord, AppError> {
        if record.source_id.is_none()
            && let Some(source_id) = legacy_source_for_url(&record.target_url)
        {
            self.proxy.backfill_known_source_if_missing(
                &record.id,
                source_id,
                &record.target_url,
            )?;
            if let Some(updated) = self.proxy.resolve_record(&record.id)? {
                record = updated;
            }
        }
        Ok(record)
    }

    async fn fetch_and_store(
        &self,
        record: &ImageProxyRecord,
        artwork_id: &str,
    ) -> Result<ArtworkResponse, AppError> {
        let result = self.fetch_remote(record).await;
        let payload = match result {
            Ok(payload) => payload,
            Err(error) => {
                let _ =
                    self.proxy
                        .mark_fetch_state(artwork_id, "failed", Some(error.code().as_str()));
                return Err(error);
            }
        };

        let (mime, width, height) = match validate_image_bytes_async(payload.bytes.clone()).await {
            Ok(value) => value,
            Err(error) => {
                let _ =
                    self.proxy
                        .mark_fetch_state(artwork_id, "failed", Some(error.code().as_str()));
                return Err(error);
            }
        };
        let now = UtcMillis::now().0;
        self.store_entry(
            artwork_id,
            ArtworkVariant::Original,
            &payload.bytes,
            CacheMetadata {
                mime,
                etag: payload.etag.as_deref(),
                last_modified: payload.last_modified.as_deref(),
                now,
            },
        )
        .await?;
        // Decode/resize is deliberately outside the async runtime.  The
        // decoded dimensions are checked before any transformation.
        let _ = (width, height);
        self.proxy.mark_fetch_state(artwork_id, "ready", None)?;
        self.evict(Some(artwork_id)).await;
        Ok(ArtworkResponse {
            bytes: payload.bytes,
            mime: mime.to_owned(),
        })
    }

    async fn generate_variant(
        &self,
        artwork_id: &str,
        variant: ArtworkVariant,
        original: &[u8],
    ) -> Result<ArtworkResponse, AppError> {
        let width = variant
            .width()
            .ok_or_else(|| artwork_format("图片变体宽度无效"))?;
        let source = original.to_vec();
        let encoded = tokio::task::spawn_blocking(move || encode_webp_variant(&source, width))
            .await
            .map_err(|_| artwork_internal("图片变体任务失败"))??;
        let now = UtcMillis::now().0;
        self.store_entry(
            artwork_id,
            variant,
            &encoded,
            CacheMetadata {
                mime: "image/webp",
                etag: None,
                last_modified: None,
                now,
            },
        )
        .await?;
        self.evict(Some(artwork_id)).await;
        Ok(ArtworkResponse {
            bytes: encoded,
            mime: "image/webp".to_owned(),
        })
    }

    async fn fetch_remote(&self, record: &ImageProxyRecord) -> Result<RemotePayload, AppError> {
        let mut last_error = None;
        for attempt in 0..MAX_FETCH_ATTEMPTS {
            match self.fetch_remote_once(record).await {
                Ok(payload) => return Ok(payload),
                Err(error)
                    if attempt + 1 < MAX_FETCH_ATTEMPTS && is_retryable_fetch_error(&error) =>
                {
                    last_error = Some(error);
                    sleep(Duration::from_millis(180)).await;
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error.unwrap_or_else(|| artwork_network("图片来源暂时不可用")))
    }

    async fn fetch_remote_once(
        &self,
        record: &ImageProxyRecord,
    ) -> Result<RemotePayload, AppError> {
        let _fetch_slot = self
            .network
            .acquire_fetch_slot()
            .await
            .ok_or_else(|| artwork_internal("图片抓取并发控制不可用"))?;
        let mut current = record
            .target_url
            .parse::<reqwest::Url>()
            .map_err(|_| artwork_security("图片来源地址无效"))?;

        for redirect_count in 0..=MAX_REDIRECTS {
            self.network
                .validate_remote_url(
                    &current,
                    record.source_id.as_deref(),
                    record.normalized_host.as_deref(),
                )
                .await?;
            let response = self
                .network
                .request(&current, record.source_id.as_deref())
                .send()
                .await
                .map_err(map_fetch_error)?;

            if response.status().is_redirection() {
                if redirect_count == MAX_REDIRECTS {
                    return Err(artwork_security("图片跳转次数超过限制"));
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or_else(|| artwork_security("图片跳转地址无效"))?;
                current = current
                    .join(location)
                    .map_err(|_| artwork_security("图片跳转地址无效"))?;
                continue;
            }
            if !response.status().is_success() {
                return Err(artwork_network("图片来源暂时不可用"));
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_ORIGINAL_BYTES as u64)
            {
                return Err(artwork_too_large());
            }
            let etag = response
                .headers()
                .get(ETAG)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let last_modified = response
                .headers()
                .get(LAST_MODIFIED)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let mut bytes = Vec::new();
            let mut response = response;
            while let Some(chunk) = response.chunk().await.map_err(map_fetch_error)? {
                if bytes.len() + chunk.len() > MAX_ORIGINAL_BYTES {
                    return Err(artwork_too_large());
                }
                bytes.extend_from_slice(&chunk);
            }
            return Ok(RemotePayload {
                bytes,
                etag,
                last_modified,
            });
        }
        Err(artwork_security("图片跳转地址无效"))
    }

    async fn read_cached(
        &self,
        artwork_id: &str,
        variant: ArtworkVariant,
        now: i64,
    ) -> Result<Option<CachedResponse>, AppError> {
        let row = self.cache_row(artwork_id, variant)?;
        let Some(row) = row else { return Ok(None) };
        let Some(path) = safe_cache_path(&self.root, &row.relative_path) else {
            // A stale or manually edited index row is cache corruption, not a
            // protocol-fatal condition.  Remove it and let the caller rebuild
            // from the registered source (or use another stale variant).
            self.delete_cache_row(artwork_id, variant)?;
            return Ok(None);
        };
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.delete_cache_row(artwork_id, variant)?;
                return Ok(None);
            }
            Err(error) => return Err(artwork_io(error)),
        };
        if bytes.is_empty()
            || bytes.len() > MAX_ORIGINAL_BYTES
            || bytes.len() as i64 != row.byte_size
            || sha256_hex(&bytes) != row.content_hash
        {
            self.delete_cache_row(artwork_id, variant)?;
            return Ok(None);
        }
        // The write path already performed bounded header/dimension parsing and
        // full decoding.  A warm read only needs the indexed hash and magic
        // MIME check; decoding every cached hit would reintroduce the first
        // paint latency this cache is intended to remove.
        if infer_image_mime(&bytes) != Some(row.mime.as_str()) {
            self.delete_cache_row(artwork_id, variant)?;
            return Ok(None);
        }
        self.touch_cache_row(artwork_id, variant, now)?;
        Ok(Some(CachedResponse {
            response: ArtworkResponse {
                bytes,
                mime: row.mime,
            },
            fresh: now <= row.expires_at,
            within_stale_window: now <= row.stale_if_error_until,
        }))
    }

    fn cache_row(
        &self,
        artwork_id: &str,
        variant: ArtworkVariant,
    ) -> Result<Option<CacheRow>, AppError> {
        let conn = self.db.lock();
        rusqlite::OptionalExtension::optional(conn.query_row(
            "SELECT relative_path, mime, content_hash, byte_size, expires_at, stale_if_error_until
             FROM image_cache_entries WHERE artwork_id = ?1 AND variant = ?2",
            rusqlite::params![artwork_id, variant.key()],
            |row| {
                Ok(CacheRow {
                    relative_path: row.get(0)?,
                    mime: row.get(1)?,
                    content_hash: row.get(2)?,
                    byte_size: row.get(3)?,
                    expires_at: row.get(4)?,
                    stale_if_error_until: row.get(5)?,
                })
            },
        ))
        .map_err(crate::db::repos::map_db_error("读取图片缓存索引失败"))
    }

    async fn store_entry(
        &self,
        artwork_id: &str,
        variant: ArtworkVariant,
        bytes: &[u8],
        metadata: CacheMetadata<'_>,
    ) -> Result<(), AppError> {
        let hash = sha256_hex(bytes);
        let relative_path = format!("images/{}/{hash}", &hash[..2]);
        let path = safe_cache_path(&self.root, &relative_path)
            .ok_or_else(|| artwork_security("图片缓存路径无效"))?;
        if tokio::fs::metadata(&path).await.is_err() {
            let parent = path
                .parent()
                .ok_or_else(|| artwork_io_message("图片缓存目录无效"))?;
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(artwork_io)?;
            let temporary = parent.join(format!(".{hash}.{}.part", uuid::Uuid::new_v4()));
            let mut file = tokio::fs::File::create(&temporary)
                .await
                .map_err(artwork_io)?;
            file.write_all(bytes).await.map_err(artwork_io)?;
            file.flush().await.map_err(artwork_io)?;
            drop(file);
            if let Err(error) = tokio::fs::rename(&temporary, &path).await {
                let _ = tokio::fs::remove_file(&temporary).await;
                if tokio::fs::metadata(&path).await.is_err() {
                    return Err(artwork_io(error));
                }
            }
        }
        let conn = self.db.lock();
        conn.execute(
            "INSERT INTO image_cache_entries
                (artwork_id, variant, relative_path, mime, content_hash, byte_size,
                 etag, last_modified, last_accessed_at, expires_at, stale_if_error_until)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(artwork_id, variant) DO UPDATE SET
                relative_path = excluded.relative_path,
                mime = excluded.mime,
                content_hash = excluded.content_hash,
                byte_size = excluded.byte_size,
                etag = excluded.etag,
                last_modified = excluded.last_modified,
                last_accessed_at = excluded.last_accessed_at,
                expires_at = excluded.expires_at,
                stale_if_error_until = excluded.stale_if_error_until",
            rusqlite::params![
                artwork_id,
                variant.key(),
                relative_path,
                metadata.mime,
                hash,
                bytes.len() as i64,
                metadata.etag,
                metadata.last_modified,
                metadata.now,
                metadata.now + ARTWORK_FRESH_TTL_MS,
                metadata.now + ARTWORK_STALE_IF_ERROR_MS,
            ],
        )
        .map_err(crate::db::repos::map_db_error("写入图片缓存索引失败"))?;
        Ok(())
    }

    fn touch_cache_row(
        &self,
        artwork_id: &str,
        variant: ArtworkVariant,
        now: i64,
    ) -> Result<(), AppError> {
        let conn = self.db.lock();
        conn.execute(
            "UPDATE image_cache_entries SET last_accessed_at = ?3
             WHERE artwork_id = ?1 AND variant = ?2",
            rusqlite::params![artwork_id, variant.key(), now],
        )
        .map_err(crate::db::repos::map_db_error("更新图片缓存访问时间失败"))?;
        Ok(())
    }

    fn delete_cache_row(&self, artwork_id: &str, variant: ArtworkVariant) -> Result<(), AppError> {
        let conn = self.db.lock();
        conn.execute(
            "DELETE FROM image_cache_entries WHERE artwork_id = ?1 AND variant = ?2",
            rusqlite::params![artwork_id, variant.key()],
        )
        .map_err(crate::db::repos::map_db_error("清理图片缓存索引失败"))?;
        Ok(())
    }

    async fn evict(&self, protected: Option<&str>) {
        let rows = {
            let conn = self.db.lock();
            let total: i64 = conn
                .query_row(
                    "SELECT COALESCE(SUM(byte_size), 0) FROM image_cache_entries",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            if total <= ARTWORK_CACHE_SOFT_LIMIT_BYTES {
                return;
            }
            let mut statement = match conn.prepare(
                "SELECT artwork_id, variant, relative_path, byte_size
                 FROM image_cache_entries ORDER BY expires_at ASC, last_accessed_at ASC",
            ) {
                Ok(statement) => statement,
                Err(_) => return,
            };
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .ok()
                .and_then(|mapped| mapped.collect::<Result<Vec<_>, _>>().ok())
                .unwrap_or_default()
        };
        let mut remaining = rows.iter().map(|(_, _, _, bytes)| *bytes).sum::<i64>();
        for (artwork_id, variant, relative_path, bytes) in rows {
            if remaining <= ARTWORK_CACHE_SOFT_LIMIT_BYTES {
                break;
            }
            if protected.is_some_and(|value| value == artwork_id) {
                continue;
            }
            if let Some(path) = safe_cache_path(&self.root, &relative_path) {
                let _ = tokio::fs::remove_file(path).await;
            }
            let conn = self.db.lock();
            let _ = conn.execute(
                "DELETE FROM image_cache_entries WHERE artwork_id = ?1 AND variant = ?2",
                rusqlite::params![artwork_id, variant],
            );
            remaining -= bytes;
        }
    }
}

#[async_trait]
impl ArtworkCachePort for ArtworkCache {
    async fn register(&self, source_id: &str, target_url: &str) -> Result<String, AppError> {
        let _ = validate_registration(source_id, target_url)?;
        self.proxy.register(source_id, target_url).await
    }

    async fn prewarm(&self, artwork_id: &str) -> Result<(), AppError> {
        let _ = self.load(artwork_id, ArtworkVariant::Width200).await?;
        Ok(())
    }
}

#[async_trait]
impl ArtworkCacheClearPort for ArtworkCache {
    async fn clear_all(&self) -> Result<u64, AppError> {
        ArtworkCache::clear_all(self).await
    }
}

#[derive(Debug)]
struct CachedResponse {
    response: ArtworkResponse,
    fresh: bool,
    within_stale_window: bool,
}

fn validate_image_bytes(bytes: &[u8]) -> Result<(&'static str, u32, u32), AppError> {
    if bytes.len() > MAX_ORIGINAL_BYTES {
        return Err(artwork_too_large());
    }
    let mime = infer_image_mime(bytes).ok_or_else(|| artwork_format("图片格式不受支持"))?;
    if mime == "image/gif" {
        return Err(artwork_format("不支持 GIF 图片"));
    }
    let format = match mime {
        "image/jpeg" => ImageFormat::Jpeg,
        "image/png" => ImageFormat::Png,
        "image/webp" => ImageFormat::WebP,
        _ => return Err(artwork_format("图片格式不受支持")),
    };
    // Read dimensions from the bounded header before decoding pixels.  This
    // rejects oversized/compression-bomb inputs without first allocating their
    // full decoded buffer.
    let (width, height) = ImageReader::with_format(Cursor::new(bytes), format)
        .into_dimensions()
        .map_err(|_| artwork_format("图片尺寸读取失败"))?;
    if width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || (width as u64) * (height as u64) > MAX_IMAGE_PIXELS
    {
        return Err(artwork_format("图片尺寸超过安全限制"));
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_PIXELS.saturating_mul(4));
    reader.limits(limits);
    reader
        .decode()
        .map_err(|_| artwork_format("图片解码失败"))?;
    Ok((mime, width, height))
}

async fn validate_image_bytes_async(bytes: Vec<u8>) -> Result<(&'static str, u32, u32), AppError> {
    tokio::task::spawn_blocking(move || validate_image_bytes(&bytes))
        .await
        .map_err(|_| artwork_internal("图片校验任务失败"))?
}

fn encode_webp_variant(bytes: &[u8], width: u32) -> Result<Vec<u8>, AppError> {
    let image = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| artwork_format("图片格式不受支持"))?
        .decode()
        .map_err(|_| artwork_format("图片解码失败"))?;
    let resized = resize_to_width(image, width);
    encode_dynamic_image(
        &resized,
        WebpOptions {
            quality: ARTWORK_VARIANT_WEBP_QUALITY,
            lossless: false,
        },
    )
    .map_err(|_| artwork_format("图片变体编码失败"))
}

fn resize_to_width(image: DynamicImage, width: u32) -> DynamicImage {
    if image.width() <= width {
        image
    } else {
        image.resize(width, u32::MAX, FilterType::Lanczos3)
    }
}

fn infer_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else {
        None
    }
}

fn safe_cache_path(root: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty()
        || relative.contains('\\')
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || Path::new(relative).is_absolute()
    {
        return None;
    }
    let path = root.join(relative);
    let _ = path.strip_prefix(root).ok()?;
    Some(path)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

fn map_fetch_error(error: reqwest::Error) -> AppError {
    AppError::new(
        "ARTWORK_FETCH_FAILED",
        if error.is_timeout() {
            ErrorKind::Timeout
        } else {
            ErrorKind::Network
        },
        "图片来源暂时不可用",
        true,
    )
}

fn is_retryable_fetch_error(error: &AppError) -> bool {
    // Only transport/upstream failures are retried.  Security, format, size,
    // cache and protocol errors remain fail-closed and are never retried here.
    matches!(
        error.code().as_str(),
        "ARTWORK_FETCH_FAILED" | "ARTWORK_DNS_PROOF_FAILED"
    )
}

fn artwork_not_found() -> AppError {
    AppError::new(
        "ARTWORK_NOT_FOUND",
        ErrorKind::NotFound,
        "图片资源不存在",
        false,
    )
}

fn artwork_network(message: &'static str) -> AppError {
    AppError::new("ARTWORK_FETCH_FAILED", ErrorKind::Network, message, true)
}

fn artwork_security(message: &'static str) -> AppError {
    AppError::new(
        "SECURITY_POLICY_DENIED",
        ErrorKind::Security,
        message,
        false,
    )
}

fn artwork_format(message: &'static str) -> AppError {
    AppError::new(
        "ARTWORK_FORMAT_UNSUPPORTED",
        ErrorKind::Parse,
        message,
        false,
    )
}

fn artwork_too_large() -> AppError {
    AppError::new(
        "ARTWORK_TOO_LARGE",
        ErrorKind::Validation,
        "图片超过大小限制",
        false,
    )
}

fn artwork_io(error: std::io::Error) -> AppError {
    AppError::new(
        "ARTWORK_CACHE_IO_FAILED",
        ErrorKind::Storage,
        "图片缓存读写失败",
        true,
    )
    .with_source(error)
}

fn artwork_io_message(message: &'static str) -> AppError {
    AppError::new("ARTWORK_CACHE_IO_FAILED", ErrorKind::Storage, message, true)
}

fn artwork_internal(message: &'static str) -> AppError {
    AppError::new("ARTWORK_CACHE_INTERNAL", ErrorKind::Internal, message, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbaImage};
    use std::sync::Arc;

    fn tiny_png() -> Vec<u8> {
        let image = RgbaImage::from_pixel(12, 18, image::Rgba([20, 40, 60, 255]));
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    fn alpha_png() -> Vec<u8> {
        let image = RgbaImage::from_fn(24, 16, |x, y| {
            image::Rgba([x as u8 * 7, y as u8 * 11, 80, if x == 0 { 0 } else { 128 }])
        });
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn webp_variant_uses_quality_82_and_roundtrips_dimensions() {
        assert_eq!(ARTWORK_VARIANT_WEBP_QUALITY, 82.0);
        let encoded = encode_webp_variant(&tiny_png(), 200).unwrap();
        assert!(encoded.starts_with(b"RIFF"));
        assert_eq!(&encoded[8..12], b"WEBP");
        assert!(
            !encoded.windows(4).any(|chunk| chunk == b"VP8L"),
            "列表变体必须使用 lossy WebP，而不是无损 VP8L"
        );

        let decoded = ImageReader::new(Cursor::new(encoded))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!((decoded.width(), decoded.height()), (12, 18));
    }

    #[test]
    fn webp_variant_preserves_alpha_channel() {
        let encoded = encode_webp_variant(&alpha_png(), 200).unwrap();
        let decoded = ImageReader::new(Cursor::new(encoded))
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap()
            .to_rgba8();
        assert!(decoded.pixels().any(|pixel| pixel[3] < 255));
    }

    #[test]
    fn webp_variant_rejects_malformed_input_with_safe_error() {
        let error = encode_webp_variant(b"not an image", 200).unwrap_err();
        assert_eq!(error.code().as_str(), "ARTWORK_FORMAT_UNSUPPORTED");
    }

    #[test]
    fn image_magic_policy_rejects_svg_html_and_gif() {
        assert!(validate_image_bytes(b"<svg></svg>").is_err());
        assert!(validate_image_bytes(b"<html>bad</html>").is_err());
        assert!(validate_image_bytes(b"GIF89a").is_err());

        let oversized = RgbaImage::new(MAX_IMAGE_DIMENSION + 1, 1);
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(oversized)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        assert!(validate_image_bytes(&bytes.into_inner()).is_err());
    }

    #[test]
    fn cache_path_is_relative_and_cannot_escape_root() {
        let root = Path::new("C:/app/Cache");
        assert!(safe_cache_path(root, "images/aa/hash").is_some());
        assert!(safe_cache_path(root, "../secret").is_none());
        assert!(safe_cache_path(root, "C:/secret").is_none());
        assert!(safe_cache_path(root, "images\\aa\\hash").is_none());
    }

    #[tokio::test]
    async fn remote_policy_rejects_private_literal_ip_and_signed_query() {
        let network = ArtworkNetwork::new().unwrap();
        let private = "http://127.0.0.1/a.jpg".parse().unwrap();
        assert!(
            network
                .validate_remote_url(&private, Some("cms10"), Some("127.0.0.1"))
                .await
                .is_err()
        );
        let literal_fake = "http://198.18.0.1/a.jpg".parse().unwrap();
        let error = network
            .validate_remote_url(&literal_fake, Some("cms10"), Some("198.18.0.1"))
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "SECURITY_POLICY_DENIED");
        let unknown_host = "https://img.example.com/a.jpg".parse().unwrap();
        assert!(
            network
                .validate_remote_url(&unknown_host, Some("cms10"), None)
                .await
                .is_err()
        );
        let signed = "https://img.doubanio.com/a.jpg?sig=abc".parse().unwrap();
        assert!(
            network
                .validate_remote_url(&signed, Some("douban"), Some("img.doubanio.com"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn fresh_cache_hit_and_variant_generation_do_not_need_provider() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let root = tempfile::tempdir().unwrap();
        let cache = ArtworkCache::new(db, root.path().to_path_buf()).unwrap();
        let artwork_id = cache
            .proxy
            .register("cms10", "https://invalid.invalid/poster.jpg")
            .await
            .unwrap();
        let original = tiny_png();
        cache
            .store_entry(
                &artwork_id,
                ArtworkVariant::Original,
                &original,
                CacheMetadata {
                    mime: "image/png",
                    etag: None,
                    last_modified: None,
                    now: UtcMillis::now().0,
                },
            )
            .await
            .unwrap();

        let hit = cache
            .load(&artwork_id, ArtworkVariant::Original)
            .await
            .unwrap();
        assert_eq!(hit.mime, "image/png");
        assert_eq!(hit.bytes, original);

        let variant = cache
            .load(&artwork_id, ArtworkVariant::Width200)
            .await
            .unwrap();
        assert_eq!(variant.mime, "image/webp");
        assert!(variant.bytes.starts_with(b"RIFF"));
        assert!(
            cache
                .cache_row(&artwork_id, ArtworkVariant::Width200)
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn missing_cache_file_is_treated_as_miss_and_index_is_removed() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let root = tempfile::tempdir().unwrap();
        let cache = ArtworkCache::new(db, root.path().to_path_buf()).unwrap();
        let artwork_id = cache
            .proxy
            .register("cms10", "https://invalid.invalid/poster.jpg")
            .await
            .unwrap();
        let original = tiny_png();
        cache
            .store_entry(
                &artwork_id,
                ArtworkVariant::Original,
                &original,
                CacheMetadata {
                    mime: "image/png",
                    etag: None,
                    last_modified: None,
                    now: UtcMillis::now().0,
                },
            )
            .await
            .unwrap();
        let row = cache
            .cache_row(&artwork_id, ArtworkVariant::Original)
            .unwrap()
            .unwrap();
        let path = safe_cache_path(&cache.root, &row.relative_path).unwrap();
        tokio::fs::remove_file(path).await.unwrap();
        assert!(
            cache
                .load(&artwork_id, ArtworkVariant::Original)
                .await
                .is_err()
        );
        assert!(
            cache
                .cache_row(&artwork_id, ArtworkVariant::Original)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn legacy_source_without_policy_only_reads_verified_local_cache() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let root = tempfile::tempdir().unwrap();
        let cache = ArtworkCache::new(db.clone(), root.path().to_path_buf()).unwrap();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO image_proxy (id, target_url, created_at)
                 VALUES ('legacy-artwork', 'file:///not-an-allowed-source', 0)",
                [],
            )
            .unwrap();
        }
        let original = tiny_png();
        cache
            .store_entry(
                "legacy-artwork",
                ArtworkVariant::Original,
                &original,
                CacheMetadata {
                    mime: "image/png",
                    etag: None,
                    last_modified: None,
                    now: UtcMillis::now().0,
                },
            )
            .await
            .unwrap();

        let response = cache
            .load("legacy-artwork", ArtworkVariant::Original)
            .await
            .unwrap();
        assert_eq!(response.bytes, original);
    }

    #[test]
    fn legacy_known_source_is_recovered_without_allowing_unknown_host() {
        let db = Arc::new(Db::open_in_memory().unwrap());
        let cache = ArtworkCache::new(db.clone(), std::env::temp_dir()).unwrap();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO image_proxy (id, target_url, created_at)
                 VALUES ('legacy-known', 'https://img.picbf.com/poster.jpg', 0)",
                [],
            )
            .unwrap();
        }
        let known = ImageProxyRecord {
            id: "legacy-known".to_owned(),
            source_id: None,
            target_url: "https://img.picbf.com/poster.jpg".to_owned(),
            normalized_host: None,
        };
        let recovered = cache.recover_legacy_record(known).unwrap();
        assert_eq!(recovered.source_id.as_deref(), Some("cms10"));
        assert_eq!(recovered.normalized_host.as_deref(), Some("img.picbf.com"));

        let unknown = ImageProxyRecord {
            id: "legacy-unknown".to_owned(),
            source_id: None,
            target_url: "https://images.example.invalid/poster.jpg".to_owned(),
            normalized_host: None,
        };
        let unchanged = cache.recover_legacy_record(unknown).unwrap();
        assert_eq!(unchanged.source_id, None);
        assert_eq!(unchanged.normalized_host, None);
    }
}
