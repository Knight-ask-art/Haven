//! Opaque, owner-bound runtime session and comic page grant registry.
//!
//! Paths, archive entry keys and page bytes stay server-side. Session records
//! and comic grants deliberately share one lock so close/window cleanup can
//! revoke the complete capability set at one linearization point.
//!
//! The same lock carries the per-window close transaction marker
//! (`closing_windows`). Claiming a window's close and removing its sessions
//! happen in one write-lock acquisition, so a repeated close request cannot
//! race the first one's still-pending reading-record writes and destroy the
//! last window out from under them.
//!
//! That same marker is what `register` consults: a session opened *during* a
//! close transaction would not be in the set that transaction removed, so it
//! would neither be revoked nor recorded. Opening into a closing window is
//! therefore refused outright rather than silently registered.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::ops::Deref;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use haven_application::services::{
    PreparedComicPage, PreparedComicPageAvailability, PreparedSession, PreparedSessionSource,
    PreparedSubtitleTrack,
};
use haven_application::wire::{
    ComicPageAvailabilityDto, ComicPageDto, ComicPageManifestDto, SessionEngineDto,
    SubtitleTrackDto,
};
use haven_common::{AppError, ErrorKind, UtcMillis};

pub(crate) const MAX_CONCURRENT_COMIC_PAGE_READS: usize = 4;
/// Runtime sessions are short-lived capabilities. A caller can reopen a
/// session after expiry; no remote identity is retained as a durable grant.
pub(crate) const SESSION_TTL: Duration = Duration::from_secs(30 * 60);

pub(crate) struct SessionRegistry {
    state: RwLock<RegistryState>,
}

#[derive(Default)]
struct RegistryState {
    sessions: HashMap<String, SessionRecord>,
    grants: HashMap<String, ComicGrant>,
    /// 正在关闭中的窗口标签（关闭事务占位）。见 `begin_window_close`。
    closing_windows: HashSet<String>,
}

struct SessionRecord {
    prepared: PreparedSession,
    owner_webview_label: String,
    owner_window_label: String,
    expires_at: Instant,
    /// 会话被真正打开时的观测时刻（UTC 毫秒）。这是阅读总览的唯一合法起点：
    /// 它在 `register` 的同一个写锁内确定，之后不再被任何路径改写。
    opened_at: UtcMillis,
    comic_manifest: Option<RegisteredComicManifest>,
    active_comic_reads: usize,
}

/// 一次被真正关闭的会话：运行时会话身份、真实媒体事实与两个观测时刻。
///
/// 关闭路径（显式 `session_close` / 窗口关闭）据此登记阅读活动——`media_item_id`
/// 与 `media_type` 都来自 `PreparedSession` 的真实事实，不由前端提供；
/// 带上 `session_id` 是为了让「幂等键 = 运行时会话身份」在调用点无需再查一次注册表。
#[derive(Debug)]
pub(crate) struct ClosedSession {
    pub(crate) session_id: String,
    pub(crate) prepared: PreparedSession,
    /// 会话打开时由 `register` 在写锁内观测到的时刻。见 `SessionRecord::opened_at`。
    pub(crate) opened_at: UtcMillis,
    /// 摘除记录时**在同一把写锁内**观测到的结束时刻。
    ///
    /// 它必须和 `opened_at` 一样来自注册表内部：关闭之后的撤销与统计都是异步的，
    /// 一旦改成在异步任务里补取「当下」，结束时刻就漂移到统计真正执行的那一刻——
    /// 多出来的时间不属于这段阅读，进程提前退出时它甚至根本取不到。
    pub(crate) ended_at: UtcMillis,
}

struct RegisteredComicManifest {
    dto: ComicPageManifestDto,
    grant_ids: Vec<String>,
}

#[derive(Clone)]
struct ComicGrant {
    session_id: String,
    page_index: usize,
}

/// A file handle whose canonical path was validated while the registry read
/// lock was held. The protocol consumes this handle instead of reopening a
/// changed path after authorization.
#[derive(Debug)]
pub(crate) struct VerifiedSessionFile {
    pub(crate) prepared: PreparedSession,
    pub(crate) file: File,
}

/// A subtitle handle whose session, owner, root containment and track identity
/// were checked while the registry read lock was held.
#[derive(Debug)]
pub(crate) struct VerifiedSubtitleFile {
    pub(crate) track: PreparedSubtitleTrack,
    pub(crate) file: File,
}

/// Revalidated session capability. Remote sessions intentionally have no file
/// handle; the resource protocol delegates their body read to an allowlisted
/// application port.
#[derive(Debug)]
pub(crate) enum VerifiedSession {
    Local(VerifiedSessionFile),
    Remote(PreparedSession),
    /// 云盘只读 PDF：与远端一样没有本地文件句柄，只交出 pinned 快照。绑定与凭据
    /// 校验在协议层与 `SessionService::read_cloud` 里完成，绝不走本地打开分支。
    Cloud(PreparedSession),
}

impl SessionRecord {
    fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

#[derive(Debug, Clone)]
pub(crate) struct VerifiedComicPage {
    pub(crate) session_id: String,
    pub(crate) prepared: PreparedSession,
    pub(crate) page: PreparedComicPage,
}

pub(crate) struct ComicPageReadPermit {
    registry: Arc<SessionRegistry>,
    verified: VerifiedComicPage,
}

impl Deref for ComicPageReadPermit {
    type Target = VerifiedComicPage;

    fn deref(&self) -> &Self::Target {
        &self.verified
    }
}

impl fmt::Debug for ComicPageReadPermit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComicPageReadPermit")
            .field("verified", &self.verified)
            .finish_non_exhaustive()
    }
}

impl Drop for ComicPageReadPermit {
    fn drop(&mut self) {
        self.registry
            .finish_comic_page_read(&self.verified.session_id);
    }
}

/// 一次进行中的窗口关闭事务。
///
/// 它同时是两件事的所有权凭据：
/// 1. **会话已经被摘除**（`closed_sessions`），但撤销授权与统计写还没做完；
/// 2. **本窗口在注册表里占着关闭位**（`RegistryState::closing_windows`），
///    所以同一个窗口的第二个关闭请求拿不到事务，只能被吞掉。
///
/// 因此调用方必须让它在「撤销 + 统计全部落地、窗口已经销毁」之后才离开作用域：
/// 提前释放就等于把占位让给一个只会立刻销毁窗口的重复请求——最后一个窗口的销毁
/// 会让进程退出，这次事务还在等的统计写就永远落不了地。
/// `Drop` 负责释放占位：窗口销毁后重建的同名窗口仍然关得掉。
pub(crate) struct WindowCloseLease {
    registry: Arc<SessionRegistry>,
    owner_window_label: String,
    closed_sessions: Vec<ClosedSession>,
}

