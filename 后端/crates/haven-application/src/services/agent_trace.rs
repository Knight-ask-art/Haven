//! Agent 后端事件轨迹的 typed 基础。
//!
//! 事件是给未来 UI / 诊断投影使用的最小审计事实，不是模型消息历史，也不是调试日志。
//! 这里没有自由文本、原始 Provider 响应、路径、SQL 或 secret 字段；所有写操作仍然
//! 必须通过 Proposal / Approval / CAS 服务完成。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use haven_common::{AppError, ErrorKind, UtcMillis};
use haven_domain::ids::{AgentContextSnapshotId, AgentRequestId, AgentSessionId};
use haven_domain::setting_proposal::is_canonical_digest;

use crate::wire::{
    AgentTraceEventDto, AgentTraceEventKindDto, AgentTraceGetRequest, AgentTraceGetResultDto,
};

pub const MAX_TRACE_EVENTS_PER_SESSION: usize = 256;
/// Collector 的全局会话上限。达到上限后拒绝新的 session，而不是无界增长或
/// 随机淘汰仍可能被 UI 读取的旧轨迹。
pub const MAX_TRACE_SESSIONS: usize = 1_024;

/// Agent 轨迹的闭合事件集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentEventKind {
    RequestStarted,
    ContextLoaded,
    ProviderRequest,
    ProviderResponse,
    StructuredOutputValidated,
    ProposalCreated,
    WaitingForApproval,
    ApprovalRejected,
    CasStarted,
    Applied,
    ReceiptCreated,
    Cancelled,
    Retrying,
    Failed,
}

/// 事件绑定的上下文身份。只保留可重新核对的 id/hash，不保留上下文正文。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AgentTraceContext {
    pub context_id: AgentContextSnapshotId,
    pub context_hash: String,
}

impl AgentTraceContext {
    pub fn new(
        context_id: AgentContextSnapshotId,
        context_hash: impl Into<String>,
    ) -> Result<Self, AppError> {
        let context_hash = context_hash.into();
        if context_id.as_uuid().is_nil() || !is_canonical_digest(&context_hash) {
            return Err(trace_invalid("轨迹上下文身份或 hash 非法"));
        }
        Ok(Self {
            context_id,
            context_hash,
        })
    }
}

/// 事件草稿。序号由 collector 在接收时分配，调用方不能伪造顺序。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTraceEventDraft {
    pub session_id: AgentSessionId,
    pub request_id: AgentRequestId,
    pub context: AgentTraceContext,
    pub kind: AgentEventKind,
    pub duration_ms: Option<u64>,
}

impl AgentTraceEventDraft {
    pub fn new(
        session_id: AgentSessionId,
        request_id: AgentRequestId,
        context: AgentTraceContext,
        kind: AgentEventKind,
        duration_ms: Option<u64>,
    ) -> Result<Self, AppError> {
        if session_id.as_uuid().is_nil() || request_id.as_uuid().is_nil() {
            return Err(trace_invalid("轨迹身份不得是 nil UUID"));
        }
        Ok(Self {
            session_id,
            request_id,
            context,
            kind,
            duration_ms,
        })
    }
}

/// collector 产出的不可变事件。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTraceEvent {
    pub session_id: AgentSessionId,
    pub request_id: AgentRequestId,
    pub context: AgentTraceContext,
    pub sequence: u64,
    pub kind: AgentEventKind,
    pub duration_ms: Option<u64>,
    pub occurred_at: UtcMillis,
}

#[async_trait]
pub trait AgentTracePort: Send + Sync {
    async fn record(&self, draft: AgentTraceEventDraft) -> Result<AgentTraceEvent, AppError>;
}

