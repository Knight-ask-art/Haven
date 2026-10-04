//! `session_open` command (Phase A).

use tauri::{Runtime, State, Webview};

use haven_application::wire::{
    ErrorDto, SessionCloseRequest, SessionCloseResultDto, SessionOpenRequest, SessionOpenResultDto,
};
use haven_domain::ids::{MediaItemId, ReadingSessionId};

use crate::ipc::{invalid_id, run_blocking, to_error_dto};
use crate::session_registry::ClosedSession;
use crate::state::AppState;

pub async fn run_session_open(
    state: &AppState,
    owner_webview_label: &str,
    owner_window_label: &str,
    request: SessionOpenRequest,
) -> Result<SessionOpenResultDto, ErrorDto> {
    let prepared = state
        .session
        .prepare(request)
        .await
        .map_err(|e| to_error_dto(&e))?;
    let exposes_root_content = prepared.engine != haven_application::wire::SessionEngineDto::Comic;
    let result = SessionOpenResultDto {
        schema_version: 1,
        session_id: String::new(),
        content_uri: None,
        work_id: prepared.work_id.clone(),
        edition_id: prepared.edition_id.clone(),
        media_item_id: prepared.media_item_id.clone(),
        engine: prepared.engine,
        progress: prepared.progress.clone(),
        stream_kind: None,
        subtitle_tracks: None,
    };
    let media_item_id_for_history = prepared.media_item_id.clone();
    let session_id = state
        .session_registry
        .register(
            prepared,
            owner_webview_label.to_owned(),
            owner_window_label.to_owned(),
        )
        .map_err(|error| to_error_dto(&error))?;
    // 历史足迹：打开即记（幂等，同 mediaItem 只一条 last_active_at 刷新），失败不影响播放。
    if let Ok(mid) = media_item_id_for_history.parse::<haven_domain::ids::MediaItemId>() {
        let _ = state.history.record(mid).await;
    }
    let mut result = result;
    result.session_id = session_id.clone();
    result.subtitle_tracks = state
        .session_registry
        .subtitle_tracks(&session_id, owner_webview_label)
        .map_err(|error| to_error_dto(&error))?;
    if exposes_root_content {
        result.content_uri = Some(crate::session_registry::SessionRegistry::uri(&session_id));
    }
    Ok(result)
}

/// 把一次**真实闭合**的会话登记进阅读活动事实表。
///
/// 这是「阅读总览」唯一的写入路径，只在真实 Reader/Player 会话闭合处被调用：
/// - `started_at` 是注册表在会话打开时观测到的时刻（调用方无法伪造或改成更早）；
/// - `ended_at` 是注册表在**摘除这条记录的那一刻**观测到的时刻，同样由注册表给出。
///   它不能在这里补取：本函数是异步的，从摘除到真正执行可能隔着一次授权撤销、
///   一次慢查询，甚至可能因为进程退出而根本不执行——那样取到的「当下」既不属于
///   这段阅读，也不是关窗发生的那一刻；
/// - 时长因此恒等于两个观测时刻之差，没有任何调用方传入时长的入口；
/// - 分类由服务层按 `media_item_id` 重新读取 `MediaItem::media_type`（未知类型不记录）；
/// - 幂等键就是运行时会话 ID，同一会话重复上报只命中同一行。
///
/// 登记失败不阻断关闭：关闭是撤销能力的**安全操作**，绝不能因为一次统计写不进去
/// 就让会话授权残留下来。
pub(crate) async fn record_closed_session(state: &AppState, closed: &ClosedSession) {
    let Ok(session_uuid) = uuid::Uuid::parse_str(&closed.session_id) else {
        eprintln!("[reading] 阅读统计跳过：关闭会话身份无效");
        return;
    };
    let Ok(media_item_id) = closed.prepared.media_item_id.parse::<MediaItemId>() else {
        eprintln!("[reading] 阅读统计跳过：媒体身份无效");
        return;
    };
    match state
        .reading_overview
        .record_closed_session(
            ReadingSessionId::from_uuid(session_uuid),
            media_item_id,
            closed.opened_at,
            closed.ended_at,
        )
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("[reading] 阅读会话未计入统计：媒体事实或可信时长不满足统计约束");
        }
        Err(error) => {
            eprintln!("[reading] 阅读统计写入失败: {}", error.code().as_str());
        }
    }
}