impl WindowCloseLease {
    /// 被关闭窗口的标签。撤销流授权必须用同一个 owner 身份（就是这次事务识别出的
    /// 那个窗口），不能另取一个：身份一旦分叉，撤销就可能落空。
    pub(crate) fn owner_window_label(&self) -> &str {
        &self.owner_window_label
    }

    /// 本次关闭摘除的全部会话；每条自带同一个结束时刻观测值（见 `ClosedSession`）。
    #[cfg(test)]
    pub(crate) fn closed_sessions(&self) -> &[ClosedSession] {
        &self.closed_sessions
    }

    /// Move the closed-session facts to the blocking persistence worker while
    /// keeping this lease alive as the per-window close guard.
    pub(crate) fn take_closed_sessions(&mut self) -> Vec<ClosedSession> {
        std::mem::take(&mut self.closed_sessions)
    }
}

impl fmt::Debug for WindowCloseLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowCloseLease")
            .field("owner_window_label", &self.owner_window_label)
            .field("closed_session_count", &self.closed_sessions.len())
            .finish_non_exhaustive()
    }
}

impl Drop for WindowCloseLease {
    fn drop(&mut self) {
        self.registry.end_window_close(&self.owner_window_label);
    }
}

impl SessionRegistry {
    pub(crate) fn new() -> Self {
        Self {
            state: RwLock::new(RegistryState::default()),
        }
    }

    pub(crate) fn register(
        &self,
        prepared: PreparedSession,
        owner_webview_label: String,
        owner_window_label: String,
    ) -> Result<String, AppError> {
        let mut state = self.write_state()?;
        // 正在关闭的窗口不再接受新会话。
        //
        // 关闭事务在**一次**写锁里摘除该窗口当时拥有的全部会话；事务开始之后、lease 释放
        // 之前登记进来的会话不在那次摘除里，于是它既不会被撤销、也不会被登记进阅读活动
        // ——它会一直留到 TTL 到期，而用户关掉应用前读到的那一段永远记不上。占位与摘除
        // 共用同一把写锁，所以这里读到的「正在关闭」就是摘除那一刻看到的同一个状态：
        // 不存在「先登记、随后又被同一次事务摘掉」的窗口，两个结果必居其一。
        if state.closing_windows.contains(&owner_window_label) {
            return Err(session_window_closing());
        }
        let id = unique_uuid(|candidate| state.sessions.contains_key(candidate));
        state.sessions.insert(
            id.clone(),
            SessionRecord {
                prepared,
                owner_webview_label,
                owner_window_label,
                expires_at: Instant::now() + SESSION_TTL,
                opened_at: UtcMillis::now(),
                comic_manifest: None,
                active_comic_reads: 0,
            },
        );
        Ok(id)
    }

    pub(crate) fn uri(id: &str) -> String {
        format!("haven-resource://session/{id}")
    }

    pub(crate) fn subtitle_uri(session_id: &str, track_id: &str) -> String {
        format!("haven-resource://session/{session_id}/subtitle/{track_id}")
    }

    pub(crate) fn subtitle_tracks(
        &self,
        id: &str,
        owner_webview_label: &str,
    ) -> Result<Option<Vec<SubtitleTrackDto>>, AppError> {
        let state = self.read_state()?;
        let record = state
            .sessions
            .get(id)
            .filter(|record| {
                record.owner_webview_label == owner_webview_label && !record.is_expired()
            })
            .ok_or_else(resource_not_found)?;
        if record.prepared.engine != SessionEngineDto::Playback
            || record.prepared.subtitle_tracks.is_empty()
        {
            return Ok(None);
        }
        let mut tracks = Vec::with_capacity(record.prepared.subtitle_tracks.len());
        for track in &record.prepared.subtitle_tracks {
            let track_uuid = uuid::Uuid::parse_str(&track.track_id)
                .ok()
                .filter(|id| id.to_string() == track.track_id)
                .ok_or_else(|| policy_denied("字幕轨道身份校验失败"))?;
            tracks.push(SubtitleTrackDto {
                track_id: track_uuid.to_string(),
                label: track.label.clone(),
                language: track.language.clone(),
                format: track.format,
                content_uri: Self::subtitle_uri(id, &track_uuid.to_string()),
            });
        }
        Ok(Some(tracks))
    }

    pub(crate) fn comic_page_uri(grant_id: &str) -> String {
        format!("haven-resource://comic-page/{grant_id}")
    }

    pub(crate) fn lookup_for_owner(
        &self,
        id: &str,
        owner_webview_label: &str,
    ) -> Result<PreparedSession, AppError> {
        let state = self.read_state()?;
        let record = state
            .sessions
            .get(id)
            .filter(|record| {
                record.owner_webview_label == owner_webview_label && !record.is_expired()
            })
            .ok_or_else(resource_not_found)?;
        Ok(record.prepared.clone())
    }

