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
//   但写入失败后必须回读权威列表；CAS 冲突不能原样重放，只提示用户看过最新内容后
//   再次显式提交 / 确认，绝不用回读到的新 revision 自动覆盖或删除未读数据。

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { toHavenError } from "@/lib/ipc/errors";
import type {
  AiProviderModelsCatalogDto,
  AiProviderProfileDto,
} from "@/lib/ipc/generated/wire";

import {
  INSECURE_ENDPOINT_CODE,
  INSECURE_ENDPOINT_MESSAGE,
  LOCALHOST_ALIAS_ENDPOINT_CODE,
  LOCALHOST_ALIAS_ENDPOINT_MESSAGE,
  NO_AVAILABLE_MODEL,
  catalogStateHint,
  clearAiProviderApiKey,
  deleteAiProviderProfile,
  isAiEndpointTransportAllowed,
  isLocalhostAliasEndpoint,
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
  /**
   * 最近一次失败是 CAS 冲突（权威列表已推进到用户没看过的一版）。
   *
   * 这种失败**不能重放同一个操作**：拿旧 revision 重试只会再次冲突；把 revision
   * 自动升级成回读后的新值再重试，则会在用户没看过的新数据上覆盖/删除。页面据此
   * 隐藏「重试」，让用户看过回读后的列表再显式提交/确认一次。
   */
  saveErrorConflict: boolean;
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
  /** 活跃时自动读取配置；默认 true。工作台与设置面板共享读取结果。 */
  enabled?: boolean;
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
  deleteProfile: (confirmedTarget?: { profileId: string; revision: string }) => Promise<boolean>;
  /**
   * 标记 / 解除"删除确认在途"。
   *
   * 确认框由页面（`useNotice`）持有，Hook 看不到它，因此这枚状态必须由调用方设置：
   * 置位后所有 profile 与凭据写入（保存、启用、模型选择、API Key 写入 / 清除）都会
   * 被拒，避免与待确认的那次删除交错留下孤儿 secret。取消纯草稿不算写入，不受影响。
   */
  setDeleteConfirmPending: (pending: boolean) => void;
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
  saveErrorConflict: false,
};

function toErrorInfo(error: unknown): AiProviderErrorInfo {
  const haven = toHavenError(error);
  return { code: haven.code, message: haven.message, retryable: haven.retryable };
}

/**
 * 后端 CAS 冲突的稳定错误码（`haven-application/src/services/ai_provider.rs` 的
 * `revision_conflict()`）。
 *
 * 冲突是一类**不能原样重放**的失败：用户手里的 revision 已经过期，本机存在一版他
 * 没看过的数据。它必须与"网络失败 / 响应丢失"区分开——后者的写入结果不确定，但用
 * 同一个 revision 重放是安全的（真写成功时会落进冲突，不会覆盖）。
 */
const PROFILE_REVISION_CONFLICT_CODE = "AI_PROVIDER_PROFILE_REVISION_CONFLICT";

/** 冲突失败的恢复文案：说明权威列表已经回读，下一步是"看过后再显式提交一次"。 */
function withConflictRecovery(
  info: AiProviderErrorInfo,
  action: "profile" | "delete",
): AiProviderErrorInfo {
  const next = action === "delete" ? "检查后重新确认删除" : "检查后再次保存";
  return {
    ...info,
    message: `${info.message}。请核对重新读取的配置后${next}；若读取失败，请先重新加载。`,
  };
}