pub async fn run_session_close(
    state: &AppState,
    owner_webview_label: &str,
    request: SessionCloseRequest,
) -> Result<SessionCloseResultDto, ErrorDto> {
    let uuid = uuid::Uuid::parse_str(&request.session_id).map_err(|_| invalid_id())?;
    let session_id = uuid.to_string();
    if session_id != request.session_id {
        return Err(invalid_id());
    }
    let closed = state
        .session_registry
        .remove_for_owner(&session_id, owner_webview_label);
    // 撤销媒体授权先于写统计，顺序不能反：`record_closed_session` 是一次 best-effort
    // 的数据库写，慢查询或写失败都不该让远端流的播放授权继续存活——授权是安全能力，
    // 统计只是旁路。`remove_for_owner` 返回的这一刻（会话能力已被摘掉）就是撤销流
    // 授权的最晚时机，所以这里先 revoke、再等统计。
    //
    // 结束时刻也已经在 `remove_for_owner` 内部观测好了（`closed.ended_at`）：显式关闭
    // 与关窗因此用同一个「摘除记录的那一刻」，而不是各自在异步任务里补取当下。
    //
    // A playback lease may be backed by either registry. Revoke the stream grant
    // through the same close command so remote playback cannot outlive the
    // frontend lease.
    state
        .stream_registry
        .revoke(&session_id, owner_webview_label);
    let closed = closed.map_err(|error| to_error_dto(&error))?;
    // 只有真的关掉了一个属于本 WebView 的会话才登记：重复关闭拿到的是 `None`，
    // 因此同一段时间不会被统计两次。撤销本身是幂等的，重复关闭同样只是一次空操作。
    if let Some(closed) = &closed {
        record_closed_session(state, closed).await;
    }
    Ok(SessionCloseResultDto {
        schema_version: 1,
        closed: true,
    })
}

#[tauri::command]
pub async fn session_open<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, AppState>,
    request: SessionOpenRequest,
) -> Result<SessionOpenResultDto, ErrorDto> {
    let owner_webview_label = webview.label().to_owned();
    let owner_window_label = webview.window().label().to_owned();
    let state = (*state.inner()).clone();
    run_blocking(move || async move {
        run_session_open(&state, &owner_webview_label, &owner_window_label, request).await
    })
    .await
}