    pub(crate) fn comic_manifest(
        &self,
        id: &str,
        owner_webview_label: &str,
    ) -> Result<ComicPageManifestDto, AppError> {
        let mut state = self.write_state()?;
        let (media_item_id, prepared_pages) = {
            let record = state
                .sessions
                .get(id)
                .filter(|record| {
                    record.owner_webview_label == owner_webview_label && !record.is_expired()
                })
                .ok_or_else(resource_not_found)?;
            if record.prepared.engine != SessionEngineDto::Comic {
                return Err(format_unsupported());
            }
            if let Some(manifest) = &record.comic_manifest {
                return Ok(manifest.dto.clone());
            }
            let pages = record
                .prepared
                .comic_pages
                .clone()
                .ok_or_else(format_unsupported)?;
            (record.prepared.media_item_id.clone(), pages)
        };

        let page_count = u32::try_from(prepared_pages.len()).map_err(|_| format_unsupported())?;
        let mut reserved_ids = HashSet::with_capacity(prepared_pages.len().saturating_mul(2));
        let mut grants = Vec::new();
        let mut pages = Vec::with_capacity(prepared_pages.len());
        for (page_index, page) in prepared_pages.iter().enumerate() {
            let page_id = unique_uuid(|candidate| {
                reserved_ids.contains(candidate)
                    || state.sessions.contains_key(candidate)
                    || state.grants.contains_key(candidate)
            });
            reserved_ids.insert(page_id.clone());
            let (availability, content_uri) = match page.availability {
                PreparedComicPageAvailability::Ready => {
                    let grant_id = unique_uuid(|candidate| {
                        reserved_ids.contains(candidate)
                            || state.sessions.contains_key(candidate)
                            || state.grants.contains_key(candidate)
                    });
                    reserved_ids.insert(grant_id.clone());
                    grants.push((grant_id.clone(), page_index));
                    (
                        ComicPageAvailabilityDto::Ready,
                        Some(Self::comic_page_uri(&grant_id)),
                    )
                }
                PreparedComicPageAvailability::Unavailable => {
                    (ComicPageAvailabilityDto::Unavailable, None)
                }
            };
            pages.push(ComicPageDto {
                page_id,
                page_index: u32::try_from(page_index).map_err(|_| format_unsupported())?,
                availability,
                content_uri,
            });
        }

        let dto = ComicPageManifestDto {
            schema_version: 1,
            session_id: id.to_owned(),
            media_item_id,
            page_count,
            pages,
        };
        for (grant_id, page_index) in &grants {
            state.grants.insert(
                grant_id.clone(),
                ComicGrant {
                    session_id: id.to_owned(),
                    page_index: *page_index,
                },
            );
        }
        let record = state.sessions.get_mut(id).ok_or_else(resource_not_found)?;
        record.comic_manifest = Some(RegisteredComicManifest {
            dto: dto.clone(),
            grant_ids: grants.into_iter().map(|(grant_id, _)| grant_id).collect(),
        });
        Ok(dto)
    }

    pub(crate) fn lookup_comic_page(
        &self,
        grant_id: &str,
        owner_webview_label: &str,
    ) -> Result<VerifiedComicPage, AppError> {
        let state = self.read_state()?;
        verified_comic_page(&state, grant_id, owner_webview_label)
    }

    /// Replace the server-side prepared page facts for an owner-bound Comic
    /// session. Existing page grants are revoked before the next manifest is
    /// materialized, so a refreshed manifest never leaves stale runtime
    /// capabilities usable.
    pub(crate) fn replace_comic_pages(
        &self,
        id: &str,
        owner_webview_label: &str,
        pages: Vec<PreparedComicPage>,
    ) -> Result<(), AppError> {
        let mut state = self.write_state()?;
        let grant_ids = {
            let record = state
                .sessions
                .get(id)
                .filter(|record| {
                    record.owner_webview_label == owner_webview_label && !record.is_expired()
                })
                .ok_or_else(resource_not_found)?;
            if record.prepared.engine != SessionEngineDto::Comic {
                return Err(format_unsupported());
            }
            record
                .comic_manifest
                .as_ref()
                .map(|manifest| manifest.grant_ids.clone())
                .unwrap_or_default()
        };
        for grant_id in grant_ids {
            state.grants.remove(&grant_id);
        }
        let record = state
            .sessions
            .get_mut(id)
            .filter(|record| {
                record.owner_webview_label == owner_webview_label && !record.is_expired()
            })
            .ok_or_else(resource_not_found)?;
        record.prepared.comic_pages = Some(pages);
        record.comic_manifest = None;
        Ok(())
    }

    pub(crate) fn begin_comic_page_read(
        self: &Arc<Self>,
        grant_id: &str,
        owner_webview_label: &str,
    ) -> Result<ComicPageReadPermit, AppError> {
        let mut state = self.write_state()?;
        let grant = state
            .grants
            .get(grant_id)
            .cloned()
            .ok_or_else(resource_not_found)?;
        let record = state
            .sessions
            .get_mut(&grant.session_id)
            .filter(|record| {
                record.owner_webview_label == owner_webview_label && !record.is_expired()
            })
            .ok_or_else(resource_not_found)?;
        if record.active_comic_reads >= MAX_CONCURRENT_COMIC_PAGE_READS {
            return Err(resource_busy());
        }
        let page = record
            .prepared
            .comic_pages
            .as_ref()
            .and_then(|pages| pages.get(grant.page_index))
            .cloned()
            .ok_or_else(resource_not_found)?;
        record.active_comic_reads += 1;
        Ok(ComicPageReadPermit {
            registry: self.clone(),
            verified: VerifiedComicPage {
                session_id: grant.session_id,
                prepared: comic_session_snapshot(&record.prepared),
                page,
            },
        })
    }

