// 外部 Agent 接入设置 Hook（A5 接线切片）：设置页该分组的唯一接线点。
//
// 契约：docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md §4.2、§4.5、§9。
//
// 规则：
// - 组件只通过本 Hook 读状态、开关 Broker；不得直接 invoke。
// - **默认事实来自客户端**：初始状态是「未知 + 读取中」，不预设 disabled，也绝不
//   因为界面想显示点什么就伪造 `listening`。
// - 竞态与卸载都不能让旧响应覆盖新状态：每次请求领一个序号，回来时不是最新的就丢弃。
// - enable / disable 使用命令返回值（同一个严格守卫），失败时保留上一次真实投影并
//   给出可重试错误——失败不会把 listening 悄悄降级成 disabled。

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { toHavenError } from "@/lib/ipc/errors";
import type {
  AgentBrokerStatusDto,
  AgentBrokerStatusResultDto,
} from "@/lib/ipc/generated/wire";

import {
  agentBrokerEndpointKind,
  disableAgentBroker,
  enableAgentBroker,
  readAgentBrokerStatus,
  type AgentBrokerEndpointKind,
} from "../ipc/agent-broker-gateway";

export type AgentBrokerAction = "idle" | "enabling" | "disabling";

export interface AgentBrokerErrorInfo {
  code: string;
  message: string;
  retryable: boolean;
}

export interface AgentBrokerSettingsState {
  /** 首次读取（或重试读取）是否在进行中。 */
  loading: boolean;
  action: AgentBrokerAction;
  /** 客户端给出的最后一份有效投影；读取成功前恒为 null。 */
  result: AgentBrokerStatusResultDto | null;
  error: AgentBrokerErrorInfo | null;
}

export interface AgentBrokerSettingsController {
  /** 状态读取是否在进行中。 */
  loading: boolean;
  action: AgentBrokerAction;
  /** 客户端事实；`null` 表示还没读到，不表示 disabled。 */
  status: AgentBrokerStatusDto | null;
  /** 客户端给出的最后一份有效投影；读取成功前恒为 null。 */
  result: AgentBrokerStatusResultDto | null;
  endpoint: string | null;
  reason: string | null;
  /** `live` 才允许复制端点与展示模板。 */
  endpointKind: AgentBrokerEndpointKind;
  error: AgentBrokerErrorInfo | null;
  /** 读取或开关正在进行：按钮据此禁用。 */
  busy: boolean;
  reload: () => Promise<void>;
  enable: () => Promise<void>;
  disable: () => Promise<void>;
  dismissError: () => void;
}

const EMPTY_STATE: AgentBrokerSettingsState = {
  loading: true,
  action: "idle",
  result: null,
  error: null,
};

function toErrorInfo(error: unknown): AgentBrokerErrorInfo {
  const haven = toHavenError(error);
  return { code: haven.code, message: haven.message, retryable: haven.retryable };
}

export function useAgentBrokerSettings(): AgentBrokerSettingsController {
  const [state, setState] = useState<AgentBrokerSettingsState>(EMPTY_STATE);
  // 单调递增的请求序号：只允许最新一次请求写状态。
  const requestId = useRef(0);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  /** 过期响应与卸载后的响应都不得写入状态。 */
  const isStale = useCallback((id: number): boolean => (
    id !== requestId.current || !mounted.current
  ), []);

  const reload = useCallback(async (): Promise<void> => {
    const id = ++requestId.current;
    setState((current) => ({ ...current, loading: true, error: null }));
    try {
      const result = await readAgentBrokerStatus();
      if (isStale(id)) return;
      setState({ loading: false, action: "idle", result, error: null });
    } catch (error) {
      if (isStale(id)) return;
      setState((current) => ({ ...current, loading: false, error: toErrorInfo(error) }));
    }
  }, [isStale]);

  const runAction = useCallback(async (
    action: Exclude<AgentBrokerAction, "idle">,
  ): Promise<void> => {
    const id = ++requestId.current;
    // 开关命令本身就替代了在途的状态读取：由它接管 loading，避免读取被丢弃后
    // 页面永远停在「读取中」。
    setState((current) => ({ ...current, loading: false, action, error: null }));
    try {
      const result = action === "enabling" ? await enableAgentBroker() : await disableAgentBroker();
      if (isStale(id)) return;
      // 用命令返回值本身，不额外猜测：它已经过与 status 相同的严格守卫。
      setState({ loading: false, action: "idle", result, error: null });
    } catch (error) {
      if (isStale(id)) return;
      // 失败保留上一次真实投影（可能仍是 listening），只补一条可重试错误。
      setState((current) => ({
        ...current,
        loading: false,
        action: "idle",
        error: toErrorInfo(error),
      }));
    }
  }, [isStale]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const enable = useCallback((): Promise<void> => runAction("enabling"), [runAction]);
  const disable = useCallback((): Promise<void> => runAction("disabling"), [runAction]);

  const dismissError = useCallback(() => {
    setState((current) => ({ ...current, error: null }));
  }, []);

  const status = state.result?.status ?? null;
  const endpoint = state.result?.endpoint ?? null;
  const endpointKind = useMemo(
    () => agentBrokerEndpointKind(status, endpoint),
    [status, endpoint],
  );

  return {
    loading: state.loading,
    action: state.action,
    status,
    result: state.result,
    endpoint,
    reason: state.result?.reason ?? null,
    endpointKind,
    error: state.error,
    busy: state.loading || state.action !== "idle",
    reload,
    enable,
    disable,
    dismissError,
  };
}