/// Best-effort 轨迹写入辅助。
///
/// 轨迹是诊断/未来 UI 的旁路事实，不能改变 Proposal、CAS 或 Receipt 的业务结果。
/// 因此上下文解析、草稿校验和 collector 写入的任何失败都会被有意吞掉；调用方只需
/// 在业务关键节点调用本函数，不得为了记录轨迹而复制一套业务错误边界。
pub async fn record_best_effort(
    trace: Option<&Arc<dyn AgentTracePort>>,
    session_id: AgentSessionId,
    request_id: AgentRequestId,
    context_id: &str,
    context_hash: &str,
    kind: AgentEventKind,
) {
    let Some(trace) = trace else {
        return;
    };
    let Ok(context_id) = context_id.parse::<AgentContextSnapshotId>() else {
        return;
    };
    let Ok(context) = AgentTraceContext::new(context_id, context_hash.to_owned()) else {
        return;
    };
    let Ok(draft) = AgentTraceEventDraft::new(session_id, request_id, context, kind, None) else {
        return;
    };
    let _ = trace.record(draft).await;
}

/// 轨迹读取端口。写入端口与读取端口分开，便于未来把 collector 替换成持久化/事件总线
/// 实现，而不会把 UI 读取能力混入 Proposal/CAS 写入路径。
#[async_trait]
pub trait AgentTraceQueryPort: Send + Sync {
    async fn read(&self, session_id: AgentSessionId) -> Result<Vec<AgentTraceEvent>, AppError>;
}

/// 轨迹读取 Application service。它只返回闭合、脱敏、有限事件 DTO。
#[derive(Clone)]
pub struct AgentTraceQueryService {
    query: Arc<dyn AgentTraceQueryPort>,
}

impl AgentTraceQueryService {
    pub fn new(query: Arc<dyn AgentTraceQueryPort>) -> Self {
        Self { query }
    }

    pub async fn get(
        &self,
        request: &AgentTraceGetRequest,
    ) -> Result<AgentTraceGetResultDto, AppError> {
        let session_id = request
            .session_id
            .parse::<AgentSessionId>()
            .map_err(|_| trace_invalid("轨迹会话 ID 非法"))?;
        if session_id.as_uuid().is_nil() {
            return Err(trace_invalid("轨迹会话 ID 不得是 nil UUID"));
        }
        // QueryPort 的实现属于可替换边界；即便未来接入持久化实现，也不能把
        // 越界会话或无界结果直接投影给 UI。内存 collector 本身已经有同样的限制，
        // 这里再做一次 Application 层防线，保持 typed 读取契约闭合。
        let events = self
            .query
            .read(session_id)
            .await?
            .into_iter()
            .filter(|event| event.session_id == session_id)
            .take(MAX_TRACE_EVENTS_PER_SESSION)
            .collect::<Vec<_>>();
        Ok(AgentTraceGetResultDto {
            schema_version: 1,
            session_id: session_id.to_string(),
            events: events.into_iter().map(trace_event_to_dto).collect(),
        })
    }
}

/// 有界内存 collector。它是后端基础实现与测试替身；持久化/事件总线可以实现同一个 port，
/// 但不能扩展事件字段去携带模型原文或敏感材料。
#[derive(Clone, Default)]
pub struct InMemoryAgentTraceCollector {
    sessions: Arc<Mutex<HashMap<AgentSessionId, Vec<AgentTraceEvent>>>>,
}

impl InMemoryAgentTraceCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self, session_id: AgentSessionId) -> Vec<AgentTraceEvent> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&session_id)
            .cloned()
            .unwrap_or_default()
    }
}

#[async_trait]
impl AgentTracePort for InMemoryAgentTraceCollector {
    async fn record(&self, draft: AgentTraceEventDraft) -> Result<AgentTraceEvent, AppError> {
        let mut sessions = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !sessions.contains_key(&draft.session_id) && sessions.len() >= MAX_TRACE_SESSIONS {
            return Err(AppError::new(
                "AGENT_TRACE_SESSION_LIMIT_EXCEEDED",
                ErrorKind::Conflict,
                "Agent 轨迹会话数量已达到上限",
                false,
            ));
        }
        let events = sessions.entry(draft.session_id).or_default();
        if events.len() >= MAX_TRACE_EVENTS_PER_SESSION {
            return Err(AppError::new(
                "AGENT_TRACE_LIMIT_EXCEEDED",
                ErrorKind::Conflict,
                "Agent 轨迹已达到本次会话上限",
                false,
            ));
        }
        let event = AgentTraceEvent {
            session_id: draft.session_id,
            request_id: draft.request_id,
            context: draft.context,
            sequence: events.len() as u64 + 1,
            kind: draft.kind,
            duration_ms: draft.duration_ms,
            occurred_at: UtcMillis::now(),
        };
        events.push(event.clone());
        Ok(event)
    }
}