    fn finish_comic_page_read(&self, session_id: &str) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(record) = state.sessions.get_mut(session_id) {
            record.active_comic_reads = record.active_comic_reads.saturating_sub(1);
        }
    }

    /// Owner-aware idempotent close. A different WebView cannot use close as an
    /// existence oracle and cannot revoke another owner's session.
    ///
    /// 返回被关闭会话的真实事实（含摘除那一刻观测到的结束时刻）；重复关闭返回
    /// `Ok(None)`，这正是「幂等关闭不会重复计入阅读时长」的依据——第二次调用根本
    /// 没有可登记的会话，而不是靠调用方自觉去重。
    pub(crate) fn remove_for_owner(
        &self,
        id: &str,
        owner_webview_label: &str,
    ) -> Result<Option<ClosedSession>, AppError> {
        let mut state = self.write_state()?;
        let is_owner = state
            .sessions
            .get(id)
            .is_some_and(|record| record.owner_webview_label == owner_webview_label);
        if !is_owner {
            return Ok(None);
        }
        // 结束时刻在这里观测：调用方随后还要撤销流授权、再等统计写落地，那些都是
        // 异步的，等它们跑完再取「当下」已经不是关闭发生的那一刻了。
        let ended_at = UtcMillis::now();
        Ok(remove_session(&mut state, id)
            .map(|record| closed_session(id.to_owned(), record, ended_at)))
    }

    /// 销毁窗口时关闭它拥有的**全部**会话，并如实返回每一条的真实事实，
    /// 让上层能把它们登记进阅读活动（窗口被销毁同样是一次内容会话的结束）。
    ///
    /// 这是给**非正常销毁**（外部 `destroy`、进程退出）用的 best-effort 入口：
    /// 窗口已经不存在，没有可以等待的时机。正常的用户关窗走 [`Self::begin_window_close`]，
    /// 它把「占位 + 摘除」和「等统计落地再销毁窗口」合成一次事务。
    pub(crate) fn remove_window(&self, owner_window_label: &str) -> Vec<ClosedSession> {
        let mut state = self.close_state();
        let ended_at = UtcMillis::now();
        remove_window_sessions(&mut state, owner_window_label, ended_at)
    }

    /// 开始一次窗口关闭事务：在同一把写锁内占位并摘除该窗口的全部会话。
    ///
    /// - `Some(lease)`：本调用取得这次关闭事务的所有权。会话能力已经被摘掉、
    ///   结束时刻已经取到，统计尚未写；lease 被丢弃之前不得销毁窗口。
    /// - `None`：本窗口已经有一次关闭事务在进行中，这次请求必须只是空操作。
    ///   否则第二个请求会摘到一个空会话集合、然后立刻销毁窗口，把第一次事务还在
    ///   等的统计写连同进程一起带走。
    ///
    /// 占位与摘除共用一次写锁，所以「谁先到」有唯一的线性化点；这也让重复请求的
    /// 去重不必依赖窗口是否还活着这种外部状态。
    pub(crate) fn begin_window_close(
        self: &Arc<Self>,
        owner_window_label: &str,
    ) -> Option<WindowCloseLease> {
        let mut state = self.close_state();
        if !state.closing_windows.insert(owner_window_label.to_owned()) {
            return None;
        }
        let ended_at = UtcMillis::now();
        let closed_sessions = remove_window_sessions(&mut state, owner_window_label, ended_at);
        Some(WindowCloseLease {
            registry: self.clone(),
            owner_window_label: owner_window_label.to_owned(),
            closed_sessions,
        })
    }

    /// 释放一次窗口关闭事务的占位。由 [`WindowCloseLease`] 的 `Drop` 调用：
    /// 占位不能跟着「窗口还存不存在」走，否则销毁后重建的同名窗口会关不掉。
    fn end_window_close(&self, owner_window_label: &str) {
        self.close_state()
            .closing_windows
            .remove(owner_window_label);
    }

    /// Atomically validate and open a non-Comic session file while the record
    /// read lock is held. Comic archive bytes must never use this root channel.
    #[cfg(test)]
    pub(crate) fn revalidate(
        &self,
        id: &str,
        owner_webview_label: &str,
    ) -> Result<VerifiedSessionFile, AppError> {
        match self.revalidate_any(id, owner_webview_label)? {
            VerifiedSession::Local(local) => Ok(local),
            VerifiedSession::Remote(_) | VerifiedSession::Cloud(_) => Err(resource_not_found()),
        }
    }

    /// Revalidate an open session without assuming it is backed by a local
    /// path. This is the resource protocol entry point for both local and
    /// remote sessions.
    pub(crate) fn revalidate_any(
        &self,
        id: &str,
        owner_webview_label: &str,
    ) -> Result<VerifiedSession, AppError> {
        let state = self.read_state()?;
        let record = state
            .sessions
            .get(id)
            .filter(|record| {
                record.owner_webview_label == owner_webview_label && !record.is_expired()
            })
            .ok_or_else(resource_not_found)?;
        let prepared = &record.prepared;
        if matches!(&prepared.source, PreparedSessionSource::Remote { .. }) {
            return Ok(VerifiedSession::Remote(prepared.clone()));
        }
        // 云盘会话由应用层受控读取，没有本地路径可以重新校验；owner / 过期 / 关闭
        // 事务的判定与其它会话完全共用上面那一次查表，不在这里放宽任何一条。
        if matches!(&prepared.source, PreparedSessionSource::CloudObject { .. }) {
            return Ok(VerifiedSession::Cloud(prepared.clone()));
        }
        if prepared.engine == SessionEngineDto::Comic {
            return Err(resource_not_found());
        }
        let Some(prepared_root) = prepared.canonical_root.as_deref() else {
            return Err(resource_unavailable());
        };
        let Some(prepared_file) = prepared.canonical_file.as_deref() else {
            return Err(resource_unavailable());
        };
        let root = std::fs::canonicalize(prepared_root).map_err(|_| resource_unavailable())?;
        let file = std::fs::canonicalize(prepared_file).map_err(|_| resource_unavailable())?;
        if root != prepared_root
            || file != prepared_file
            || file.strip_prefix(&root).is_err()
            || !file.is_file()
        {
            return Err(policy_denied("资源路径校验失败"));
        }
        let handle = File::open(&file).map_err(|_| resource_unavailable())?;
        Ok(VerifiedSession::Local(VerifiedSessionFile {
            prepared: prepared.clone(),
            file: handle,
        }))
    }

    /// Revalidate one local subtitle track. Database/resource binding is
    /// checked by the protocol before this registry operation; this method
    /// owns the owner/session/track/path capability boundary.
    pub(crate) fn revalidate_subtitle(
        &self,
        session_id: &str,
        track_id: &str,
        owner_webview_label: &str,
    ) -> Result<VerifiedSubtitleFile, AppError> {
        let state = self.read_state()?;
        let record = state
            .sessions
            .get(session_id)
            .filter(|record| {
                record.owner_webview_label == owner_webview_label && !record.is_expired()
            })
            .ok_or_else(resource_not_found)?;
        if record.prepared.engine != SessionEngineDto::Playback {
            return Err(format_unsupported());
        }
        let track = record
            .prepared
            .subtitle_tracks
            .iter()
            .find(|track| track.track_id == track_id)
            .cloned()
            .ok_or_else(resource_not_found)?;
        let Some(prepared_root) = record.prepared.canonical_root.as_deref() else {
            return Err(resource_not_found());
        };
        let root = std::fs::canonicalize(prepared_root).map_err(|_| resource_unavailable())?;
        let file =
            std::fs::canonicalize(&track.canonical_file).map_err(|_| resource_unavailable())?;
        if root != prepared_root
            || file != track.canonical_file
            || file.strip_prefix(&root).is_err()
            || !file.is_file()
        {
            return Err(policy_denied("字幕资源路径校验失败"));
        }
        let handle = File::open(&file).map_err(|_| resource_unavailable())?;
        Ok(VerifiedSubtitleFile {
            track,
            file: handle,
        })
    }

    fn read_state(&self) -> Result<RwLockReadGuard<'_, RegistryState>, AppError> {
        self.state.read().map_err(|_| registry_unavailable())
    }

    fn write_state(&self) -> Result<RwLockWriteGuard<'_, RegistryState>, AppError> {
        self.state.write().map_err(|_| registry_unavailable())
    }

    /// 关闭路径专用的写锁：毒化（别的线程持锁时 panic）在这里必须被恢复，而不是上报。
    ///
    /// 关闭路径没有「返回错误」这个选项——那要么让窗口永远关不掉，要么让能力撤销
    /// 悄悄不发生，而撤销才是安全的一侧。恢复出守卫后撤销照常进行；最坏情况是少登记
    /// 一条 best-effort 的阅读记录。`finish_comic_page_read` 出于同样的理由恢复。
    fn close_state(&self) -> RwLockWriteGuard<'_, RegistryState> {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn verified_comic_page(
    state: &RegistryState,
    grant_id: &str,
    owner_webview_label: &str,
) -> Result<VerifiedComicPage, AppError> {
    let grant = state.grants.get(grant_id).ok_or_else(resource_not_found)?;
    let record = state
        .sessions
        .get(&grant.session_id)
        .filter(|record| record.owner_webview_label == owner_webview_label && !record.is_expired())
        .ok_or_else(resource_not_found)?;
    let page = record
        .prepared
        .comic_pages
        .as_ref()
        .and_then(|pages| pages.get(grant.page_index))
        .cloned()
        .ok_or_else(resource_not_found)?;
    Ok(VerifiedComicPage {
        session_id: grant.session_id.clone(),
        prepared: comic_session_snapshot(&record.prepared),
        page,
    })
}