export function useAiProviderSettings(options: AiProviderSettingsOptions = {}): AiProviderSettingsController {
  // 不活跃的控制器没有"正在读取"这回事：`loading: true` 会让工作台显示一个永远不会
  // 结束的读取中状态（因为它按设计就没打算读）。
  const [state, setState] = useState<AiProviderSettingsState>(() => (
    (options.enabled ?? true) ? EMPTY_STATE : { ...EMPTY_STATE, loading: false }
  ));
  // Profile 列表与模型目录是两个独立的异步资源；刷新模型不能让列表请求失效，反之亦然。
  const profileLoadId = useRef(0);
  const profilesInitialized = useRef(false);
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
  /**
   * 非模态确认框在途。
   *
   * 删除确认是**异步且可能长时间挂着**的：确认框打开期间，行上的其余写入控件（保存配置、
   * 写 / 清 API Key、启用开关、模型选择）如果还能提交，就会和"待确认的那一次删除"交错。
   * 具体后果不是"顺序不好看"，而是**孤儿凭据**——用户的 credentialSet 先落盘、删除后
   * 才执行，后端已经按行清理过 `haven:ai:<profileId>`，系统凭据管理器里却留下一份谁都
   * 引用不到的密钥。
   *
   * 因此确认在途期间，删除与凭据写入共用这一道**同步**闸门：先检测、再 true、
   * `finally` 释放，钩子自身不依赖异步的 `state.saving`（那个状态落后一个渲染）。
   */
  const deleteConfirmInFlight = useRef(false);
  /**
   * 最近一次提交的 profile 草稿**及其 CAS 依据**。
   *
   * 重试"保存"必须重放同一份输入，也必须重放**提交时**的 `expectedRevision`：失败后
   * 本 Hook 会回读权威列表，若重试时再从列表里现查 revision，就等于自动升级到用户
   * 没看过的那一版，并在它上面覆盖写入。
   */
  const lastSaveRequest = useRef<{
    draft: AiProviderProfileDraft;
    expectedRevision: string | null;
  } | null>(null);

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

  /** 跟随 AI 分区的活跃状态，避免其他分区触发模型读取。 */
  const enabledRef = useRef(options.enabled ?? true);
  enabledRef.current = options.enabled ?? true;

  /**
   * 读取 Profile 列表，并在必要时顺带读取模型目录。
   * `preferredProfileId` 让删除/新建后仍能停留在用户刚才看的那个 profile 上。
   */
  const load = useCallback(async (preferredProfileId?: string | null): Promise<void> => {
    profilesInitialized.current = false;
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
      profilesInitialized.current = true;
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
      profilesInitialized.current = true;
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

  /** 进入 AI 分区时读取配置；退出时失效尚未完成的请求。 */
  const enabled = options.enabled ?? true;
  useEffect(() => {
    if (enabled) void load();
    else setState((current) => ({ ...current, loading: false, catalogLoading: false }));
    return () => {
      profileLoadId.current += 1;
      catalogRequestId.current += 1;
      profilesInitialized.current = false;
    };
  }, [enabled, load]);

  const refreshModels = useCallback(async (): Promise<void> => {
    if (!profilesInitialized.current || state.loading || state.loadError !== null) {
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
    // 退出 AI 分区后不自动请求模型目录。
    if (!enabledRef.current) {
      catalogRequestId.current += 1;
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
    // 显式 reload 可在非活跃时读取配置；自动模型读取只跟随活跃分区。
    if (!enabled) return;
    void refreshModels();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [catalogIdentity, state.loading, enabled]);

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

  /**
   * 用**提交时定下的 CAS 依据**写入一份 profile 草稿。
   *
   * `expectedRevision` 不从 `state.profiles` 现查：失败后本 Hook 会回读权威列表，
   * 「重试」若现查就会拿到用户还没看过的新 revision——那正是"自动升级版本并覆盖
   * 未读改动"。重放必须用提交时的值；真要接受新版本，只能由用户看过列表后重新提交。
   *
   * `checkExisting` 只服务一次新的界面提交（create-only 快速失败）。重试交给服务端的
   * create-only CAS：回读后"已经存在"可能只是上一次写入其实已落盘，不能因此改写错误。
   */
  const runProfileSave = useCallback(async (
    draft: AiProviderProfileDraft,
    expectedRevision: string | null,
    checkExisting: boolean,
  ): Promise<boolean> => {
    // 删除确认在途时整段 profile 写入都停下来：确认框是异步的，期间任何一次
    // 保存 / 启用 / 选模型都会写到"用户正在决定要删掉的那一份"上。
    if (deleteConfirmInFlight.current) return false;
    const existing = state.profiles.find(
      (profile) => profile.profileId === draft.profileId,
    );
    if (state.loading || state.loadError !== null) return false;
    if (checkExisting && draft.createOnly && existing) {
      setState((current) => ({
        ...current,
        saveError: {
          code: "AI_PROVIDER_PROFILE_ALREADY_EXISTS",
          message: "该配置 ID 已存在，请更换 ID，以免覆盖现有服务。",
          retryable: false,
        },
        saveErrorAction: "profile",
        saveErrorConflict: false,
      }));
      return false;
    }
    // 保存边界：明文端点在这里就被拒绝，省掉一次注定失败的往返。
    // 后端仍会独立地再判一次——前端这条只是快速失败，不是权威结论。
    //
    // `localhost` 必须先分类：它是"主机名不被接受"，而不是"明文传输不被接受"。用同一
    // 句"改成 HTTPS"打发它，用户照做之后仍然保存不了（本机服务改成 HTTPS 只会更难连
    // 上），而真正该做的是把主机名写成 127.0.0.1 / [::1]。这条只改提示分类，不放行任何
    // 地址：`localhost` 依然到不了后端保存路径。
    if (isLocalhostAliasEndpoint(draft.endpoint)) {
      setState((current) => ({
        ...current,
        saveError: {
          code: LOCALHOST_ALIAS_ENDPOINT_CODE,
          message: LOCALHOST_ALIAS_ENDPOINT_MESSAGE,
          retryable: false,
        },
        saveErrorAction: "profile",
        saveErrorConflict: false,
      }));
      return false;
    }
    if (!isAiEndpointTransportAllowed(draft.endpoint)) {
      setState((current) => ({
        ...current,
        saveError: {
          code: INSECURE_ENDPOINT_CODE,
          message: INSECURE_ENDPOINT_MESSAGE,
          retryable: false,
        },
        saveErrorAction: "profile",
        saveErrorConflict: false,
      }));
      return false;
    }
    lastSaveRequest.current = { draft, expectedRevision };
    setState((current) => ({
      ...current,
      saving: true,
      saveError: null,
      saveErrorAction: null,
      saveErrorConflict: false,
    }));
    try {
      const saved = await upsertAiProviderProfile({
        profileId: draft.profileId,
        displayName: draft.displayName,
        kind: "openai_compatible",
        endpoint: draft.endpoint,
        enabled: draft.enabled,
        selectedModelId: draft.selectedModelId,
        // 服务端也执行 create-only CAS：即使 UI 列表过期或并发新建，也不能覆盖现有项。
        expectedRevision,
      });
      lastSaveRequest.current = null;
      setState((current) => ({
        ...current,
        saving: false,
        saveError: null,
        saveErrorAction: null,
        saveErrorConflict: false,
      }));
      await load(saved.profileId);
      return true;
    } catch (error) {
      const info = toErrorInfo(error);
      // 冲突意味着本机已有一版用户没看过的数据：保留用户草稿与这次失败提示，
      // 回读权威列表，并**禁用重放**。不确定结果的写入（IPC 失败可能发生在后端
      // 已提交之后）同样必须回读，但可以用原 revision 安全重放。
      const conflict = info.code === PROFILE_REVISION_CONFLICT_CODE;
      setState((current) => ({
        ...current,
        saving: false,
        saveError: conflict ? withConflictRecovery(info, "profile") : info,
        saveErrorAction: "profile",
        saveErrorConflict: conflict,
      }));
      // 回读不清空 `saveError`，也不触碰页面持有的草稿：失败提示与未保存输入都保留。
      await load(draft.profileId);
      return false;
    }
  }, [load, state.loadError, state.loading, state.profiles]);

  const saveProfile = useCallback(async (draft: AiProviderProfileDraft): Promise<boolean> => {
    const existing = state.profiles.find(
      (profile) => profile.profileId === draft.profileId,
    );
    // 界面上的「保存」是一次新的显式提交：使用此刻权威列表里的 revision 是正确的。
    // create-only 的 CAS 依据固定为 null，服务端会拒绝覆盖同名配置。
    const expectedRevision = draft.createOnly ? null : existing?.revision ?? null;
    return await runProfileSave(draft, expectedRevision, true);
  }, [runProfileSave, state.profiles]);

  const submitApiKey = useCallback(async (secret: string): Promise<boolean> => {
    const profileId = selectedRef.current;
    if (!profileId) return false;
    // 与"清除"互斥：两条写操作不能并发决定同一个凭据的最终状态。
    // 与"删除确认"同样互斥：删除会连带清理这份凭据，交错写入只会留下孤儿 secret。
    if (credentialMutationInFlight.current || deleteConfirmInFlight.current) return false;
    credentialMutationInFlight.current = true;
    setState((current) => ({
      ...current,
      saving: true,
      saveError: null,
      saveErrorAction: null,
      saveErrorConflict: false,
    }));
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
        saveErrorConflict: false,
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
    if (credentialMutationInFlight.current || deleteConfirmInFlight.current) return false;
    credentialMutationInFlight.current = true;
    setState((current) => ({
      ...current,
      saving: true,
      saveError: null,
      saveErrorAction: null,
      saveErrorConflict: false,
    }));
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
        saveErrorConflict: false,
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
   *
   * 删除与**凭据写入**（[`submitApiKey`] / [`clearApiKey`]）共用 [`credentialMutationInFlight`]
   * 闸门，理由和"写入与清除互斥"一样但更严重：两者交错会留下**孤儿 secret**——写密钥的
   * 请求在删除之后才落盘，而后端的删除只清理它执行那一刻的行与凭据。闸门是同步
   * ref（先检测、再 true、`finally` 释放），不依赖异步的 `saving` 状态。
   */
  const deleteProfileTarget = useCallback(async (target: {
    profileId: string;
    revision: string;
  }): Promise<boolean> => {
    // 闸门先取：没拿到就没有发起删除，也就没有"可重放的目标"要记。
    if (credentialMutationInFlight.current) return false;
    credentialMutationInFlight.current = true;
    // 删除没有可重放的输入：记下这一次的目标，失败之后"重试"重放的就是它；成功后清空。
    lastDeleteTarget.current = target;
    setState((current) => ({
      ...current,
      saving: true,
      saveError: null,
      saveErrorAction: null,
      saveErrorConflict: false,
    }));
    try {
      await deleteAiProviderProfile(target.profileId, target.revision);
      lastDeleteTarget.current = null;
      setState((current) => ({
        ...current,
        saving: false,
        saveError: null,
        saveErrorAction: null,
        saveErrorConflict: false,
      }));
      await load(null);
      return true;
    } catch (error) {
      const info = toErrorInfo(error);
      // CAS 冲突：目标行已被推进到用户没看过的一版。保留旧 revision 重试只会再次
      // 冲突；自动换成刚读到的 revision 则会在没看过的数据上执行删除。两者都不行，
      // 因此清掉可重放目标、禁用重试，只把回读后的目标留给用户重新确认。
      const conflict = info.code === PROFILE_REVISION_CONFLICT_CODE;
      if (conflict) lastDeleteTarget.current = null;
      setState((current) => ({
        ...current,
        saving: false,
        saveError: conflict ? withConflictRecovery(info, "delete") : info,
        saveErrorAction: "delete",
        saveErrorConflict: conflict,
      }));
      // 冲突或结果不确定（响应丢失时删除可能已经落盘）：回读权威列表，不在旧快照上
      // 继续展示；失败提示保留，不因回读被清掉。
      await load(null);
      return false;
    } finally {
      credentialMutationInFlight.current = false;
    }
  }, [load]);

  /**
   * 删除配置。
   *
   * 传入 `confirmedTarget` 时删的是**确认框打开那一刻**定下的那一份：确认框是异步的，
   * 期间用户可能切到别的配置，按"此刻选中项"删就会删错人。缺省仍按当前选中项。
   *
   * **已经记下的删除目标只应由本人清除**：失败时留着供"重试"重放（CAS 冲突除外：
   * 那种目标已经过期，不能安全重放），成功时由 [`deleteProfileTarget`] 清空。
   * 确认在途时这里直接返回，不会把目标改写成别的值。
   */
  const deleteProfile = useCallback(async (confirmedTarget?: {
    profileId: string;
    revision: string;
  }): Promise<boolean> => {
    if (state.loading || state.loadError !== null) return false;
    // 确认期间只允许已确认的固定目标执行，其余写入口继续保持锁定。
    if (deleteConfirmInFlight.current && !confirmedTarget) return false;
    if (confirmedTarget) {
      // 目标在确认框打开那一刻就定下了，原样交给删除：切到别的 profile 也改不了它。
      return await deleteProfileTarget(confirmedTarget);
    }
    const profile = state.profiles.find(
      (item) => item.profileId === selectedRef.current,
    );
    if (!profile) return false;
    // 目标在这里定下来并记牢：失败之后的重试要重放它，而不是重放"那一刻选中的东西"。
    return await deleteProfileTarget({
      profileId: profile.profileId,
      revision: profile.revision,
    });
  }, [deleteProfileTarget, state.loadError, state.loading, state.profiles]);

  /**
   * 重试失败的那一次操作。
   *
   * 只有"重放同一件事"才算重试：
   * - `profile`：重放最近一次提交的**同一份**草稿与**同一个** expectedRevision；
   * - `delete`：重放失败时记下的**那个** profile 与它的 revision——不是"此刻选中的那个"。
   *   用户完全可能在删除失败之后切到别的 profile，此时按当前选中项重试就是删错人；
   * - **CAS 冲突不重试**：旧 revision 只会再次冲突，升级 revision 则会覆盖/删除用户
   *   没看过的新数据。冲突时已回读权威列表，用户必须看过之后重新提交或重新确认；
   * - `credential`：**不重试**。密钥原文按设计不留存在任何组件状态里，拿不到输入就
   *   无法重放；用一个"重试"按钮去做别的事（例如重新拉模型）只会掩盖真正的失败。
   */
  const retrySaveError = useCallback(async (): Promise<boolean> => {
    if (deleteConfirmInFlight.current) return false;
    if (state.saveErrorConflict) return false;
    const action = state.saveErrorAction;
    if (action === "profile") {
      const request = lastSaveRequest.current;
      if (!request) return false;
      return await runProfileSave(request.draft, request.expectedRevision, false);
    }
    if (action === "delete") {
      const target = lastDeleteTarget.current;
      if (!target) return false;
      return await deleteProfileTarget(target);
    }
    return false;
  }, [deleteProfileTarget, runProfileSave, state.saveErrorAction, state.saveErrorConflict]);

  const dismissSaveError = useCallback(() => {
    // 提示被关掉之后就不再保留可重放的输入：留着它们只会让一次"已经作废的失败"
    // 有机会在很久之后被别的东西触发。
    lastDeleteTarget.current = null;
    lastSaveRequest.current = null;
    setState((current) => ({
      ...current,
      saveError: null,
      saveErrorAction: null,
      saveErrorConflict: false,
    }));
  }, []);

  const setDeleteConfirmPending = useCallback((pending: boolean) => {
    deleteConfirmInFlight.current = pending;
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
    state: { ...state, loading: state.loading || (enabled && !profilesInitialized.current) },
    selectedProfile,
    modelOptions,
    hasAvailableModel,
    unavailableModelLabel: NO_AVAILABLE_MODEL,
    unavailableModelHint,
    credentialConfigured: selectedProfile?.credentialConfigured ?? false,
    // 历史遗留的明文行：仍然可列出、可编辑，但用它发请求会被后端拒绝。
    // 界面据此给出"必须改成 https"的可执行结论，而不是让用户去猜为什么拉不到模型。
    selectedEndpointInsecure:
      selectedProfile !== null && !isAiEndpointTransportAllowed(selectedProfile.endpoint),
    // 只有"重试真的会重做那件事"时才提供重试——凭据写入的密钥原文按设计不留存，
    // CAS 冲突也不能原样重放（旧 revision 会再次冲突，新 revision 会覆盖未读数据）。
    canRetrySaveError:
      state.saveError !== null
      && state.saveError.retryable
      && !state.saveErrorConflict
      && (state.saveErrorAction === "profile" || state.saveErrorAction === "delete"),
    selectProfile,
    reload: load,
    refreshModels,
    saveProfile,
    retrySaveError,
    submitApiKey,
    clearApiKey,
    deleteProfile,
    setDeleteConfirmPending,
    dismissSaveError,
  };
}
