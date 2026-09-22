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
  NO_AVAILABLE_MODEL,
  catalogStateHint,
  clearAiProviderApiKey,
  deleteAiProviderProfile,
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
}

export interface AiProviderProfileDraft {
  profileId: string;
  displayName: string;
  endpoint: string;
  enabled: boolean;
  selectedModelId: string | null;
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
  selectProfile: (profileId: string | null) => void;
  reload: () => Promise<void>;
  refreshModels: () => Promise<void>;
  saveProfile: (draft: AiProviderProfileDraft) => Promise<boolean>;
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
};

function toErrorInfo(error: unknown): AiProviderErrorInfo {
  const haven = toHavenError(error);
  return { code: haven.code, message: haven.message, retryable: haven.retryable };
}

export function useAiProviderSettings(): AiProviderSettingsController {
  const [state, setState] = useState<AiProviderSettingsState>(EMPTY_STATE);
  const requestId = useRef(0);
  const selectedRef = useRef<string | null>(null);
  selectedRef.current = state.selectedProfileId;

  /**
   * 读取 Profile 列表，并在必要时顺带读取模型目录。
   * `preferredProfileId` 让删除/新建后仍能停留在用户刚才看的那个 profile 上。
   */
  const load = useCallback(async (preferredProfileId?: string | null): Promise<void> => {
    const id = ++requestId.current;
    setState((current) => ({ ...current, loading: true, loadError: null }));
    try {
      const profiles = await listAiProviderProfiles();
      if (id !== requestId.current) return;
      const desired = preferredProfileId ?? selectedRef.current;
      const selected = profiles.some((profile) => profile.profileId === desired)
        ? desired ?? null
        : profiles[0]?.profileId ?? null;
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
      if (id !== requestId.current) return;
      setState((current) => ({
        ...current,
        loading: false,
        loadError: toErrorInfo(error),
        profiles: [],
        selectedProfileId: null,
        catalog: null,
      }));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const refreshModels = useCallback(async (): Promise<void> => {
    const profileId = selectedRef.current;
    if (!profileId) {
      setState((current) => ({ ...current, catalog: null, catalogError: null }));
      return;
    }
    const id = ++requestId.current;
    setState((current) => ({
      ...current,
      catalogLoading: true,
      catalogError: null,
      catalog: null,
    }));
    try {
      const catalog = await listAiProviderModels(profileId);
      if (id !== requestId.current) return;
      setState((current) => ({
        ...current,
        catalogLoading: false,
        catalogError: null,
        catalog,
      }));
    } catch (error) {
      if (id !== requestId.current) return;
      // Provider 失败不伪造任何模型：目录保持空，页面显示可重试错误。
      setState((current) => ({
        ...current,
        catalogLoading: false,
        catalog: null,
        catalogError: toErrorInfo(error),
      }));
    }
  }, []);

  // 选中的 profile 变了就重新拉取它自己的目录（或被禁用/无凭据的空态）。
  useEffect(() => {
    if (state.loading || state.selectedProfileId === null) return;
    void refreshModels();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state.selectedProfileId, state.loading]);

  const selectProfile = useCallback((profileId: string | null) => {
    setState((current) => ({
      ...current,
      selectedProfileId: profileId,
      catalog: null,
      catalogError: null,
    }));
  }, []);

  const saveProfile = useCallback(async (draft: AiProviderProfileDraft): Promise<boolean> => {
    const existing = state.profiles.find(
      (profile) => profile.profileId === draft.profileId,
    );
    setState((current) => ({ ...current, saving: true, saveError: null }));
    try {
      const saved = await upsertAiProviderProfile({
        profileId: draft.profileId,
        displayName: draft.displayName,
        kind: "openai_compatible",
        endpoint: draft.endpoint,
        enabled: draft.enabled,
        selectedModelId: draft.selectedModelId,
        expectedRevision: existing?.revision ?? null,
      });
      setState((current) => ({ ...current, saving: false, saveError: null }));
      await load(saved.profileId);
      return true;
    } catch (error) {
      setState((current) => ({ ...current, saving: false, saveError: toErrorInfo(error) }));
      return false;
    }
  }, [load, state.profiles]);

  const submitApiKey = useCallback(async (secret: string): Promise<boolean> => {
    const profileId = selectedRef.current;
    if (!profileId) return false;
    setState((current) => ({ ...current, saving: true, saveError: null }));
    try {
      await setAiProviderApiKey(profileId, secret);
      // secret 到此为止：不写入本地状态、不缓存、不回显。
      setState((current) => ({
        ...current,
        saving: false,
        profiles: current.profiles.map((profile) => (
          profile.profileId === profileId
            ? { ...profile, credentialConfigured: true }
            : profile
        )),
      }));
      await refreshModels();
      return true;
    } catch (error) {
      setState((current) => ({ ...current, saving: false, saveError: toErrorInfo(error) }));
      return false;
    }
  }, [refreshModels]);

  const clearApiKey = useCallback(async (): Promise<boolean> => {
    const profileId = selectedRef.current;
    if (!profileId) return false;
    setState((current) => ({ ...current, saving: true, saveError: null }));
    try {
      await clearAiProviderApiKey(profileId);
      setState((current) => ({
        ...current,
        saving: false,
        profiles: current.profiles.map((profile) => (
          profile.profileId === profileId
            ? { ...profile, credentialConfigured: false }
            : profile
        )),
        catalog: null,
        catalogError: null,
      }));
      return true;
    } catch (error) {
      setState((current) => ({ ...current, saving: false, saveError: toErrorInfo(error) }));
      return false;
    }
  }, []);

  const deleteProfile = useCallback(async (): Promise<boolean> => {
    const profile = state.profiles.find(
      (item) => item.profileId === selectedRef.current,
    );
    if (!profile) return false;
    setState((current) => ({ ...current, saving: true, saveError: null }));
    try {
      await deleteAiProviderProfile(profile.profileId, profile.revision);
      setState((current) => ({ ...current, saving: false }));
      await load(null);
      return true;
    } catch (error) {
      setState((current) => ({ ...current, saving: false, saveError: toErrorInfo(error) }));
      return false;
    }
  }, [load, state.profiles]);

  const dismissSaveError = useCallback(() => {
    setState((current) => ({ ...current, saveError: null }));
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
    selectProfile,
    reload: load,
    refreshModels,
    saveProfile,
    submitApiKey,
    clearApiKey,
    deleteProfile,
    dismissSaveError,
  };
}