#[async_trait]
impl AgentTraceQueryPort for InMemoryAgentTraceCollector {
    async fn read(&self, session_id: AgentSessionId) -> Result<Vec<AgentTraceEvent>, AppError> {
        Ok(self.snapshot(session_id))
    }
}

fn trace_event_to_dto(event: AgentTraceEvent) -> AgentTraceEventDto {
    AgentTraceEventDto {
        session_id: event.session_id.to_string(),
        request_id: event.request_id.to_string(),
        context_id: event.context.context_id.to_string(),
        context_hash: event.context.context_hash,
        sequence: event.sequence,
        kind: match event.kind {
            AgentEventKind::RequestStarted => AgentTraceEventKindDto::RequestStarted,
            AgentEventKind::ContextLoaded => AgentTraceEventKindDto::ContextLoaded,
            AgentEventKind::ProviderRequest => AgentTraceEventKindDto::ProviderRequest,
            AgentEventKind::ProviderResponse => AgentTraceEventKindDto::ProviderResponse,
            AgentEventKind::StructuredOutputValidated => {
                AgentTraceEventKindDto::StructuredOutputValidated
            }
            AgentEventKind::ProposalCreated => AgentTraceEventKindDto::ProposalCreated,
            AgentEventKind::WaitingForApproval => AgentTraceEventKindDto::WaitingForApproval,
            AgentEventKind::ApprovalRejected => AgentTraceEventKindDto::ApprovalRejected,
            AgentEventKind::CasStarted => AgentTraceEventKindDto::CasStarted,
            AgentEventKind::Applied => AgentTraceEventKindDto::Applied,
            AgentEventKind::ReceiptCreated => AgentTraceEventKindDto::ReceiptCreated,
            AgentEventKind::Cancelled => AgentTraceEventKindDto::Cancelled,
            AgentEventKind::Retrying => AgentTraceEventKindDto::Retrying,
            AgentEventKind::Failed => AgentTraceEventKindDto::Failed,
        },
        duration_ms: event.duration_ms,
        occurred_at: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(event.occurred_at.0)
            .map(|at| at.to_rfc3339())
            .unwrap_or_else(|| event.occurred_at.0.to_string()),
    }
}