fn comic_session_snapshot(prepared: &PreparedSession) -> PreparedSession {
    PreparedSession {
        work_id: prepared.work_id.clone(),
        edition_id: prepared.edition_id.clone(),
        media_item_id: prepared.media_item_id.clone(),
        engine: prepared.engine,
        resource_id: prepared.resource_id,
        storage_location_id: prepared.storage_location_id,
        canonical_root: prepared.canonical_root.clone(),
        canonical_file: prepared.canonical_file.clone(),
        subtitle_tracks: prepared.subtitle_tracks.clone(),
        mime_type: prepared.mime_type.clone(),
        media_type: prepared.media_type,
        resource_type: prepared.resource_type,
        source: prepared.source.clone(),
        comic_pages: None,
        progress: prepared.progress.clone(),
    }
}

fn closed_session(session_id: String, record: SessionRecord, ended_at: UtcMillis) -> ClosedSession {
    ClosedSession {
        session_id,
        prepared: record.prepared,
        opened_at: record.opened_at,
        ended_at,
    }
}

/// 摘除一个窗口拥有的全部会话，全部会话共用同一个结束时刻观测值。
///
/// 调用方必须已经在写锁内取到 `ended_at`：摘除与观测时刻是同一次临界区里的事实，
/// 拆成两步就会出现「时刻在摘除之前/之后」的漂移。
fn remove_window_sessions(
    state: &mut RegistryState,
    owner_window_label: &str,
    ended_at: UtcMillis,
) -> Vec<ClosedSession> {
    let session_ids: Vec<String> = state
        .sessions
        .iter()
        .filter(|(_, record)| record.owner_window_label == owner_window_label)
        .map(|(id, _)| id.clone())
        .collect();
    session_ids
        .into_iter()
        .filter_map(|session_id| {
            remove_session(state, &session_id)
                .map(|record| closed_session(session_id, record, ended_at))
        })
        .collect()
}

fn remove_session(state: &mut RegistryState, id: &str) -> Option<SessionRecord> {
    let record = state.sessions.remove(id)?;
    if let Some(manifest) = &record.comic_manifest {
        for grant_id in &manifest.grant_ids {
            state.grants.remove(grant_id);
        }
    }
    Some(record)
}

fn unique_uuid(mut exists: impl FnMut(&str) -> bool) -> String {
    loop {
        let candidate = uuid::Uuid::new_v4().to_string();
        if !exists(&candidate) {
            return candidate;
        }
    }
}

fn resource_not_found() -> AppError {
    AppError::new(
        "RESOURCE_NOT_FOUND",
        ErrorKind::NotFound,
        "资源会话不存在或已撤销",
        false,
    )
}

/// 窗口正在关闭：这一次打开不会被完成它的那个关闭事务收走，因此必须拒绝。
///
/// 为什么不是「照常登记、由关闭事务兜底」：事务已经摘完它那一刻看到的会话，之后登记的
/// 没有任何人去撤销或登记统计。为什么是 `Conflict` 而不是 `NotFound`：会话身份本身没有
/// 问题，只是当下的状态不允许——窗口关完（或重建同名窗口）之后，同一次打开会成功。
fn session_window_closing() -> AppError {
    AppError::new(
        "SESSION_WINDOW_CLOSING",
        ErrorKind::Conflict,
        "窗口正在关闭，不再打开新的阅读会话",
        false,
    )
}

fn resource_unavailable() -> AppError {
    AppError::new(
        "RESOURCE_UNAVAILABLE",
        ErrorKind::Storage,
        "本地资源当前不可用",
        false,
    )
}

fn resource_busy() -> AppError {
    AppError::new(
        "RESOURCE_UNAVAILABLE",
        ErrorKind::Timeout,
        "漫画页面读取繁忙，请稍后重试",
        true,
    )
}

fn format_unsupported() -> AppError {
    AppError::new(
        "FORMAT_UNSUPPORTED",
        ErrorKind::Unsupported,
        "当前 Session 不是受支持的漫画资源",
        false,
    )
}

fn policy_denied(message: &'static str) -> AppError {
    AppError::new(
        "SECURITY_POLICY_DENIED",
        ErrorKind::Security,
        message,
        false,
    )
}

