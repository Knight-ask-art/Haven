// 外部 MCP 客户端自动配置 Hook：设置页「外部 Agent 接入」分组的唯一接线点。
//
// 契约：docs/architecture/MCP_EXTERNAL_AGENT_TRANSPORT.md §9.1.1。
//
// 规则：
// - 组件只通过本 Hook 读状态、写入某一个客户端；不得直接 invoke。
// - **没有本地影子状态**：状态一律来自后端（它会去读真实的配置文件），这里不做乐观更新。
//   乐观更新会让"界面显示已配置"和"客户端重启后能不能连上"变成两件事。
// - 竞态与卸载都不能让旧响应覆盖新状态：读取与写入各领一个序号，写入本身作废在途读取
//   （写入会改变状态，之前读到的快照已经过期）。
// - 写入失败**保留上一次真实状态**并给出可重试错误：把失败悄悄降级成"未配置"会让用户
//   以为文件没被碰过，而失败可能发生在写入之后。

import { useCallback, useEffect, useRef, useState } from "react";

import { toHavenError } from "@/lib/ipc/errors";
import type {
  McpClientConfigStatusWire,
  McpClientTargetWire,
} from "@/lib/ipc/mcp-client-wire";

import { applyMcpClientConfig, fetchMcpClientConfigStatus } from "../ipc/mcp-client-gateway";

export interface McpClientConfigErrorInfo {
  code: string;
  message: string;
  retryable: boolean;
}

export interface McpClientConfigController {
  /** 状态读取是否在进行中。 */
  loading: boolean;
  /** 正在写入的客户端；`null` 表示没有在途写入。 */
  pendingTarget: McpClientTargetWire | null;
  /** 后端给出的最后一份有效状态；读取成功前恒为 null（不预设成"没有配置"）。 */
  status: McpClientConfigStatusWire | null;
  error: McpClientConfigErrorInfo | null;
  /** 读取或写入正在进行：按钮据此禁用。 */
  busy: boolean;
  reload: () => Promise<void>;
  /** 写入一个客户端；返回值只表示这一次写入是否成功拿到权威状态。 */
  configure: (target: McpClientTargetWire) => Promise<boolean>;
  dismissError: () => void;
}

interface McpClientConfigState {
  loading: boolean;
  pendingTarget: McpClientTargetWire | null;
  status: McpClientConfigStatusWire | null;
  error: McpClientConfigErrorInfo | null;
}

const EMPTY_STATE: McpClientConfigState = {
  loading: true,
  pendingTarget: null,
  status: null,
  error: null,
};

function toErrorInfo(error: unknown): McpClientConfigErrorInfo {
  const haven = toHavenError(error);
  return { code: haven.code, message: haven.message, retryable: haven.retryable };
}

export function useMcpClientConfig(): McpClientConfigController {
  const [state, setState] = useState<McpClientConfigState>(EMPTY_STATE);
  /** 读取序号：只允许最新一次读取写状态。 */
  const readId = useRef(0);
  /** 写入序号：与读取分开，因此一次读取不会作废一次在途写入。 */
  const mutationId = useRef(0);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  /** 过期读取不得写入状态：被更新的读取取代 / 期间发生过写入 / 组件已卸载。 */
  const isStale = useCallback(
    (id: number, mutation: number): boolean =>
      id !== readId.current || mutation !== mutationId.current || !mounted.current,
    [],
  );

  const reload = useCallback(async (): Promise<void> => {
    const id = ++readId.current;
    const mutation = mutationId.current;
    setState((current) => ({ ...current, loading: true, error: null }));
    try {
      const status = await fetchMcpClientConfigStatus();
      if (isStale(id, mutation)) return;
      setState({ loading: false, pendingTarget: null, status, error: null });
    } catch (error) {
      if (isStale(id, mutation)) return;
      // 读取失败**不清空**上一次的有效状态之外的任何东西：这里保留 status，
      // 让界面继续显示已知事实，同时如实给出错误。
      setState((current) => ({ ...current, loading: false, error: toErrorInfo(error) }));
    }
  }, [isStale]);

  const configure = useCallback(async (target: McpClientTargetWire): Promise<boolean> => {
    const id = ++mutationId.current;
    // 写入本身会改变状态：作废在途读取，并由写入接管 loading。
    readId.current += 1;
    setState((current) => ({
      ...current,
      loading: false,
      pendingTarget: target,
      error: null,
    }));
    try {
      const status = await applyMcpClientConfig(target);
      if (id !== mutationId.current || !mounted.current) return false;
      setState({ loading: false, pendingTarget: null, status, error: null });
      return true;
    } catch (error) {
      if (id !== mutationId.current || !mounted.current) return false;
      // 失败可能发生在写入之后（例如响应丢失），因此保留上一次真实状态，
      // 只补一条可重试错误——绝不在这里断言"文件没变"。
      setState((current) => ({
        ...current,
        loading: false,
        pendingTarget: null,
        error: toErrorInfo(error),
      }));
      return false;
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const dismissError = useCallback(() => {
    setState((current) => ({ ...current, error: null }));
  }, []);

  return {
    loading: state.loading,
    pendingTarget: state.pendingTarget,
    status: state.status,
    error: state.error,
    busy: state.loading || state.pendingTarget !== null,
    reload,
    configure,
    dismissError,
  };
}
