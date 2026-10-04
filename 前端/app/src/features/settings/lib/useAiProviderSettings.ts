// AI Provider 设置 Hook（A2 基础切片）：设置页 AI 分组的唯一接线点。
//
// 契约：docs/architecture/AI_SYSTEM.md §3、§4、§8。
//
// 规则：
// - 组件只通过本 Hook 读写 Profile / 凭据状态 / 模型目录；不得直接 invoke。
// - **API Key 只单向提交**：`submitApiKey(secret)` 之后本 Hook 不保存、不回填、
//   不返回该字符串；界面只能显示 `credentialConfigured`。
// - **没有可用模型时显示「无可用模型」**：不写入、不推断任何示例模型名，
//   也不会把 `selectedModelId` 的原值当成已确认的选择。
// - Provider 错误只做可重试展示：`errorRetryable` 直接来自后端 ErrorDto。

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { toHavenError } from "@/lib/ipc/errors";
import type {
  AiProviderModelsCatalogDto,
  AiProviderProfileDto,
} from "@/lib/ipc/generated/wire";

import {
  INSECURE_ENDPOINT_CODE,
  INSECURE_ENDPOINT_MESSAGE,
  NO_AVAILABLE_MODEL,
  catalogStateHint,
  clearAiProviderApiKey,
  deleteAiProviderProfile,
  isHttpsAiEndpoint,
  listAiProviderModels,
  listAiProviderProfiles,
  setAiProviderApiKey,
  toModelOptions,
  upsertAiProviderProfile,
  type AiProviderModelOption,
} from "../ipc/ai-provider-gateway";

export interface AiProviderErrorInfo {
  code: string;
  message: string;
  retryable: boolean;
}

/**
 * 失败的那一次操作。
 *
 * 存在的理由只有一个：**重试按钮必须重试失败的那件事**。曾经的界面把保存失败的
 * "重试"接到"拉取模型"上——用户点一次"重试"，看到的是一次网络请求和一份新目录，
 * 而真正失败的写入仍然没有发生。把动作记下来，"重试"才有可能名副其实。
 */
export type AiProviderFailedAction = "profile" | "credential" | "delete";

export interface AiProviderSettingsState {
  loading: boolean;
  loadError: AiProviderErrorInfo | null;
  profiles: AiProviderProfileDto[];
  selectedProfileId: string | null;
  catalog: AiProviderModelsCatalogDto | null;
  catalogLoading: boolean;
  catalogError: AiProviderErrorInfo | null;
  saving: boolean;
  saveError: AiProviderErrorInfo | null;
  /** 失败的是哪一次操作；`null` 表示当前没有失败。 */
  saveErrorAction: AiProviderFailedAction | null;
}

/**
 * 调用方在**分区之上**记住的选择。
 *
 * 设置页的每个分区各自挂载自己的设置组件：从「智能功能」切到别的分区再切回来，
 * `AiSettings` 会连同本 Hook 一起重新挂载，组件内的 `selectedRef` 归零。没有这枚
 * 记忆，重挂载就会静默回落到 `profiles[0]`——用户明明选过第二个配置，回来却变成第一个，
 * 而「打开栖伴」正是从这里取 profileId 的，于是外部模型请求被发给了另一个 Provider，
 * 界面上却没有任何一处提示这件事。
 *
 * 它**不是**一个新的持久化设置：不进 settings wire、不写 localStorage、不改后端契约，
 * 只是把"用户此刻选的是哪一个"放在分区切换摧毁不到的地方。Rust + SQLite 仍是唯一事实源。
 */
export interface AiProviderSettingsOptions {
  /** 首次读取列表时的落点；`null`/未给出时沿用组件内选择，再退到列表第一项。 */
  initialProfileId?: string | null;
  /** 选择变化（含读取后的收敛结果）时回调，供调用方在分区切换之间记住它。 */
  onSelectedProfileChange?: (profileId: string | null) => void;
}

export interface AiProviderProfileDraft {
  profileId: string;
  displayName: string;
  endpoint: string;
  enabled: boolean;
  selectedModelId: string | null;
  /** 新建模式必须使用 create-only CAS；不能把同 id 的现有配置当成更新。 */
  createOnly?: boolean;
}

