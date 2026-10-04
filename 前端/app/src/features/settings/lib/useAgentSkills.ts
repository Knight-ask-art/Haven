// 内置技能设置 Hook（原生 Skill 运行时）：设置页「内置技能」分组的唯一接线点。
//
// 契约：docs/architecture/AI_SYSTEM.md §6。
//
// 规则：
// - 组件只通过本 Hook 读状态、逐项启停；不得直接 invoke。
// - **没有本地影子状态**：技能列表与状态一律来自 Rust（SQLite 权威事实 + 编译期
//   嵌入的目录）。这里不写 localStorage，也不做乐观更新——乐观更新会让"看起来
//   启用了"和"真的启用了"在重启前后给出不同答案。
// - 竞态与卸载都不能让旧响应覆盖新状态：每次请求领一个序号，回来时不是最新的就丢弃。

import { useCallback, useEffect, useRef, useState } from "react";

import { toHavenError } from "@/lib/ipc/errors";
import type { AgentSkillStateWire } from "@/lib/ipc/agent-skill-wire";

import {
  listAgentSkills,
  setAgentSkillEnabled,
} from "../ipc/agent-skill-gateway";

export interface AgentSkillErrorInfo {
  code: string;
  message: string;
  retryable: boolean;
}

export interface AgentSkillSettingsController {
  /** 列表读取是否在进行中。 */
  loading: boolean;
  /** 正在写入的技能 id；`null` 表示没有进行中的写入。 */
  pendingSkillId: string | null;
  /** 客户端给出的最后一份有效列表；读取成功前恒为 null（不预设为空目录）。 */
  skills: AgentSkillStateWire[] | null;
  error: AgentSkillErrorInfo | null;
  busy: boolean;
  reload: () => Promise<boolean>;
  /**
   * 启用/停用一项技能。
   *
   * 返回值只表示**这一次写入是否成功拿到权威结果**，调用方据此决定要不要提示成功；
   * 失败时状态由 `error` 如实呈现，并且 Hook 已经回读了权威列表。
   */
  setEnabled: (skillId: string, enabled: boolean) => Promise<boolean>;
  dismissError: () => void;
}

function toErrorInfo(error: unknown): AgentSkillErrorInfo {
  const haven = toHavenError(error);
  return { code: haven.code, message: haven.message, retryable: haven.retryable };
}

/**
 * 把"写入失败"与"回读失败"合成一条消息；只有一边时原样返回。
 *
 * 两件事都要说：写入失败解释了**用户刚才那一按为什么没生效**，回读失败解释了
 * **现在的状态为什么不可信**。只留后者会让界面看起来像"读取出错了"，
 * 而真正需要用户注意的那次写入失败反而消失。
 */
function mergeErrors(
  write: AgentSkillErrorInfo | null,
  read: AgentSkillErrorInfo | null,
): AgentSkillErrorInfo | null {
  if (write === null) return read;
  if (read === null) return write;
  return {
    ...write,
    message: `${write.message}；最新技能状态也读取失败：${read.message}`,
  };
}

export function useAgentSkills(): AgentSkillSettingsController {
  const [loading, setLoading] = useState(true);
  const [pendingSkillId, setPendingSkillId] = useState<string | null>(null);
  const [skills, setSkills] = useState<AgentSkillStateWire[] | null>(null);
  const [error, setError] = useState<AgentSkillErrorInfo | null>(null);
  const requestId = useRef(0);
  const mutationInFlight = useRef(false);
  const mounted = useRef(true);
  /**
   * 最近一次**写入**失败，跨重读保留。
   *
   * 为什么不能只放在 `error` 状态里：一次成功的列表读取只能证明"现在的状态是这样"，
   * 不能证明"刚才那次启停为什么没生效"。若重读成功就把错误清掉，用户点了「重新读取」
   * 之后界面会恢复成一切正常的样子，而他那次没生效的启停彻底失去解释。因此写入失败
   * 只由两件事清除：下一次写入成功，或用户显式关闭。
   */
  const writeError = useRef<AgentSkillErrorInfo | null>(null);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const isStale = useCallback(
    (id: number): boolean => id !== requestId.current || !mounted.current,
    [],
  );

  const reload = useCallback(async () => {
    const id = ++requestId.current;
    setLoading(true);
    try {
      const result = await listAgentSkills();
      if (isStale(id)) return false;
      setSkills(result.skills);
      // 清掉的是**读取**错误；挂起的写入失败照旧呈现（见 writeError 的说明）。
      setError(writeError.current);
      return true;
    } catch (caught) {
      if (isStale(id)) return false;
      // 最近一次读取失败后，旧投影不再有权威性：清空它，避免用户继续操作过期状态。
      setSkills(null);
      setError(mergeErrors(writeError.current, toErrorInfo(caught)));
      return false;
    } finally {
      if (!isStale(id)) setLoading(false);
    }
  }, [isStale]);

  useEffect(() => {
    void reload();
  }, [reload]);

  const setEnabled = useCallback(
    async (skillId: string, enabled: boolean): Promise<boolean> => {
      // UI 会禁用并发按钮，但 Hook 也必须自守：别的调用方或双击不能并发写同一份授权状态。
      if (mutationInFlight.current || !mounted.current) return false;
      mutationInFlight.current = true;
      // 任何在途列表读取都可能早于这次写入，先让它失效。
      requestId.current += 1;
      setPendingSkillId(skillId);
      try {
        // 用命令返回值替换该项：状态以后端为准，不由前端推算。
        const updated = await setAgentSkillEnabled(skillId, enabled);
        if (!mounted.current) return true;
        // 写入期间可能有人手动触发重读；该响应也可能是写入提交前的旧快照。
        // 使其失效后，用命令返回的权威单项结果收敛当前列表。
        requestId.current += 1;
        setLoading(false)
        setSkills((current) => {
          if (current === null) return current;
          return current.map((skill) => (skill.skillId === updated.skillId ? updated : skill));
        });
        // 这次写入成功了：上一次写入失败已经没有解释价值，连同挂起状态一起清掉。
        writeError.current = null;
        setError(null);
        return true;
      } catch (caught) {
        if (mounted.current) {
          // 记在 ref 上而不是只记在 `error` 里：随后的重读成功不得把它清掉。
          writeError.current = toErrorInfo(caught);
          // 失败后回读权威状态：失败可能发生在写入之后（例如响应丢失），
          // 前端推测"没写成"会与权威事实分叉。回读成功时由 `reload` 把这次
          // 写入失败重新装上；回读也失败时由它把两件事合成一条消息。
          await reload();
        }
        return false;
      } finally {
        mutationInFlight.current = false;
        if (mounted.current) setPendingSkillId(null);
      }
    },
    [reload],
  );

  const dismissError = useCallback(() => {
    // 显式关闭是用户"我看过了"的表达，连挂起的写入失败一起清掉。
    writeError.current = null;
    setError(null);
  }, []);

  return {
    loading,
    pendingSkillId,
    skills,
    error,
    busy: loading || pendingSkillId !== null,
    reload,
    setEnabled,
    dismissError,
  };
}
