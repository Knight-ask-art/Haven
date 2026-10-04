//! 阅读总览应用服务（Reading Overview）。
//!
//! 两条用例，一条写入一条读取，共用同一份持久化事实：
//! - `record_closed_session`：内容会话闭合时把**真实观测到的**开始/结束时刻登记下来。
//!   分类来自 MediaItem 的真实 `media_type`；查不到 MediaItem 或类型未知时**不记录**
//!   （返回 `Ok(false)`），而不是猜一个分类——总览里的类型分布必须每一分钟都有出处。
//! - `overview`：按请求窗口读取会话事实并交给领域聚合。没有任何记录时返回的是
//!   「零条会话」而不是「0 分钟」：可选统计量保持 `None`，页面据此显示空态。
//!
//! 本层不做时区判断，也不从 HistoryEntry 的时间戳或条目数反推时长。

use std::sync::Arc;

use async_trait::async_trait;
use haven_common::{AppError, UtcMillis};
use haven_domain::contracts::{MediaItemRepository, ReadingActivityRepository};
use haven_domain::enums::MediaType;
use haven_domain::ids::{MediaItemId, ReadingSessionId};
use haven_domain::reading_activity::{
    ReadingOverview, ReadingOverviewRequest, ReadingSession, ReadingSessionCategory, aggregate,
};

/// 阅读总览的组合端口：会话事实 + 分类来源。
///
/// 刻意只暴露「取一个 MediaItem 的 media_type」，而不是整个 MediaItemRepository：
/// 这条用例需要的唯一分类事实就是它，端口越窄越难被误用成第二条读取路径。
#[async_trait]
pub trait ReadingOverviewPorts: Send + Sync {
    async fn record_session(&self, session: &ReadingSession) -> Result<(), AppError>;
    async fn list_sessions_between(
        &self,
        from: UtcMillis,
        to: UtcMillis,
    ) -> Result<Vec<ReadingSession>, AppError>;
    /// 该媒体条目的真实类型；条目不存在时返回 `None`。
    async fn media_item_type(&self, id: MediaItemId) -> Result<Option<MediaType>, AppError>;
}

/// 组合根适配：任何同时提供会话事实与 MediaItem 读取的 Repository 都直接是端口，
/// 不需要在 `src-tauri` 里再包一层只做转发的 shim（那会是一条容易与事实源漂移的
/// 平行读取路径）。分类永远来自 `MediaItem::media_type`，不取自调用方。
#[async_trait]
impl<T> ReadingOverviewPorts for T
where
    T: ReadingActivityRepository + MediaItemRepository + Send + Sync,
{
    async fn record_session(&self, session: &ReadingSession) -> Result<(), AppError> {
        ReadingActivityRepository::record_session(self, session).await
    }

    async fn list_sessions_between(
        &self,
        from: UtcMillis,
        to: UtcMillis,
    ) -> Result<Vec<ReadingSession>, AppError> {
        ReadingActivityRepository::list_sessions_between(self, from, to).await
    }

    async fn media_item_type(&self, id: MediaItemId) -> Result<Option<MediaType>, AppError> {
        Ok(MediaItemRepository::get(self, id)
            .await?
            .map(|item| item.media_type))
    }
}

#[derive(Clone)]
pub struct ReadingOverviewService {
    ports: Arc<dyn ReadingOverviewPorts>,
}

impl ReadingOverviewService {
    pub fn new(ports: Arc<dyn ReadingOverviewPorts>) -> Self {
        Self { ports }
    }

    /// 登记一次已闭合的内容会话；返回是否真的记录了。
    ///
    /// 三种情况下返回 `Ok(false)`（都**不是**错误，也不产生任何写入）：
    /// - 会话时长非正或超出可信上限（`ReadingSession::new` 拒绝）；
    /// - MediaItem 不存在（内容已被删除，统计不该凭空长出一条）；
    /// - MediaItem 的类型是 `Unknown`（没有可归属的一级分类）。
    pub async fn record_closed_session(
        &self,
        session_id: ReadingSessionId,
        media_item_id: MediaItemId,
        started_at: UtcMillis,
        ended_at: UtcMillis,
    ) -> Result<bool, AppError> {
        let Some(media_type) = self.ports.media_item_type(media_item_id).await? else {
            return Ok(false);
        };
        let Some(category) = ReadingSessionCategory::from_media_type(media_type) else {
            return Ok(false);
        };
        let Ok(session) =
            ReadingSession::new(session_id, media_item_id, category, started_at, ended_at)
        else {
            return Ok(false);
        };
        self.ports.record_session(&session).await?;
        Ok(true)
    }