export interface AiProviderSettingsController {
  state: AiProviderSettingsState;
  selectedProfile: AiProviderProfileDto | null;
  modelOptions: AiProviderModelOption[];
  /** 只有拿到 `ready` 目录且至少一条模型时为 true。 */
  hasAvailableModel: boolean;
  /** 空态文案：恒为「无可用模型」。 */
  unavailableModelLabel: string;
  /** 无可用模型时的原因说明（未配置密钥 / 已停用 / 目录为空 / 尚未读取）。 */
  unavailableModelHint: string;
  /** 凭据事实；界面据此显示「已配置 / 未配置」。 */
  credentialConfigured: boolean;
  /** 选中的 profile 是否使用明文端点（历史遗留行）。 */
  selectedEndpointInsecure: boolean;
  /** 保存失败能否重试：只有"重试真的会重做那件事"时才为 true。 */
  canRetrySaveError: boolean;
  selectProfile: (profileId: string | null) => void;
  reload: () => Promise<void>;
  refreshModels: () => Promise<void>;
  saveProfile: (draft: AiProviderProfileDraft) => Promise<boolean>;
  /** 重试失败的那一次操作（凭据写入不可重试：密钥原文不留存）。 */
  retrySaveError: () => Promise<boolean>;
  /** 单向提交 API Key；成功后只更新「已配置」事实。 */
  submitApiKey: (secret: string) => Promise<boolean>;
  clearApiKey: () => Promise<boolean>;
  deleteProfile: () => Promise<boolean>;
  dismissSaveError: () => void;
}

const EMPTY_STATE: AiProviderSettingsState = {
  loading: true,
  loadError: null,
  profiles: [],
  selectedProfileId: null,
  catalog: null,
  catalogLoading: false,
  catalogError: null,
  saving: false,
  saveError: null,
  saveErrorAction: null,
};

function toErrorInfo(error: unknown): AiProviderErrorInfo {
  const haven = toHavenError(error);
  return { code: haven.code, message: haven.message, retryable: haven.retryable };
}