fn trace_invalid(message: &'static str) -> AppError {
    AppError::new("AGENT_TRACE_INVALID", ErrorKind::Validation, message, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "a";

    fn context() -> AgentTraceContext {
        AgentTraceContext::new(AgentContextSnapshotId::new(), HASH.repeat(64)).unwrap()
    }

    #[tokio::test]
    async fn collector_assigns_monotonic_sequence_without_free_form_payload() {
        let collector = InMemoryAgentTraceCollector::new();
        let session = AgentSessionId::new();
        let request = AgentRequestId::new();
        let first = collector
            .record(
                AgentTraceEventDraft::new(
                    session,
                    request,
                    context(),
                    AgentEventKind::RequestStarted,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let second = collector
            .record(
                AgentTraceEventDraft::new(
                    session,
                    request,
                    context(),
                    AgentEventKind::ProposalCreated,
                    Some(12),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.sequence, 1);
        assert_eq!(second.sequence, 2);
        assert_eq!(collector.snapshot(session).len(), 2);
        let json = serde_json::to_string(&second).unwrap();
        for forbidden in ["apiKey", "rawResponse", "absolutePath", "sql", "secret"] {
            assert!(!json.contains(forbidden));
        }
    }

    #[tokio::test]
    async fn invalid_identity_and_hash_fail_before_collection() {
        assert!(AgentTraceContext::new(AgentContextSnapshotId::new(), "bad").is_err());
        assert!(
            AgentTraceContext::new(
                AgentContextSnapshotId::from_uuid(uuid::Uuid::nil()),
                HASH.repeat(64),
            )
            .is_err()
        );
        assert!(
            AgentTraceEventDraft::new(
                AgentSessionId::from_uuid(uuid::Uuid::nil()),
                AgentRequestId::new(),
                context(),
                AgentEventKind::Failed,
                None,
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn collector_is_bounded() {
        let collector = InMemoryAgentTraceCollector::new();
        let session = AgentSessionId::new();
        for _ in 0..MAX_TRACE_EVENTS_PER_SESSION {
            collector
                .record(
                    AgentTraceEventDraft::new(
                        session,
                        AgentRequestId::new(),
                        context(),
                        AgentEventKind::Retrying,
                        None,
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
        }
        let error = collector
            .record(
                AgentTraceEventDraft::new(
                    session,
                    AgentRequestId::new(),
                    context(),
                    AgentEventKind::Failed,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_TRACE_LIMIT_EXCEEDED");
    }

    #[tokio::test]
    async fn collector_rejects_new_sessions_after_global_limit() {
        let collector = InMemoryAgentTraceCollector::new();
        for _ in 0..MAX_TRACE_SESSIONS {
            collector
                .record(
                    AgentTraceEventDraft::new(
                        AgentSessionId::new(),
                        AgentRequestId::new(),
                        context(),
                        AgentEventKind::RequestStarted,
                        None,
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
        }
        let error = collector
            .record(
                AgentTraceEventDraft::new(
                    AgentSessionId::new(),
                    AgentRequestId::new(),
                    context(),
                    AgentEventKind::Failed,
                    None,
                )
                .unwrap(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_TRACE_SESSION_LIMIT_EXCEEDED");
    }

    #[tokio::test]
    async fn query_service_returns_bounded_closed_dto_for_the_requested_session() {
        let collector = Arc::new(InMemoryAgentTraceCollector::new());
        let session = AgentSessionId::new();
        let request = AgentRequestId::new();
        collector
            .record(
                AgentTraceEventDraft::new(
                    session,
                    request,
                    context(),
                    AgentEventKind::RequestStarted,
                    Some(7),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let service = AgentTraceQueryService::new(collector);
        let result = service
            .get(&AgentTraceGetRequest {
                session_id: session.to_string(),
            })
            .await
            .unwrap();

        assert_eq!(result.schema_version, 1);
        assert_eq!(result.session_id, session.to_string());
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].sequence, 1);
        assert_eq!(
            result.events[0].kind,
            AgentTraceEventKindDto::RequestStarted
        );
        assert_eq!(result.events[0].duration_ms, Some(7));
        let json = serde_json::to_string(&result).unwrap();
        for forbidden in ["apiKey", "rawResponse", "absolutePath", "sql", "secret"] {
            assert!(!json.contains(forbidden), "轨迹 DTO 不得包含 {forbidden}");
        }
    }

    #[tokio::test]
    async fn query_service_rejects_nil_session_without_reading() {
        let collector = Arc::new(InMemoryAgentTraceCollector::new());
        let service = AgentTraceQueryService::new(collector);
        let nil = AgentSessionId::from_uuid(uuid::Uuid::nil()).to_string();

        let error = service
            .get(&AgentTraceGetRequest { session_id: nil })
            .await
            .unwrap_err();
        assert_eq!(error.code().as_str(), "AGENT_TRACE_INVALID");
    }

    #[tokio::test]
    async fn best_effort_trace_failure_does_not_escape_when_collector_is_full() {
        let collector = Arc::new(InMemoryAgentTraceCollector::new());
        let trace: Arc<dyn AgentTracePort> = collector.clone();
        let session = AgentSessionId::new();
        let request = AgentRequestId::new();
        let trace_context = context();
        let context_id = trace_context.context_id.to_string();
        for _ in 0..MAX_TRACE_EVENTS_PER_SESSION {
            record_best_effort(
                Some(&trace),
                session,
                request,
                &context_id,
                &trace_context.context_hash,
                AgentEventKind::RequestStarted,
            )
            .await;
        }
        // A full collector must not turn a best-effort diagnostic write into a business error.
        record_best_effort(
            Some(&trace),
            session,
            request,
            &context_id,
            &trace_context.context_hash,
            AgentEventKind::ReceiptCreated,
        )
        .await;
        assert_eq!(
            collector.snapshot(session).len(),
            MAX_TRACE_EVENTS_PER_SESSION
        );
    }
}
