//! 外部 Agent 接入 Broker 的 Tauri 命令（A5 接线切片）。
//!
//! 契约来源：[`docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md`](../../../docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md)
//! §4.5（默认关闭、用户显式开启）与 §12（Tauri 命令 / AppState 接线）。
//!
//! 本层只做三件事：读状态、显式开启、显式关闭。命令**不接受**端点参数，也不选择
//! API 实现——端点由 [`crate::agent_broker`] 按平台自行解析，失败即 fail closed。
//! 这里没有自由 invoke、没有 SQL、没有文件系统调用。
//!
//! 开启时注入的 [`AgentSettingsBrokerApi`] 复用 `AppState` 里同一个
//! `AgentSettingsIpcService`：与 UI 共用一份 UoW 与设置事实源，不新建第二套
//! DB/Repository。Broker 请求集因此天然是 Application service 面的真子集
//! （只有 Read / Propose，没有 approve/reject/apply）。

use std::sync::Arc;

use tauri::State;

use haven_application::services::{AgentContextQueryService, AgentSettingsIpcService};
use haven_application::wire::{AgentBrokerStatusDto, AgentBrokerStatusResultDto, ErrorDto};

use crate::agent_broker::error::BrokerError;
use crate::agent_broker::{AgentBrokerManager, AgentSettingsBrokerApi, BrokerStatus};
use crate::ipc::run_blocking;
use crate::state::AppState;

fn project_status(status: BrokerStatus) -> AgentBrokerStatusResultDto {
    let (status, endpoint, reason) = match status {
        BrokerStatus::Disabled => (AgentBrokerStatusDto::Disabled, None, None),
        BrokerStatus::Listening { endpoint } => {
            (AgentBrokerStatusDto::Listening, Some(endpoint), None)
        }
        BrokerStatus::EndpointBusy => (AgentBrokerStatusDto::Busy, None, None),
        BrokerStatus::Unavailable { reason } => {
            (AgentBrokerStatusDto::Unavailable, None, Some(reason))
        }
    };
    AgentBrokerStatusResultDto {
        schema_version: 1,
        status,
        endpoint,
        reason,
    }
}

impl From<BrokerStatus> for AgentBrokerStatusResultDto {
    fn from(status: BrokerStatus) -> Self {
        project_status(status)
    }
}

/// Broker 稳定错误 → Wire `ErrorDto`。
///
/// `BrokerError` 的 `code` 来自闭合集合，`message` 按构造约定就是脱敏的可展示
/// 文案（见 `agent_broker::error`），因此直接投影，不拼接路径或上游细节。
fn to_error_dto(error: &BrokerError) -> ErrorDto {
    ErrorDto {
        code: error.code().to_owned(),
        user_message: error.message().to_owned(),
        retryable: error.retryable(),
    }
}

// ---------- 命令核心（可在无 Tauri State 的情况下直接调用） ----------

/// 读取当前状态：**不**开启、不探测端点。
pub async fn run_agent_broker_status(
    broker: &AgentBrokerManager,
) -> Result<AgentBrokerStatusResultDto, ErrorDto> {
    Ok(broker.status().await.into())
}

/// 显式开启：创建端点并开始监听；已开启时幂等返回同一端点。
///
/// API 由本层构造，且只可能指向既有的 `AgentSettingsIpcService`——调用方无法
/// 通过参数换掉它。
pub async fn run_agent_broker_enable(
    broker: &AgentBrokerManager,
    agent_settings: &AgentSettingsIpcService,
    agent_context: &AgentContextQueryService,
) -> Result<AgentBrokerStatusResultDto, ErrorDto> {
    let api = Arc::new(AgentSettingsBrokerApi::new(
        agent_settings.clone(),
        agent_context.clone(),
    ));
    broker
        .enable(api)
        .await
        .map(AgentBrokerStatusResultDto::from)
        .map_err(|error| to_error_dto(&error))
}

/// 显式关闭：停止监听并断开在途连接，返回关闭后的状态。
pub async fn run_agent_broker_disable(
    broker: &AgentBrokerManager,
) -> Result<AgentBrokerStatusResultDto, ErrorDto> {
    Ok(broker.disable().await.into())
}

// ---------- Tauri 命令 ----------

/// `agent_broker_status`：外部 Agent 接入的当前状态（默认 `disabled`）。
#[tauri::command]
pub async fn agent_broker_status(
    state: State<'_, AppState>,
) -> Result<AgentBrokerStatusResultDto, ErrorDto> {
    let broker = state.agent_broker.clone();
    run_blocking(move || async move { run_agent_broker_status(&broker).await }).await
}

/// `agent_broker_enable`：用户显式开启外部 Agent 接入。
#[tauri::command]
pub async fn agent_broker_enable(
    state: State<'_, AppState>,
) -> Result<AgentBrokerStatusResultDto, ErrorDto> {
    let broker = state.agent_broker.clone();
    let agent_settings = state.agent_settings.clone();
    let agent_context = state.agent_context.clone();
    run_blocking(move || async move {
        run_agent_broker_enable(&broker, &agent_settings, &agent_context).await
    })
    .await
}

/// `agent_broker_disable`：用户显式关闭外部 Agent 接入。
#[tauri::command]
pub async fn agent_broker_disable(
    state: State<'_, AppState>,
) -> Result<AgentBrokerStatusResultDto, ErrorDto> {
    let broker = state.agent_broker.clone();
    run_blocking(move || async move { run_agent_broker_disable(&broker).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_projection_pins_labels_and_optional_fields() {
        let listening = AgentBrokerStatusResultDto::from(BrokerStatus::Listening {
            endpoint: r"\\.\pipe\haven-agent-v1-3f2a91c4".to_owned(),
        });
        assert_eq!(listening.status, AgentBrokerStatusDto::Listening);
        assert_eq!(
            listening.endpoint.as_deref(),
            Some(r"\\.\pipe\haven-agent-v1-3f2a91c4")
        );
        assert!(listening.reason.is_none());

        // 除 listening 外一律不暴露端点：未开启时 UI 不该有可显示的目标。
        let disabled = AgentBrokerStatusResultDto::from(BrokerStatus::Disabled);
        assert_eq!(disabled.status, AgentBrokerStatusDto::Disabled);
        assert!(disabled.endpoint.is_none() && disabled.reason.is_none());

        let busy = AgentBrokerStatusResultDto::from(BrokerStatus::EndpointBusy);
        assert_eq!(busy.status, AgentBrokerStatusDto::Busy);
        assert!(busy.endpoint.is_none() && busy.reason.is_none());

        let unavailable = AgentBrokerStatusResultDto::from(BrokerStatus::Unavailable {
            reason: "当前平台不支持外部 Agent 接入的本地端点。".to_owned(),
        });
        assert_eq!(unavailable.status, AgentBrokerStatusDto::Unavailable);
        assert!(unavailable.endpoint.is_none());
        assert!(unavailable.reason.is_some());
    }

    #[test]
    fn broker_error_keeps_stable_code_and_retryability() {
        let busy = to_error_dto(&BrokerError::endpoint_busy());
        assert_eq!(busy.code, "HAVEN_BROKER_ENDPOINT_BUSY");
        assert!(!busy.retryable);
        assert!(!busy.user_message.is_empty());

        let timeout = to_error_dto(&BrokerError::timeout());
        assert_eq!(timeout.code, "HAVEN_BROKER_TIMEOUT");
        assert!(timeout.retryable);
    }
}