export function useAiProviderSettings(options: AiProviderSettingsOptions = {}): AiProviderSettingsController {
  const [state, setState] = useState<AiProviderSettingsState>(EMPTY_STATE);
  // Profile 列表与模型目录是两个独立的异步资源；刷新模型不能让列表请求失效，反之亦然。
  const profileLoadId = useRef(0);
  const catalogRequestId = useRef(0);
  const selectedRef = useRef<string | null>(null);
  selectedRef.current = state.selectedProfileId;
  /**
   * 调用方记住的选择。**只在挂载时读一次**，而且用 ref 而不是直接闭包捕获：`load` 是
   * 依赖为 `[]` 的稳定回调，它的身份被 `useEffect(…, [load])` 依赖着；把 `options`
   * 放进依赖或让 `load` 每次重建，都会让"挂载时读一次列表"变成每渲染一次读一次。
   */
  const initialProfileIdRef = useRef<string | null>(options.initialProfileId ?? null);
  const selectionListenerRef = useRef(options.onSelectedProfileChange);
  selectionListenerRef.current = options.onSelectedProfileChange;
  /** 选择收敛后告知调用方；失败不该影响本 Hook 的任何行为。 */
  const notifySelection = useCallback((profileId: string | null) => {
    selectionListenerRef.current?.(profileId);
  }, []);
  /**
   * 凭据写入 / 删除的互斥闸门。
   *
   * 设置 / 清除是两条**互相覆盖**的写操作：同时放行它们，"先点保存、再点清除"的最终
   * 结果由两个请求的完成顺序决定，而界面显示的是最后一次回读——用户会看到"未配置"
   * 却可能真的写进去了（或反过来）。UI 也会禁用按钮，但 Hook 必须自守：双击、快速
   * 切换或别的调用方都不该绕过这条闸门。
   */
  const credentialMutationInFlight = useRef(false);
  /** 最近一次提交的 profile 草稿：重试"保存"时必须重放**同一份**输入，而不是重新拼一份。 */
  const lastSaveDraft = useRef<AiProviderProfileDraft | null>(null);

  /**
   * 失败的那一次删除指向的是**哪一个** profile。
   *
   * 删除的重试必须重放"用户当时要删的那一份"，而不是"此刻选中的那一份"：删除失败之后，
   * 用户完全可以切到另一个 profile 去看别的配置，再回来点「重试」。如果重试读的是当前
   * 选中项，被删掉的就是他刚刚切过去的那一份——一次发生在界面上毫无提示的数据丢失。
   *
   * 连 revision 一起记下来还有第二个用处：revision 是服务端 CAS 的依据，如果这一份在这
   * 期间被改过，重试会以冲突告终，而不是删掉一个用户没看过的版本。
   */
  const lastDeleteTarget = useRef<{ profileId: string; revision: string } | null>(null);

  /**
   * 读取 Profile 列表，并在必要时顺带读取模型目录。
   * `preferredProfileId` 让删除/新建后仍能停留在用户刚才看的那个 profile 上。
   */
  const load = useCallback(async (preferredProfileId?: string | null): Promise<void> => {
    const id = ++profileLoadId.current;
    // 刷新列表期间不再展示旧的模型加载状态；如果选中项变了，后续 effect 会重新读取。
    catalogRequestId.current += 1;
    setState((current) => ({
      ...current,
      loading: true,
      loadError: null,
      catalogLoading: false,
    }));
    try {
      const profiles = await listAiProviderProfiles();
      if (id !== profileLoadId.current) return;
      // 组件内选择 → 调用方记住的选择 → 列表第一项。
      //
      // 中间那一项专门用来跨"分区切换导致的重新挂载"存活：少了它，重新挂载会直接落到
      // `profiles[0]`，把用户选过的配置悄悄换成另一个。「打开栖伴」取的正是这个值。
      const desired = preferredProfileId ?? selectedRef.current ?? initialProfileIdRef.current;
      const selected = profiles.some((profile) => profile.profileId === desired)
        ? desired ?? null
        : profiles[0]?.profileId ?? null;
      // 收敛结果同步给调用方：删除 / 新建之后"用户看到的那个"才是记忆该指向的配置。
      notifySelection(selected);
      setState((current) => ({
        ...current,
        loading: false,
        loadError: null,
        profiles,
        selectedProfileId: selected,
        // 切换 profile 时旧目录不属于新 profile，必须丢弃而不是继续显示。
        catalog: selected === current.selectedProfileId ? current.catalog : null,
        catalogError: null,
      }));
    } catch (error) {
      if (id !== profileLoadId.current) return;
      setState((current) => ({
        ...current,
        loading: false,
        loadError: toErrorInfo(error),
        // 保留最后一次成功读取的快照供恢复后收敛；UI 必须以 loadError 为准，
        // 不能把这次读取失败误呈现为「尚未配置任何 Provider」。
        catalogLoading: false,
      }));
    }
  }, [notifySelection]);

  useEffect(() => {
    void load();
  }, [load]);

  const refreshModels = useCallback(async (): Promise<void> => {
    if (state.loading || state.loadError !== null) {
      catalogRequestId.current += 1;
      return;
    }
    const profileId = selectedRef.current;
    if (!profileId) {
      catalogRequestId.current += 1;
      setState((current) => ({
        ...current,
        catalog: null,
        catalogLoading: false,
        catalogError: null,
      }));
      return;
    }
    const id = ++catalogRequestId.current;
    setState((current) => ({
      ...current,
      catalogLoading: true,
      catalogError: null,
      catalog: null,
    }));
    try {
      const catalog = await listAiProviderModels(profileId);
      if (id !== catalogRequestId.current || profileId !== selectedRef.current) return;
      setState((current) => ({
        ...current,
        catalogLoading: false,
        catalogError: null,
        catalog,
      }));
    } catch (error) {
      if (id !== catalogRequestId.current || profileId !== selectedRef.current) return;
      // Provider 失败不伪造任何模型：目录保持空，页面显示可重试错误。
      setState((current) => ({
        ...current,
        catalogLoading: false,
        catalog: null,
        catalogError: toErrorInfo(error),
      }));
    }
  }, [state.loadError, state.loading]);

  // 选中的 profile 变了就重新拉取它自己的目录（或被禁用/无凭据的空态）。
  //
  // 依赖的是"目录身份"而不是 profile id：模型目录取决于这份配置的**输入**——
  // 端点、启用状态、凭据是否已配置、以及配置本身的 revision。只盯 id 会漏掉最常走的
  // 那条路：用户刚给同一个 profile 写完 API Key，id 没变，但目录已经从
  // `no_credential` 变成了可用的模型列表。少了这一步，界面会把"刚配置好密钥"显示成
  // 「无可用模型」并且模型选择器一直禁用——那不是空态，那是过期状态。
  const catalogIdentity = useMemo(() => {
    const profile = state.profiles.find((item) => item.profileId === state.selectedProfileId);
    if (profile === undefined) return null;
    return [
      profile.profileId,
      profile.revision,
      profile.endpoint,
      String(profile.enabled),
      String(profile.credentialConfigured),
    ].join("|");
  }, [state.profiles, state.selectedProfileId]);

  useEffect(() => {
    if (state.loading || catalogIdentity === null) return;
    void refreshModels();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [catalogIdentity, state.loading]);

  const selectProfile = useCallback((profileId: string | null) => {
    catalogRequestId.current += 1;
    // 显式选择（含清空）立刻上报：即使这一次读取随后失败，用户的意图也已经记住了。
    notifySelection(profileId);
    setState((current) => ({
      ...current,
      selectedProfileId: profileId,
      catalog: null,
      catalogLoading: false,
      catalogError: null,
    }));
  }, [notifySelection]);

  const saveProfile = useCallback(async (draft: AiProviderProfileDraft): Promise<boolean> => {
    const existing = state.profiles.find(
      (profile) => profile.profileId === draft.profileId,
    );
    if (state.loading || state.loadError !== null) return false;
    if (draft.createOnly && existing) {
      setState((current) => ({
        ...current,
        saveError: {
          code: "AI_PROVIDER_PROFILE_ALREADY_EXISTS",
          message: "该配置 ID 已存在，请更换 ID，以免覆盖现有服务。",
          retryable: false,
        },
        saveErrorAction: "profile",
      }));
      return false;
    }
    // 保存边界：明文端点在这里就被拒绝，省掉一次注定失败的往返。
    // 后端仍会独立地再判一次——前端这条只是快速失败，不是权威结论。
    if (!isHttpsAiEndpoint(draft.endpoint)) {
      setState((current) => ({
        ...current,
        saveError: {
          code: INSECURE_ENDPOINT_CODE,
          message: INSECURE_ENDPOINT_MESSAGE,
          retryable: false,
        },
        saveErrorAction: "profile",
      }));
      return false;
    }
    lastSaveDraft.current = draft;
    setState((current) => ({ ...current, saving: true, saveError: null }));
    try {
      const saved = await upsertAiProviderProfile({
        profileId: draft.profileId,
        displayName: draft.displayName,
        kind: "openai_compatible",
        endpoint: draft.endpoint,
        enabled: draft.enabled,
        selectedModelId: draft.selectedModelId,
        // 服务端也执行 create-only CAS：即使 UI 列表过期或并发新建，也不能覆盖现有项。
        expectedRevision: draft.createOnly ? null : existing?.revision ?? null,
      });
      setState((current) => ({
        ...current,
        saving: false,
        saveError: null,
        saveErrorAction: null,
      }));
      await load(saved.profileId);
      return true;
    } catch (error) {
      setState((current) => ({
        ...current,
        saving: false,
        saveError: toErrorInfo(error),
        saveErrorAction: "profile",
      }));
      return false;
    }
  }, [load, state.loadError, state.loading, state.profiles]);

  const submitApiKey = useCallback(async (secret: string): Promise<boolean> => {
    const profileId = selectedRef.current;
    if (!profileId) return false;
    // 与"清除"互斥：两条写操作不能并发决定同一个凭据的最终状态。
    if (credentialMutationInFlight.current) return false;
    credentialMutationInFlight.current = true;
    setState((current) => ({ ...current, saving: true, saveError: null }));
    try {
      await setAiProviderApiKey(profileId, secret);
      // secret 到此为止：不写入本地状态、不缓存、不回显。
      setState((current) => ({ ...current, saving: false }));
      // 单向写入后回读 profile 列表，让 credentialConfigured 仍以系统凭据管理器为准。
      await load(profileId);
      return true;
    } catch (error) {
      setState((current) => ({
        ...current,
        saving: false,
        saveError: toErrorInfo(error),
        saveErrorAction: "credential",
      }));
      // IPC 失败可能发生在后端已提交之后；刷新权威状态，不推测是否写入成功。
      await load(profileId);
      return false;
    } finally {
      credentialMutationInFlight.current = false;
    }
  }, [load]);

  const clearApiKey = useCallback(async (): Promise<boolean> => {
    const profileId = selectedRef.current;
    if (!profileId) return false;
    if (credentialMutationInFlight.current) return false;
    credentialMutationInFlight.current = true;
    setState((current) => ({ ...current, saving: true, saveError: null }));
    try {
      await clearAiProviderApiKey(profileId);
      setState((current) => ({ ...current, saving: false }));
      // 清除与写入使用同一权威回读，避免 lost response 留下错误的「已配置」状态。
      await load(profileId);
      return true;
    } catch (error) {
      setState((current) => ({
        ...current,
        saving: false,
        saveError: toErrorInfo(error),
        saveErrorAction: "credential",
      }));
      await load(profileId);
      return false;
    } finally {
      credentialMutationInFlight.current = false;
    }
  }, [load]);

  /**
   * 按**指定目标**删除。
   *
   * 删除按钮与「重试」共用这一条：两者的区别只在"目标从哪来"——按钮取当前选中的那一份，
   * 重试取失败时记下的那一份。放在同一个函数里是为了让"删的到底是哪一份"只有一处实现，
   * 不会出现"按钮改了、重试没改"这种分叉。
   */
  const deleteProfileTarget = useCallback(async (target: {
    profileId: string;
    revision: string;
  }): Promise<boolean> => {
    setState((current) => ({ ...current, saving: true, saveError: null }));
    try {
      await deleteAiProviderProfile(target.profileId, target.revision);
      lastDeleteTarget.current = null;
      setState((current) => ({
        ...current,
        saving: false,
        saveError: null,
        saveErrorAction: null,
      }));
      await load(null);
      return true;
    } catch (error) {
      setState((current) => ({
        ...current,
        saving: false,
        saveError: toErrorInfo(error),
        saveErrorAction: "delete",
      }));
      return false;
    }
  }, [load]);

  const deleteProfile = useCallback(async (): Promise<boolean> => {
    if (state.loading || state.loadError !== null) return false;
    const profile = state.profiles.find(
      (item) => item.profileId === selectedRef.current,
    );
    if (!profile) return false;
    // 目标在这里定下来并记牢：失败之后的重试要重放它，而不是重放"那一刻选中的东西"。
    const target = { profileId: profile.profileId, revision: profile.revision };
    lastDeleteTarget.current = target;
    return await deleteProfileTarget(target);
  }, [deleteProfileTarget, state.loadError, state.loading, state.profiles]);

  /**
   * 重试失败的那一次操作。
   *
   * 只有"重放同一件事"才算重试：
   * - `profile`：重放最近一次提交的**同一份**草稿；
   * - `delete`：重放失败时记下的**那个** profile 与它的 revision——不是"此刻选中的那个"。
   *   用户完全可能在删除失败之后切到别的 profile，此时按当前选中项重试就是删错人；
   * - `credential`：**不重试**。密钥原文按设计不留存在任何组件状态里，拿不到输入就
   *   无法重放；用一个"重试"按钮去做别的事（例如重新拉模型）只会掩盖真正的失败。
   */
  const retrySaveError = useCallback(async (): Promise<boolean> => {
    const action = state.saveErrorAction;
    if (action === "profile") {
      const draft = lastSaveDraft.current;
      if (!draft) return false;
      return await saveProfile(draft);
    }
    if (action === "delete") {
      const target = lastDeleteTarget.current;
      if (!target) return false;
      return await deleteProfileTarget(target);
    }
    return false;
  }, [deleteProfileTarget, saveProfile, state.saveErrorAction]);

  const dismissSaveError = useCallback(() => {
    // 提示被关掉之后就不再保留可重放的删除目标：留着它只会让一次"已经作废的失败"
    // 有机会在很久之后被别的东西触发。
    lastDeleteTarget.current = null;
    setState((current) => ({ ...current, saveError: null, saveErrorAction: null }));
  }, []);

  const selectedProfile = useMemo(
    () => state.profiles.find((profile) => profile.profileId === state.selectedProfileId) ?? null,
    [state.profiles, state.selectedProfileId],
  );

  const modelOptions = useMemo(() => toModelOptions(state.catalog), [state.catalog]);

  const hasAvailableModel = modelOptions.length > 0;

  const unavailableModelHint = state.catalogError
    ? state.catalogError.message
    : catalogStateHint(state.catalog?.state ?? null);

  return {
    state,
    selectedProfile,
    modelOptions,
    hasAvailableModel,
    unavailableModelLabel: NO_AVAILABLE_MODEL,
    unavailableModelHint,
    credentialConfigured: selectedProfile?.credentialConfigured ?? false,
    // 历史遗留的明文行：仍然可列出、可编辑，但用它发请求会被后端拒绝。
    // 界面据此给出"必须改成 https"的可执行结论，而不是让用户去猜为什么拉不到模型。
    selectedEndpointInsecure:
      selectedProfile !== null && !isHttpsAiEndpoint(selectedProfile.endpoint),
    // 只有"重试真的会重做那件事"时才提供重试——凭据写入的密钥原文按设计不留存。
    canRetrySaveError:
      state.saveError !== null
      && state.saveError.retryable
      && (state.saveErrorAction === "profile" || state.saveErrorAction === "delete"),
    selectProfile,
    reload: load,
    refreshModels,
    saveProfile,
    retrySaveError,
    submitApiKey,
    clearApiKey,
    deleteProfile,
    dismissSaveError,
  };
}
