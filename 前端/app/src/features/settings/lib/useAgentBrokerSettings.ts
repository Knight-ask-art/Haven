// 外部 Agent 接入设置 Hook（A5 接线切片）：设置页该分组的唯一接线点。
//
// 契约：docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md §4.2、§4.5、§9。
//
// 规则：
// - 组件只通过本 Hook 读状态、开关 Broker；不得直接 invoke。
// - **默认事实来自客户端**：初始状态是「未知 + 读取中」，不预设 disabled，也绝不
//   因为界面想显示点什么就伪造 `listening`。
// - 竞态与卸载都不能让旧响应覆盖新状态：每次读取领一个序号，回来时不是最新的就丢弃。
// - **读取与变更用两条独立的序号**。这是刻意的：一次读取绝不能作废一次在途的开关。
//   共用一个序号时，用户点「启用」之后任何一次重读（分组刷新按钮、重挂载时的自动读取）
//   都会把开关的返回值丢掉，而 `action` 还停在 `enabling` —— 界面从此永久禁用两个按钮，
//   用户既看不到结果也点不动任何东西。
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
  /** 读取的序号：只允许最新一次**读取**写状态。 */
  const readId = useRef(0);
  /** 变更的序号：与读取分开，因此一次读取不会作废一次在途的开关。 */
  const mutationId = useRef(0);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  /**
   * 过期读取不得写入状态。三条判据：
   * - 已经被更新的读取取代（`readId`）；
   * - 期间发生过开关（`mutationId` 变了）：那次读取拿到的是变更**之前**的快照；
   * - 组件已卸载。
   */
  const isStale = useCallback((id: number, mutation: number): boolean => (
    id !== readId.current || mutation !== mutationId.current || !mounted.current
  ), []);

  const reload = useCallback(async (): Promise<void> => {
    const id = ++readId.current;
    // 读取发起时快照当前的变更序号：期间发生过变更，这次读取的结果就已经过期了。
    const mutation = mutationId.current;
    setState((current) => ({ ...current, loading: true, error: null }));
    try {
      const result = await readAgentBrokerStatus();
      if (isStale(id, mutation)) return;
      setState((current) => ({
        ...current,
        loading: false,
        // **不把 `action` 按回 `idle`。** 一次读取回来时可能还有一条开关命令在途
        // （设置页的「重新加载配置」正是这样触发的：它同时调用读取与开关所属分组的
        // reload）。把 action 复位会让 `busy` 变假、按钮重新可点，用户于是能在第一条
        // 命令还没回来时再发一条互相覆盖的开关；而界面此刻显示的仍是读取拿到的旧快照。
        // 保留 action 让界面如实停在"正在开启 / 正在关闭"，直到那条命令自己落地。
        action: current.action,
        result,
        error: null,
      }));
    } catch (error) {
      if (isStale(id, mutation)) return;
      setState((current) => ({ ...current, loading: false, error: toErrorInfo(error) }));
    }
  }, [isStale]);

  const runAction = useCallback(async (
    action: Exclude<AgentBrokerAction, "idle">,
  ): Promise<void> => {
    const id = ++mutationId.current;
    // 开关命令本身就替代了在途的状态读取：由它接管 loading，避免读取被丢弃后
    // 页面永远停在「读取中」。让读取失效**只**通过读取序号，变更序号此时才推进。
    readId.current += 1;
    setState((current) => ({ ...current, loading: false, action, error: null }));
    try {
      const result = action === "enabling" ? await enableAgentBroker() : await disableAgentBroker();
      if (id !== mutationId.current || !mounted.current) return;
      // **落地时再推进一次读取序号。** 上面那次推进只能作废"开关之前发出"的读取；
      // 一次**在开关在途期间发出**的读取（例如分组刷新按钮，或另一个调用方）拿到的是
      // 变更**之前**的快照，它的响应完全可能落在这一行之后——两次序号的比较都会通过，
      // 于是旧快照覆盖掉刚写入的事实，界面显示的开关状态与真实监听状态相反。
      // 推进之后，任何"此刻之前发出"的读取都不再能写状态。
      readId.current += 1;
      // 用命令返回值本身，不额外猜测：它已经过与 status 相同的严格守卫。
      setState({ loading: false, action: "idle", result, error: null });
    } catch (error) {
      if (id !== mutationId.current || !mounted.current) return;
      // 失败保留上一次真实投影（可能仍是 listening），只补一条可重试错误。
      setState((current) => ({
        ...current,
        loading: false,
        action: "idle",
        error: toErrorInfo(error),
      }));
    }
  }, []);

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