#[tauri::command]
pub async fn session_close<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, AppState>,
    request: SessionCloseRequest,
) -> Result<SessionCloseResultDto, ErrorDto> {
    let owner_webview_label = webview.label().to_owned();
    let state = (*state.inner()).clone();
    run_blocking(
        move || async move { run_session_close(&state, &owner_webview_label, request).await },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream_registry::StreamGrantFacts;
    use haven_application::services::{PreparedSession, PreparedSessionSource};
    use haven_application::wire::SessionEngineDto;
    use haven_common::UtcMillis;
    use haven_domain::contracts::{
        EditionRepository, MediaItemRepository, ReadingActivityRepository, WorkRepository,
    };
    use haven_domain::entities::{Edition, MediaIndex, MediaItem, Work};
    use haven_domain::enums::{MediaItemStatus, MediaType, ResourceType, WorkStatus, WorkType};
    use haven_domain::ids::{EditionId, ResourceId, WorkId};
    use haven_infrastructure::Db;
    use std::sync::Arc;

    #[tokio::test]
    async fn session_close_rejects_invalid_id_stably() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_session_close(
            &state,
            "main",
            SessionCloseRequest {
                session_id: "not-a-uuid".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ID");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn invalid_id_maps_without_path_or_resource_details() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let error = run_session_open(
            &state,
            "main",
            "main",
            SessionOpenRequest {
                media_item_id: "not-an-id".into(),
                engine: haven_application::wire::SessionEngineDto::Playback,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "INVALID_ID");
        assert!(!error.user_message.contains("/") && !error.user_message.contains("\\"));
    }

    /// 关闭会话必须真正撤销媒体授权：播放租约可能落在任一注册表上，而撤销走的是
    /// 同一条 `session_close` 命令，远端播放不能比前端租约活得更久。
    #[tokio::test]
    async fn session_close_revokes_the_stream_grant() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let upstream = "https://media.example.com/stream.m3u8";
        let grant = state
            .stream_registry
            .register(
                StreamGrantFacts {
                    work_id: "w".into(),
                    edition_id: "e".into(),
                    media_item_id: "m".into(),
                    mime_type: None,
                    is_hls: true,
                    progress: None,
                    upstream_url: upstream.to_owned(),
                },
                upstream,
                "main",
            )
            .unwrap()
            .to_string();
        assert!(state.stream_registry.lookup(&grant, "main").is_some());

        run_session_close(
            &state,
            "main",
            SessionCloseRequest {
                session_id: grant.clone(),
            },
        )
        .await
        .unwrap();

        assert!(
            state.stream_registry.lookup(&grant, "main").is_none(),
            "关闭之后流授权不得继续存活"
        );
        // 重复关闭仍然是幂等的成功（撤销与登记都不会发生第二次）。
        run_session_close(
            &state,
            "main",
            SessionCloseRequest {
                session_id: grant.clone(),
            },
        )
        .await
        .unwrap();
    }

    /// 顺序契约：撤销流授权必须写在登记阅读统计**之前**。
    ///
    /// 统计写库是 best-effort 的旁路，媒体授权是安全能力；顺序一旦反过来，一次慢查询
    /// 或一次写失败就足以让授权留在注册表里。这里断言的是源码里两条调用的先后——
    /// 「慢/失败的数据库写」在单元测试里没有稳定可复现的观察点，而这条顺序是结构性
    /// 的，值得被钉住（关窗路径的同一个顺序见 `lib.rs` 的 `revoke_then_record`，
    /// 由 `lib.rs` 里的源码对接测试钉住）。
    #[test]
    fn close_revokes_the_stream_grant_before_recording_reading_stats() {
        let source = include_str!("session.rs");
        let body = source
            .split_once("pub async fn run_session_close")
            .expect("run_session_close 必须存在")
            .1;
        let revoke_at = body.find(".revoke(").expect("关闭路径必须撤销流授权");
        let record_at = body
            .find("record_closed_session(state, closed)")
            .expect("关闭路径必须登记真实闭合的会话");
        assert!(
            revoke_at < record_at,
            "撤销流授权必须先于写阅读统计：统计慢或失败不能拖住授权撤销"
        );
    }

    /// 结束时刻取自注册表摘除记录的那一刻，而不是写统计时的当下。
    ///
    /// 这里故意交给登记路径一条**历史**会话（构造时给的两个观测值都在一小时之前）。
    /// 如果实现回到「写入前取 `UtcMillis::now()`」，落库的结束时刻会变成「刚才」，
    /// 与观测量差出近一小时——下面两条断言都会失败。
    #[tokio::test]
    async fn a_closed_session_is_recorded_with_the_removal_timestamp() {
        let state = AppState::new(Arc::new(Db::open_in_memory().unwrap()));
        let media_item_id = seed_movie(&state).await;
        let now = UtcMillis::now();
        let opened_at = UtcMillis(now.0 - 3_600_000);
        let ended_at = UtcMillis(now.0 - 3_540_000);
        let closed = ClosedSession {
            session_id: uuid::Uuid::new_v4().to_string(),
            prepared: prepared_session(&media_item_id.to_string()),
            opened_at,
            ended_at,
        };

        record_closed_session(&state, &closed).await;

        let listed = state
            .repos
            .list_sessions_between(UtcMillis(0), UtcMillis(now.0 + 1))
            .await
            .unwrap();
        assert_eq!(listed.len(), 1, "真实闭合的会话必须落库");
        assert_eq!(listed[0].started_at, opened_at);
        assert_eq!(
            listed[0].ended_at, ended_at,
            "结束时刻必须来自摘除记录的那一刻，而不是写统计时的当下"
        );
    }

    /// 夹具：一个可归属分类（Movie → video）的媒体条目，让登记路径真的落库。
    /// 与 `commands/progress.rs` 的夹具同源：Work → Edition → MediaItem 三层都要在，
    /// 否则外键约束先把这条事实挡在库外。
    async fn seed_movie(state: &AppState) -> MediaItemId {
        let work_id = WorkId::new();
        let edition_id = EditionId::new();
        let media_item_id = MediaItemId::new();
        let now = UtcMillis::now();
        state
            .repos
            .work
            .save(&Work {
                id: work_id,
                canonical_title: "关窗统计测试作品".into(),
                original_title: None,
                sort_title: None,
                description: None,
                work_type: WorkType::Fiction,
                release_year: None,
                language: None,
                director: None,
                actor: None,
                status: WorkStatus::Completed,
                rating_value: None,
                rating_scale: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        state
            .repos
            .edition
            .save(&Edition {
                id: edition_id,
                work_id,
                title: "关窗统计测试版本".into(),
                subtitle: None,
                edition_type: MediaType::Movie,
                release_date: None,
                language: None,
                region: None,
                publisher_or_studio: None,
                description: None,
                artwork: Default::default(),
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        state
            .repos
            .media_item
            .save(&MediaItem {
                id: media_item_id,
                edition_id,
                parent_id: None,
                media_type: MediaType::Movie,
                title: "关窗统计测试电影".into(),
                index: MediaIndex::Movie,
                duration_ms: Some(100_000),
                page_count: None,
                chapter_count: None,
                published_at: None,
                status: MediaItemStatus::Available,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        media_item_id
    }

    /// 夹具：登记路径只读 `session_id` 与 `prepared.media_item_id`，
    /// 其余字段取「本地播放会话」的最小合法值。
    fn prepared_session(media_item_id: &str) -> PreparedSession {
        PreparedSession {
            work_id: "w".into(),
            edition_id: "e".into(),
            media_item_id: media_item_id.to_owned(),
            engine: SessionEngineDto::Playback,
            resource_id: ResourceId::new(),
            storage_location_id: None,
            canonical_root: None,
            canonical_file: None,
            subtitle_tracks: Vec::new(),
            source: PreparedSessionSource::Local,
            mime_type: None,
            media_type: MediaType::Movie,
            resource_type: ResourceType::LocalFile,
            comic_pages: None,
            progress: None,
        }
    }
}