    /// 读取总览聚合。`now` 显式传入，保证同一份事实在同一时刻总是得到同一份结果。
    pub async fn overview(
        &self,
        request: ReadingOverviewRequest,
        now: UtcMillis,
    ) -> Result<ReadingOverview, AppError> {
        let (from, to) = request.utc_window(now);
        let sessions = self.ports.list_sessions_between(from, to).await?;
        Ok(aggregate(&sessions, &request, now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryPorts {
        sessions: Mutex<Vec<ReadingSession>>,
        /// `media_item_id → media_type` 的固定夹具（未登记即「条目不存在」）。
        media_types: Mutex<Vec<(MediaItemId, MediaType)>>,
    }

    #[async_trait]
    impl ReadingOverviewPorts for MemoryPorts {
        async fn record_session(&self, session: &ReadingSession) -> Result<(), AppError> {
            self.sessions.lock().unwrap().push(*session);
            Ok(())
        }

        async fn list_sessions_between(
            &self,
            from: UtcMillis,
            to: UtcMillis,
        ) -> Result<Vec<ReadingSession>, AppError> {
            Ok(self
                .sessions
                .lock()
                .unwrap()
                .iter()
                .filter(|session| session.started_at.0 >= from.0 && session.started_at.0 < to.0)
                .copied()
                .collect())
        }

        async fn media_item_type(&self, id: MediaItemId) -> Result<Option<MediaType>, AppError> {
            Ok(self
                .media_types
                .lock()
                .unwrap()
                .iter()
                .find(|(media_item_id, _)| *media_item_id == id)
                .map(|(_, media_type)| *media_type))
        }
    }

    fn service(ports: &Arc<MemoryPorts>) -> ReadingOverviewService {
        ReadingOverviewService::new(ports.clone())
    }

    /// 真实媒体类型 → 可归属分类 → 落库；分类由 MediaItem 事实决定，不由引擎名猜测。
    #[tokio::test]
    async fn a_closed_session_is_recorded_under_its_real_media_type() {
        let ports = Arc::new(MemoryPorts::default());
        let media_item_id = MediaItemId::new();
        ports
            .media_types
            .lock()
            .unwrap()
            .push((media_item_id, MediaType::Article));
        let service = service(&ports);

        let recorded = service
            .record_closed_session(
                ReadingSessionId::new(),
                media_item_id,
                UtcMillis(1_000),
                UtcMillis(61_000),
            )
            .await
            .unwrap();
        assert!(recorded);
        let sessions = ports.sessions.lock().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].category, ReadingSessionCategory::Periodical);
        assert_eq!(sessions[0].duration_ms(), 60_000);
    }

    /// 条目不存在 / 类型未知 / 时长不可信：都不记录，且都不是错误。
    #[tokio::test]
    async fn unknown_subjects_are_skipped_without_inventing_a_category() {
        let ports = Arc::new(MemoryPorts::default());
        let unknown_item = MediaItemId::new();
        ports
            .media_types
            .lock()
            .unwrap()
            .push((unknown_item, MediaType::Unknown));
        let service = service(&ports);

        // 条目不存在。
        assert!(
            !service
                .record_closed_session(
                    ReadingSessionId::new(),
                    MediaItemId::new(),
                    UtcMillis(1_000),
                    UtcMillis(61_000),
                )
                .await
                .unwrap()
        );
        // 类型未知。
        assert!(
            !service
                .record_closed_session(
                    ReadingSessionId::new(),
                    unknown_item,
                    UtcMillis(1_000),
                    UtcMillis(61_000),
                )
                .await
                .unwrap()
        );
        // 时长非正。
        let known_item = MediaItemId::new();
        ports
            .media_types
            .lock()
            .unwrap()
            .push((known_item, MediaType::Book));
        assert!(
            !service
                .record_closed_session(
                    ReadingSessionId::new(),
                    known_item,
                    UtcMillis(61_000),
                    UtcMillis(61_000),
                )
                .await
                .unwrap()
        );

        assert!(ports.sessions.lock().unwrap().is_empty());
    }

    /// 空事实集 → 明确空态（`None` 统计量），而不是伪造的 0 分钟。
    #[tokio::test]
    async fn an_unused_device_overviews_as_an_explicit_empty_result() {
        let ports = Arc::new(MemoryPorts::default());
        let service = service(&ports);
        let request = ReadingOverviewRequest::new(7, 0).unwrap();

        let overview = service
            .overview(request, UtcMillis(1_710_072_000_000))
            .await
            .unwrap();
        assert_eq!(overview.session_count, 0);
        assert_eq!(overview.average_daily_duration_ms, None);
        assert_eq!(overview.recent_week_duration_ms, None);
        assert_eq!(overview.longest_streak_days, None);
        assert!(overview.categories.is_empty());
    }

    /// 记录过的会话真的会在下一次总览里出现（端到端断言，不只是各半边的单测）。
    #[tokio::test]
    async fn recorded_sessions_show_up_in_the_next_overview() {
        let ports = Arc::new(MemoryPorts::default());
        let media_item_id = MediaItemId::new();
        ports
            .media_types
            .lock()
            .unwrap()
            .push((media_item_id, MediaType::Comic));
        let service = service(&ports);
        let now = UtcMillis(1_710_072_000_000);
        service
            .record_closed_session(
                ReadingSessionId::new(),
                media_item_id,
                UtcMillis(now.0 - 45 * 60_000),
                now,
            )
            .await
            .unwrap();

        let overview = service
            .overview(ReadingOverviewRequest::new(7, 0).unwrap(), now)
            .await
            .unwrap();
        assert_eq!(overview.session_count, 1);
        assert_eq!(overview.total_duration_ms, 45 * 60_000);
        assert_eq!(overview.categories.len(), 1);
        assert_eq!(
            overview.categories[0].category,
            ReadingSessionCategory::Comic
        );
        assert_eq!(overview.recent_week_duration_ms, Some(45 * 60_000));
    }
}