fn registry_unavailable() -> AppError {
    AppError::new(
        "INTERNAL_ERROR",
        ErrorKind::Internal,
        "资源会话暂时不可用",
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use haven_application::services::{
        PreparedComicPage, PreparedComicPageAvailability, PreparedComicPageSource,
        PreparedSubtitleTrack,
    };
    use haven_application::wire::SubtitleFormatDto;
    use haven_domain::enums::{MediaType, ResourceType};
    use haven_domain::ids::{ResourceId, StorageLocationId};
    use std::io::Read;
    use std::path::PathBuf;

    fn prepared(root: PathBuf, file: PathBuf) -> PreparedSession {
        PreparedSession {
            work_id: "w".into(),
            edition_id: "e".into(),
            media_item_id: uuid::Uuid::new_v4().to_string(),
            engine: SessionEngineDto::Playback,
            resource_id: ResourceId::new(),
            storage_location_id: Some(StorageLocationId::new()),
            canonical_root: Some(root),
            canonical_file: Some(file),
            subtitle_tracks: Vec::new(),
            source: PreparedSessionSource::Local,
            mime_type: None,
            media_type: MediaType::Movie,
            resource_type: ResourceType::LocalFile,
            comic_pages: None,
            progress: None,
        }
    }

    fn comic_prepared(root: PathBuf, file: PathBuf) -> PreparedSession {
        let mut prepared = prepared(root, file);
        prepared.engine = SessionEngineDto::Comic;
        prepared.media_type = MediaType::Comic;
        prepared.resource_type = ResourceType::ComicArchive;
        prepared.comic_pages = Some(vec![
            PreparedComicPage {
                availability: PreparedComicPageAvailability::Ready,
                identity: haven_domain::comic_identity::PageIdentity::stable("page-1"),
                source: PreparedComicPageSource::ArchiveEntry {
                    entry_index: 0,
                    normalized_name: "page1.jpg".into(),
                    crc32: 1,
                    compressed_size: 3,
                    uncompressed_size: 3,
                    source_size: 3,
                    source_sha256: [1; 32],
                },
            },
            PreparedComicPage {
                availability: PreparedComicPageAvailability::Unavailable,
                identity: haven_domain::comic_identity::PageIdentity::stable("page-2"),
                source: PreparedComicPageSource::ArchiveEntry {
                    entry_index: 1,
                    normalized_name: "page2.jpg".into(),
                    crc32: 2,
                    compressed_size: 0,
                    uncompressed_size: 0,
                    source_size: 3,
                    source_sha256: [1; 32],
                },
            },
            PreparedComicPage {
                availability: PreparedComicPageAvailability::Ready,
                identity: haven_domain::comic_identity::PageIdentity::stable("page-3"),
                source: PreparedComicPageSource::ArchiveEntry {
                    entry_index: 2,
                    normalized_name: "page3.jpg".into(),
                    crc32: 3,
                    compressed_size: 3,
                    uncompressed_size: 3,
                    source_size: 3,
                    source_sha256: [1; 32],
                },
            },
            PreparedComicPage {
                availability: PreparedComicPageAvailability::Ready,
                identity: haven_domain::comic_identity::PageIdentity::stable("page-4"),
                source: PreparedComicPageSource::ArchiveEntry {
                    entry_index: 3,
                    normalized_name: "page4.jpg".into(),
                    crc32: 4,
                    compressed_size: 3,
                    uncompressed_size: 3,
                    source_size: 3,
                    source_sha256: [1; 32],
                },
            },
        ]);
        prepared
    }

    fn grant_id(uri: &str) -> String {
        uri.strip_prefix("haven-resource://comic-page/")
            .expect("comic grant URI")
            .to_owned()
    }

    #[test]
    fn session_close_is_owner_aware_idempotent_and_open_handle_can_finish() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("video.mkv");
        std::fs::write(&file, b"video").unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let registry = SessionRegistry::new();
        let id = registry
            .register(prepared(root, file), "main".into(), "main".into())
            .unwrap();
        assert_eq!(
            SessionRegistry::uri(&id),
            format!("haven-resource://session/{id}")
        );
        let mut verified = registry.revalidate(&id, "main").unwrap();
        assert!(registry.remove_for_owner(&id, "main2").unwrap().is_none());
        assert!(registry.lookup_for_owner(&id, "main").is_ok());
        assert!(registry.remove_for_owner(&id, "main").unwrap().is_some());
        assert!(registry.remove_for_owner(&id, "main").unwrap().is_none());
        assert_eq!(
            registry
                .revalidate(&id, "main")
                .unwrap_err()
                .code()
                .as_str(),
            "RESOURCE_NOT_FOUND"
        );
        let mut content = Vec::new();
        verified.file.read_to_end(&mut content).unwrap();
        assert_eq!(content, b"video");
    }

    #[test]
    fn subtitle_revalidation_is_owner_bound_and_uses_the_registered_file() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("video.mkv");
        let subtitle = dir.path().join("video.zh.srt");
        std::fs::write(&video, b"video").unwrap();
        std::fs::write(&subtitle, b"1\n00:00:00,000 --> 00:00:01,000\nhello\n").unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let video = std::fs::canonicalize(video).unwrap();
        let subtitle = std::fs::canonicalize(subtitle).unwrap();
        let track_id = uuid::Uuid::new_v4().to_string();
        let mut session = prepared(root.clone(), video);
        session.subtitle_tracks.push(PreparedSubtitleTrack {
            track_id: track_id.clone(),
            label: "中文".into(),
            language: Some("zh-CN".into()),
            format: SubtitleFormatDto::Srt,
            canonical_file: subtitle,
        });
        let registry = SessionRegistry::new();
        let session_id = registry
            .register(session, "main".into(), "main".into())
            .unwrap();

        let mut verified = registry
            .revalidate_subtitle(&session_id, &track_id, "main")
            .unwrap();
        let mut body = String::new();
        verified.file.read_to_string(&mut body).unwrap();
        assert!(body.contains("hello"));
        assert!(registry
            .revalidate_subtitle(&session_id, &track_id, "other")
            .is_err());
        assert!(registry
            .revalidate_subtitle(&session_id, &uuid::Uuid::new_v4().to_string(), "main")
            .is_err());
        assert!(root.is_dir());
    }

    #[test]
    fn comic_manifest_is_idempotent_and_close_revokes_every_grant() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chapter.cbz");
        std::fs::write(&file, b"zip").unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let registry = SessionRegistry::new();
        let id = registry
            .register(comic_prepared(root, file), "main".into(), "main".into())
            .unwrap();

        let first = registry.comic_manifest(&id, "main").unwrap();
        let second = registry.comic_manifest(&id, "main").unwrap();
        assert_eq!(first, second);
        assert_eq!(first.page_count, 4);
        assert_eq!(first.pages[0].page_index, 0);
        assert_eq!(first.pages[1].page_index, 1);
        assert_eq!(
            first.pages[1].availability,
            ComicPageAvailabilityDto::Unavailable
        );
        assert!(first.pages[1].content_uri.is_none());
        let page_ids: HashSet<&str> = first
            .pages
            .iter()
            .map(|page| page.page_id.as_str())
            .collect();
        let grant_ids: HashSet<String> = first
            .pages
            .iter()
            .filter_map(|page| page.content_uri.as_deref().map(grant_id))
            .collect();
        assert_eq!(page_ids.len(), first.pages.len());
        assert_eq!(grant_ids.len(), 3);
        assert!(page_ids.iter().all(|page_id| !grant_ids.contains(*page_id)));
        let grant = grant_id(first.pages[0].content_uri.as_deref().unwrap());
        assert_ne!(first.pages[0].page_id, grant);
        assert!(registry.lookup_comic_page(&grant, "main").is_ok());
        assert_eq!(
            registry
                .lookup_comic_page(&grant, "main2")
                .unwrap_err()
                .code()
                .as_str(),
            "RESOURCE_NOT_FOUND"
        );

        registry.remove_for_owner(&id, "main").unwrap();
        assert_eq!(
            registry
                .lookup_comic_page(&grant, "main")
                .unwrap_err()
                .code()
                .as_str(),
            "RESOURCE_NOT_FOUND"
        );
    }

    #[test]
    fn window_cleanup_is_scoped_and_reopen_rotates_runtime_identities() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chapter.cbz");
        std::fs::write(&file, b"zip").unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let registry = SessionRegistry::new();
        let first_id = registry
            .register(
                comic_prepared(root.clone(), file.clone()),
                "main".into(),
                "window-a".into(),
            )
            .unwrap();
        let second_id = registry
            .register(
                comic_prepared(root.clone(), file.clone()),
                "side".into(),
                "window-b".into(),
            )
            .unwrap();
        let first_manifest = registry.comic_manifest(&first_id, "main").unwrap();
        assert_eq!(registry.remove_window("window-a").len(), 1);
        assert!(registry.lookup_for_owner(&first_id, "main").is_err());
        assert!(registry.lookup_for_owner(&second_id, "side").is_ok());

        let reopened_id = registry
            .register(comic_prepared(root, file), "main".into(), "window-a".into())
            .unwrap();
        let reopened = registry.comic_manifest(&reopened_id, "main").unwrap();
        assert_ne!(first_id, reopened_id);
        assert_ne!(first_manifest.pages[0].page_id, reopened.pages[0].page_id);
        assert_ne!(
            first_manifest.pages[0].content_uri,
            reopened.pages[0].content_uri
        );
    }

    #[test]
    fn comic_page_reads_are_bounded_per_session() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("chapter.cbz");
        std::fs::write(&file, b"zip").unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let registry = std::sync::Arc::new(SessionRegistry::new());
        let id = registry
            .register(comic_prepared(root, file), "main".into(), "main".into())
            .unwrap();
        let manifest = registry.comic_manifest(&id, "main").unwrap();
        let grant = grant_id(manifest.pages[0].content_uri.as_deref().unwrap());

        let mut leases = Vec::new();
        for _ in 0..MAX_CONCURRENT_COMIC_PAGE_READS {
            leases.push(registry.begin_comic_page_read(&grant, "main").unwrap());
        }
        assert!(leases[0].prepared.comic_pages.is_none());
        let busy = registry.begin_comic_page_read(&grant, "main").unwrap_err();
        assert_eq!(busy.code().as_str(), "RESOURCE_UNAVAILABLE");
        assert!(busy.retryable());
        drop(leases);
        let lease = registry.begin_comic_page_read(&grant, "main").unwrap();
        drop(lease);
        assert!(registry.begin_comic_page_read(&grant, "main").is_ok());
    }

    /// 关窗事务：同一窗口的第二次关闭请求必须被吞掉，直到第一次事务结束。
    ///
    /// 这不是「顺手去重」：第二个请求摘到的是已经被摘除的**空**会话集合，如果它照样
    /// 销毁窗口，最后一个窗口的销毁会让进程退出，第一次事务还在等的那次统计写就永远
    /// 落不了地——用户关掉应用前读的最后一段就丢了。
    #[test]
    fn window_close_is_coalesced_per_window_and_released_with_the_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("video.mkv");
        std::fs::write(&file, b"video").unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let first = registry
            .register(
                prepared(root.clone(), file.clone()),
                "main".into(),
                "window-a".into(),
            )
            .unwrap();
        let second = registry
            .register(
                prepared(root.clone(), file.clone()),
                "main-2".into(),
                "window-a".into(),
            )
            .unwrap();
        let other = registry
            .register(
                prepared(root.clone(), file.clone()),
                "side".into(),
                "window-b".into(),
            )
            .unwrap();

        let lease = registry
            .begin_window_close("window-a")
            .expect("第一次关闭请求必须取得事务");
        assert_eq!(lease.owner_window_label(), "window-a");
        let mut closed_ids: Vec<String> = lease
            .closed_sessions()
            .iter()
            .map(|closed| closed.session_id.clone())
            .collect();
        closed_ids.sort_unstable();
        let mut expected = vec![first.clone(), second.clone()];
        expected.sort_unstable();
        assert_eq!(closed_ids, expected, "只摘除本窗口拥有的会话");

        assert!(
            registry.begin_window_close("window-a").is_none(),
            "事务结束前，同一窗口的重复关闭请求必须被吞掉"
        );
        assert!(
            registry.lookup_for_owner(&first, "main").is_err(),
            "会话能力在关闭事务开始的那一刻就已摘除"
        );
        assert!(
            registry.lookup_for_owner(&other, "side").is_ok(),
            "关掉一个窗口不得影响别的窗口的会话"
        );

        // 另一个窗口有自己的事务，两个窗口的关闭请求互不吞并。
        let other_lease = registry
            .begin_window_close("window-b")
            .expect("另一个窗口可以独立关闭");
        assert_eq!(other_lease.closed_sessions().len(), 1);
        assert_eq!(other_lease.closed_sessions()[0].session_id, other);
        drop(other_lease);

        // 事务结束后占位释放。这一条针对的是「销毁后重建同名窗口」：占位如果跟着
        // 窗口存不存在走，重建窗口的关闭请求会被当成重复请求吞掉，那个窗口就关不掉，
        // 它的最后一段阅读同样记不上。
        drop(lease);
        let reopened = registry
            .register(prepared(root, file), "main".into(), "window-a".into())
            .unwrap();
        let reopened_lease = registry
            .begin_window_close("window-a")
            .expect("事务结束后同名窗口必须还能再次关闭");
        assert_eq!(reopened_lease.closed_sessions().len(), 1);
        assert_eq!(reopened_lease.closed_sessions()[0].session_id, reopened);
        drop(reopened_lease);
    }

    #[test]
    fn destroyed_window_cleanup_recovers_a_poisoned_registry_lock() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("video.mkv");
        std::fs::write(&file, b"video").unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let registry = Arc::new(SessionRegistry::new());
        let id = registry
            .register(prepared(root, file), "main".into(), "window-a".into())
            .unwrap();

        let poisoned = registry.clone();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = poisoned.state.write().unwrap();
            panic!("test-only lock poison");
        }));
        assert!(panic.is_err());

        let closed = registry.remove_window("window-a");
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].session_id, id);
        assert!(registry.lookup_for_owner(&id, "main").is_err());
    }

    /// 关闭事务进行中的窗口不得再接受新会话；事务结束后同名窗口可以重新打开。
    ///
    /// 这不是「顺手挡一下」：关闭事务只摘除它开始那一刻看到的会话，事务期间登记进来的
    /// 会话不在其中——它既不会被撤销授权，也不会被登记进阅读活动，只会一直留到 TTL 到期，
    /// 而用户关掉应用前读到的那一段永远记不上。占位与登记共用同一把写锁，所以这条用例
    /// 不需要任何时序假设：占位在，登记就必须被拒；占位不在，登记就必须成功。
    #[test]
    fn registering_into_a_closing_window_is_rejected_until_the_transaction_ends() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("video.mkv");
        std::fs::write(&file, b"video").unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let registry = Arc::new(SessionRegistry::new());

        // 事务开始**之前**登记的会话照常被这次事务收走。
        let opened_before = registry
            .register(
                prepared(root.clone(), file.clone()),
                "main".into(),
                "window-a".into(),
            )
            .unwrap();
        let lease = registry
            .begin_window_close("window-a")
            .expect("第一次关闭请求取得事务");
        assert_eq!(lease.closed_sessions().len(), 1);
        assert_eq!(lease.closed_sessions()[0].session_id, opened_before);

        // 事务进行中：同一窗口的每一次新登记都必须被拒绝，且不可重试
        // （窗口关完之前重试没有意义，关完之后调用方会重新发起一次打开）。
        for attempt in 0..2 {
            let error = registry
                .register(
                    prepared(root.clone(), file.clone()),
                    "main".into(),
                    "window-a".into(),
                )
                .unwrap_err();
            assert_eq!(
                error.code().as_str(),
                "SESSION_WINDOW_CLOSING",
                "第 {attempt} 次尝试必须被拒绝"
            );
            assert!(!error.retryable(), "窗口正在关闭不是一次可重试的故障");
        }
        // 被拒绝的登记没有留下任何痕迹：注册表里不属于该窗口的会话一条都没多。
        assert!(
            registry.remove_window("window-a").is_empty(),
            "被拒绝的登记不得留在注册表里"
        );

        // 别的窗口不受影响：占位是按窗口记账的。
        let other = registry
            .register(
                prepared(root.clone(), file.clone()),
                "side".into(),
                "window-b".into(),
            )
            .unwrap();
        assert!(registry.lookup_for_owner(&other, "side").is_ok());

        // 事务结束（窗口已销毁、占位已释放）之后，同名窗口重新打开会话是允许的——
        // 否则销毁后重建的窗口会永远打不开任何内容。
        drop(lease);
        let reopened = registry
            .register(prepared(root, file), "main".into(), "window-a".into())
            .unwrap();
        assert!(registry.lookup_for_owner(&reopened, "main").is_ok());
    }

    /// 结束时刻的观测点：必须在摘除记录的同一个写锁内取，不能由调用方稍后补取。
    ///
    /// 两条关闭入口（显式 `session_close` 与窗口关闭）在摘除之后都还要撤销授权、
    /// 写统计，而这些是异步的；两条都必须自带那一刻的时刻，上层也就没有机会把它
    /// 换成「写统计时的当下」。
    #[test]
    fn a_closed_session_carries_the_timestamp_observed_at_removal() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("video.mkv");
        std::fs::write(&file, b"video").unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let file = std::fs::canonicalize(file).unwrap();
        let registry = Arc::new(SessionRegistry::new());
        registry
            .register(
                prepared(root.clone(), file.clone()),
                "main".into(),
                "window-a".into(),
            )
            .unwrap();
        registry
            .register(
                prepared(root.clone(), file.clone()),
                "main-2".into(),
                "window-a".into(),
            )
            .unwrap();

        let before = UtcMillis::now();
        let lease = registry
            .begin_window_close("window-a")
            .expect("第一个关闭请求取得事务");
        let after = UtcMillis::now();
        assert_eq!(lease.closed_sessions().len(), 2);
        let ended_at = lease.closed_sessions()[0].ended_at;
        for closed in lease.closed_sessions() {
            assert_eq!(
                closed.ended_at, ended_at,
                "一次关闭只观测一次结束时刻，该窗口的全部会话共用它"
            );
            assert!(
                closed.ended_at >= before && closed.ended_at <= after,
                "结束时刻必须落在摘除记录的那一瞬间: {closed:?} 不在 [{before:?}, {after:?}] 内"
            );
            assert!(closed.opened_at <= closed.ended_at, "结束不早于开始");
        }
        drop(lease);

        // 显式 session_close 走的是同一条规则。
        let id = registry
            .register(prepared(root, file), "main".into(), "window-a".into())
            .unwrap();
        let before = UtcMillis::now();
        let closed = registry
            .remove_for_owner(&id, "main")
            .unwrap()
            .expect("第一次关闭必须返回真实事实");
        let after = UtcMillis::now();
        assert!(
            closed.ended_at >= before && closed.ended_at <= after,
            "显式关闭的结束时刻同样来自摘除那一刻: {closed:?} 不在 [{before:?}, {after:?}] 内"
        );
        assert!(
            registry.remove_for_owner(&id, "main").unwrap().is_none(),
            "重复关闭没有第二次观测，也就不会出现第二个结束时刻"
        );
    }
}
