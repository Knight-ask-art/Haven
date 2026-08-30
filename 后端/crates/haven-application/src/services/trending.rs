//! 搜索页热榜的应用服务与技术缓存端口。
//!
//! 热榜是可删除、可重建的技术缓存，不是 Work/Edition/MediaItem 业务事实。
//! Query 只读取缓存；Refresh 才访问 Provider、登记 Artwork 并写回缓存。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use haven_common::{AppError, ErrorKind, UtcMillis};
use tokio::sync::watch;

use crate::wire::{TrendingBoardDto, TrendingBoardsDto, TrendingItemDto};

pub const TRENDING_SOURCE_ID: &str = "douban";
pub const TRENDING_FRESH_TTL_MS: i64 = 6 * 60 * 60 * 1_000;
pub const TRENDING_STALE_WINDOW_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
pub const TRENDING_REFRESH_BUDGET: Duration = Duration::from_secs(5);
pub const TRENDING_REFRESH_COOLDOWN: Duration = Duration::from_secs(60);

pub const CANONICAL_BOARD_IDS: [&str; 4] = ["anime", "cn_drama", "variety", "us_drama"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteArtworkCandidate {
    pub source_id: String,
    pub target_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrendingItemCandidate {
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub poster: Option<RemoteArtworkCandidate>,
    pub status_badge: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrendingBoardCandidate {
    pub board_id: String,
    pub title: String,
    pub subtitle: String,
    pub items: Vec<TrendingItemCandidate>,
}

#[async_trait]
pub trait TrendingProvider: Send + Sync {
    /// 只请求缺失或过期榜单；Infrastructure 负责并发限制和网络预算。
    async fn fetch_boards(
        &self,
        board_ids: &[String],
    ) -> Result<Vec<TrendingBoardCandidate>, AppError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrendingBoardCacheEntry {
    pub board: TrendingBoardDto,
    pub source_id: String,
    pub revision: String,
    pub refreshed_at: i64,
    pub expires_at: i64,
}

#[async_trait]
pub trait TrendingCachePort: Send + Sync {
    async fn list(&self) -> Result<Vec<TrendingBoardCacheEntry>, AppError>;
    async fn upsert(&self, entry: &TrendingBoardCacheEntry) -> Result<(), AppError>;
}

#[async_trait]
pub trait ArtworkCachePort: Send + Sync {
    /// 返回稳定的 `image_proxy.id`；远端 URL 不进入 Wire DTO。
    async fn register(&self, source_id: &str, target_url: &str) -> Result<String, AppError>;

    /// 低优先级预热列表首屏变体。预热失败不得影响热榜 DTO。
    async fn prewarm(&self, _artwork_id: &str) -> Result<(), AppError> {
        Ok(())
    }
}

#[derive(Default)]
struct RefreshGateState {
    in_flight: bool,
    completion: Option<Arc<watch::Sender<u64>>>,
    last_result: Option<Result<TrendingBoardsDto, AppError>>,
    cooldown_until: Option<Instant>,
}

#[derive(Debug, Clone)]
struct RefreshOutcome {
    dto: TrendingBoardsDto,
    warning: Option<AppError>,
}

#[derive(Clone)]
pub struct TrendingService {
    provider: Arc<dyn TrendingProvider>,
    cache: Arc<dyn TrendingCachePort>,
    artwork: Arc<dyn ArtworkCachePort>,
    gate: Arc<Mutex<RefreshGateState>>,
}

impl TrendingService {
    pub fn new(
        provider: Arc<dyn TrendingProvider>,
        cache: Arc<dyn TrendingCachePort>,
        artwork: Arc<dyn ArtworkCachePort>,
    ) -> Self {
        Self {
            provider,
            cache,
            artwork,
            gate: Arc::new(Mutex::new(RefreshGateState::default())),
        }
    }

    /// 本地 Query：不访问 Provider，不隐式写缓存。无可用快照返回空榜。
    pub async fn boards(&self) -> Result<TrendingBoardsDto, AppError> {
        let entries = self.cache.list().await?;
        Ok(self.dto_from_entries(entries, UtcMillis::now().0))
    }

    /// 显式技术刷新：检查 TTL，单飞访问 Provider，按榜单合并并写回缓存。
    pub async fn refresh(&self) -> Result<TrendingBoardsDto, AppError> {
        let (waiter, immediate) = {
            let mut gate = self
                .gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if gate.in_flight {
                let receiver = gate.completion.as_ref().map(|sender| sender.subscribe());
                (receiver, None)
            } else if gate
                .cooldown_until
                .is_some_and(|until| until > Instant::now())
            {
                let result = gate
                    .last_result
                    .clone()
                    .unwrap_or_else(|| Err(refresh_failed("热榜刷新正在冷却，请稍后重试")));
                (None, Some(result))
            } else {
                gate.in_flight = true;
                gate.cooldown_until = None;
                let (sender, _receiver) = watch::channel(0_u64);
                gate.completion = Some(Arc::new(sender));
                (None, None)
            }
        };

        if let Some(mut receiver) = waiter {
            // The watch receiver is subscribed while holding the gate lock, so
            // a fast completion cannot be lost between observing `in_flight`
            // and awaiting the result.
            let _ = receiver.changed().await;
            return self
                .gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .last_result
                .clone()
                .unwrap_or_else(|| Err(refresh_failed("热榜刷新未返回结果")));
        }
        if let Some(result) = immediate {
            return result;
        }

        let attempt = self.refresh_once().await;
        let result = attempt
            .as_ref()
            .map(|outcome| outcome.dto.clone())
            .map_err(Clone::clone);
        let notify = {
            let mut gate = self
                .gate
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            gate.last_result = Some(result.clone());
            gate.in_flight = false;
            if attempt
                .as_ref()
                .map_or(true, |outcome| outcome.warning.is_some())
            {
                gate.cooldown_until = Some(Instant::now() + TRENDING_REFRESH_COOLDOWN);
            }
            gate.completion.take()
        };
        if let Some(sender) = notify {
            let next = (*sender.borrow()).saturating_add(1);
            let _ = sender.send(next);
        }
        result
    }

    async fn refresh_once(&self) -> Result<RefreshOutcome, AppError> {
        let now = UtcMillis::now().0;
        let entries = self.cache.list().await?;
        let by_id: BTreeMap<String, TrendingBoardCacheEntry> = entries
            .into_iter()
            .filter(|entry| entry.refreshed_at >= now - TRENDING_STALE_WINDOW_MS)
            .map(|entry| (entry.board.board_id.clone(), entry))
            .collect();
        let missing_or_expired = CANONICAL_BOARD_IDS
            .iter()
            .filter(|board_id| {
                by_id
                    .get(**board_id)
                    .is_none_or(|entry| entry.expires_at <= now)
            })
            .map(|board_id| (*board_id).to_owned())
            .collect::<Vec<_>>();

        if !missing_or_expired.is_empty() {
            let candidates = match tokio::time::timeout(
                TRENDING_REFRESH_BUDGET,
                self.provider.fetch_boards(&missing_or_expired),
            )
            .await
            {
                Ok(Ok(candidates)) => candidates,
                Ok(Err(error)) => {
                    // A provider-wide outage must not hide a still-usable
                    // local snapshot.  Return the best snapshot directly;
                    // only a cold start has to surface the retryable error.
                    let stale = self.dto_from_entries(by_id.values().cloned().collect(), now);
                    if !stale.boards.is_empty() {
                        return Ok(RefreshOutcome {
                            dto: stale,
                            warning: Some(error),
                        });
                    }
                    return Err(error);
                }
                Err(_) => {
                    let stale = self.dto_from_entries(by_id.values().cloned().collect(), now);
                    if !stale.boards.is_empty() {
                        return Ok(RefreshOutcome {
                            dto: stale,
                            warning: Some(refresh_timeout()),
                        });
                    }
                    return Err(refresh_timeout());
                }
            };
            if candidates.is_empty() {
                let stale = self.dto_from_entries(by_id.values().cloned().collect(), now);
                if !stale.boards.is_empty() {
                    return Ok(RefreshOutcome {
                        dto: stale,
                        warning: Some(refresh_failed("热榜来源暂时没有可用数据")),
                    });
                }
                return Err(refresh_failed("暂无可用热榜，网络恢复后可重试"));
            }
            let mut prepared_any = false;
            for candidate in candidates {
                let Some(board) = self.prepare_board(candidate).await else {
                    continue;
                };
                prepared_any = true;
                serde_json::to_string(&board).map_err(|error| {
                    AppError::new(
                        "TRENDING_CACHE_SERIALIZE_FAILED",
                        ErrorKind::Parse,
                        "热榜缓存序列化失败",
                        false,
                    )
                    .with_source(error)
                })?;
                let entry = TrendingBoardCacheEntry {
                    board,
                    source_id: TRENDING_SOURCE_ID.to_owned(),
                    revision: format!("{}-{}", now, uuid::Uuid::new_v4()),
                    refreshed_at: now,
                    expires_at: now + TRENDING_FRESH_TTL_MS,
                };
                self.cache.upsert(&entry).await?;
                for artwork_id in entry
                    .board
                    .items
                    .iter()
                    .take(2)
                    .filter_map(|item| item.poster_uri.as_deref())
                    .filter_map(|uri| uri.strip_prefix("haven://artwork/"))
                    .map(str::to_owned)
                {
                    let artwork = self.artwork.clone();
                    tokio::spawn(async move {
                        let _ = artwork.prewarm(&artwork_id).await;
                    });
                }
            }
            if !prepared_any {
                let stale = self.dto_from_entries(by_id.values().cloned().collect(), now);
                if !stale.boards.is_empty() {
                    return Ok(RefreshOutcome {
                        dto: stale,
                        warning: Some(refresh_failed("热榜来源返回了无效数据")),
                    });
                }
                return Err(refresh_failed("暂无可用热榜，网络恢复后可重试"));
            }
        }

        let dto = self.boards().await?;
        if dto.boards.is_empty() {
            Err(refresh_failed("暂无可用热榜，网络恢复后可重试"))
        } else {
            Ok(RefreshOutcome { dto, warning: None })
        }
    }

    async fn prepare_board(&self, candidate: TrendingBoardCandidate) -> Option<TrendingBoardDto> {
        if !CANONICAL_BOARD_IDS.contains(&candidate.board_id.as_str()) {
            return None;
        }
        let title = bounded_text(&candidate.title, 80)?;
        let subtitle = bounded_text(&candidate.subtitle, 80)?;
        let mut items = Vec::new();
        for item in candidate.items.into_iter().take(10) {
            let Some(title) = bounded_text(&item.title, 200) else {
                continue;
            };
            // Subtitle/description are display-only fields.  A provider may
            // omit either one; keep the valid item instead of discarding its
            // entire board, while still applying the length limit.
            let subtitle = bounded_text(&item.subtitle, 200).unwrap_or_default();
            let description = bounded_text(&item.description, 80).unwrap_or_default();
            let poster_uri = match item.poster {
                Some(remote) => self
                    .artwork
                    .register(&remote.source_id, &remote.target_url)
                    .await
                    .ok()
                    .map(|id| format!("haven://artwork/{id}")),
                None => None,
            };
            items.push(TrendingItemDto {
                title,
                subtitle,
                description,
                poster_uri,
                status_badge: item.status_badge.and_then(|value| bounded_text(&value, 80)),
            });
        }
        if items.is_empty() {
            return None;
        }
        Some(TrendingBoardDto {
            board_id: candidate.board_id,
            title,
            subtitle,
            items,
        })
    }

    fn dto_from_entries(
        &self,
        entries: Vec<TrendingBoardCacheEntry>,
        now: i64,
    ) -> TrendingBoardsDto {
        let mut by_id = BTreeMap::new();
        for entry in entries {
            if entry.refreshed_at < now - TRENDING_STALE_WINDOW_MS {
                continue;
            }
            let board_id = entry.board.board_id.clone();
            if CANONICAL_BOARD_IDS.contains(&board_id.as_str())
                && entry.board.items.iter().all(|item| {
                    item.poster_uri
                        .as_deref()
                        .is_none_or(is_controlled_artwork_uri)
                })
            {
                by_id.insert(board_id, entry.board);
            }
        }
        let boards = CANONICAL_BOARD_IDS
            .iter()
            .filter_map(|id| by_id.remove(*id))
            .collect();
        TrendingBoardsDto {
            schema_version: 1,
            boards,
        }
    }
}

fn bounded_text(value: &str, max_chars: usize) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(value.chars().take(max_chars).collect())
}

fn is_controlled_artwork_uri(value: &str) -> bool {
    let Some(id) = value.strip_prefix("haven://artwork/") else {
        return false;
    };
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn refresh_timeout() -> AppError {
    AppError::new(
        "TRENDING_REFRESH_TIMEOUT",
        ErrorKind::Timeout,
        "热榜刷新超时，请稍后重试",
        true,
    )
}

fn refresh_failed(message: &'static str) -> AppError {
    AppError::new("TRENDING_REFRESH_FAILED", ErrorKind::Network, message, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct FakeCache {
        entries: Mutex<Vec<TrendingBoardCacheEntry>>,
    }

    #[async_trait]
    impl TrendingCachePort for FakeCache {
        async fn list(&self) -> Result<Vec<TrendingBoardCacheEntry>, AppError> {
            Ok(self.entries.lock().unwrap().clone())
        }

        async fn upsert(&self, entry: &TrendingBoardCacheEntry) -> Result<(), AppError> {
            let mut entries = self.entries.lock().unwrap();
            entries.retain(|old| old.board.board_id != entry.board.board_id);
            entries.push(entry.clone());
            Ok(())
        }
    }

    struct FakeArtwork;

    #[async_trait]
    impl ArtworkCachePort for FakeArtwork {
        async fn register(&self, _source_id: &str, _target_url: &str) -> Result<String, AppError> {
            Ok("artwork-1".into())
        }
    }

    struct FakeProvider {
        calls: AtomicUsize,
        delay: Duration,
    }

    #[async_trait]
    impl TrendingProvider for FakeProvider {
        async fn fetch_boards(
            &self,
            board_ids: &[String],
        ) -> Result<Vec<TrendingBoardCandidate>, AppError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            Ok(board_ids
                .iter()
                .map(|board_id| TrendingBoardCandidate {
                    board_id: board_id.clone(),
                    title: board_id.clone(),
                    subtitle: "TOP 10".into(),
                    items: vec![TrendingItemCandidate {
                        title: "作品".into(),
                        subtitle: "2026".into(),
                        description: "简介".into(),
                        poster: None,
                        status_badge: None,
                    }],
                })
                .collect())
        }
    }

    #[tokio::test]
    async fn query_is_warm_local_and_refresh_is_single_flight() {
        let cache = Arc::new(FakeCache::default());
        let provider = Arc::new(FakeProvider {
            calls: AtomicUsize::new(0),
            delay: Duration::from_millis(20),
        });
        let service = TrendingService::new(provider.clone(), cache, Arc::new(FakeArtwork));
        assert!(service.boards().await.unwrap().boards.is_empty());
        let (a, b) = tokio::join!(service.refresh(), service.refresh());
        assert_eq!(a.unwrap().boards.len(), 4);
        assert_eq!(b.unwrap().boards.len(), 4);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(service.boards().await.unwrap().boards.len(), 4);
    }

    #[tokio::test]
    async fn invalid_remote_artwork_only_removes_poster() {
        struct FailingArtwork;
        #[async_trait]
        impl ArtworkCachePort for FailingArtwork {
            async fn register(
                &self,
                _source_id: &str,
                _target_url: &str,
            ) -> Result<String, AppError> {
                Err(AppError::new(
                    "ARTWORK_FAILED",
                    ErrorKind::Network,
                    "failed",
                    true,
                ))
            }
        }
        struct OneBoard;
        #[async_trait]
        impl TrendingProvider for OneBoard {
            async fn fetch_boards(
                &self,
                _ids: &[String],
            ) -> Result<Vec<TrendingBoardCandidate>, AppError> {
                Ok(vec![TrendingBoardCandidate {
                    board_id: "anime".into(),
                    title: "动漫".into(),
                    subtitle: "TOP".into(),
                    items: vec![TrendingItemCandidate {
                        title: "作品".into(),
                        subtitle: "2026".into(),
                        description: "简介".into(),
                        poster: Some(RemoteArtworkCandidate {
                            source_id: "douban".into(),
                            target_url: "https://img.doubanio.com/a.jpg".into(),
                        }),
                        status_badge: None,
                    }],
                }])
            }
        }
        let service = TrendingService::new(
            Arc::new(OneBoard),
            Arc::new(FakeCache::default()),
            Arc::new(FailingArtwork),
        );
        let dto = service.refresh().await.unwrap();
        assert_eq!(dto.boards[0].items[0].poster_uri, None);
    }

    #[tokio::test]
    async fn provider_failure_returns_stale_and_enters_cooldown() {
        struct FailingProvider {
            calls: AtomicUsize,
        }

        #[async_trait]
        impl TrendingProvider for FailingProvider {
            async fn fetch_boards(
                &self,
                _ids: &[String],
            ) -> Result<Vec<TrendingBoardCandidate>, AppError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Err(AppError::new(
                    "PROVIDER_UNAVAILABLE",
                    ErrorKind::Network,
                    "unavailable",
                    true,
                ))
            }
        }

        let cache = Arc::new(FakeCache::default());
        let now = UtcMillis::now().0;
        cache.entries.lock().unwrap().push(TrendingBoardCacheEntry {
            board: TrendingBoardDto {
                board_id: "anime".into(),
                title: "动漫热门".into(),
                subtitle: "TOP 10".into(),
                items: vec![TrendingItemDto {
                    title: "旧快照".into(),
                    subtitle: "2025".into(),
                    description: "可用".into(),
                    poster_uri: None,
                    status_badge: None,
                }],
            },
            source_id: TRENDING_SOURCE_ID.into(),
            revision: "stale".into(),
            refreshed_at: now - 1_000,
            expires_at: now - 1,
        });
        let provider = Arc::new(FailingProvider {
            calls: AtomicUsize::new(0),
        });
        let service = TrendingService::new(provider.clone(), cache, Arc::new(FakeArtwork));

        assert_eq!(service.refresh().await.unwrap().boards.len(), 1);
        // The stale fallback is an intentional successful response, but the
        // failed attempt still enters the 60-second single-flight cooldown.
        assert_eq!(service.refresh().await.unwrap().boards.len(), 1);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn empty_provider_result_returns_stale_and_enters_cooldown() {
        struct EmptyProvider {
            calls: AtomicUsize,
        }

        #[async_trait]
        impl TrendingProvider for EmptyProvider {
            async fn fetch_boards(
                &self,
                _ids: &[String],
            ) -> Result<Vec<TrendingBoardCandidate>, AppError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(Vec::new())
            }
        }

        let cache = Arc::new(FakeCache::default());
        let now = UtcMillis::now().0;
        cache.entries.lock().unwrap().push(TrendingBoardCacheEntry {
            board: TrendingBoardDto {
                board_id: "anime".into(),
                title: "动漫热门".into(),
                subtitle: "TOP 10".into(),
                items: vec![TrendingItemDto {
                    title: "旧快照".into(),
                    subtitle: "2025".into(),
                    description: "可用".into(),
                    poster_uri: None,
                    status_badge: None,
                }],
            },
            source_id: TRENDING_SOURCE_ID.into(),
            revision: "stale".into(),
            refreshed_at: now - 1_000,
            expires_at: now - 1,
        });
        let provider = Arc::new(EmptyProvider {
            calls: AtomicUsize::new(0),
        });
        let service = TrendingService::new(provider.clone(), cache, Arc::new(FakeArtwork));

        assert_eq!(service.refresh().await.unwrap().boards.len(), 1);
        assert_eq!(service.refresh().await.unwrap().boards.len(), 1);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}
