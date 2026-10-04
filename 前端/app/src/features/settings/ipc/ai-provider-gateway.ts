// AI Provider Gateway（A2 基础切片）：设置页 AI 分组的唯一数据通道。
// 契约：docs/architecture/AI_SYSTEM.md §3、§4、§8。
//
// 职责边界：
// - 只调用 Typed HavenClient（Tauri 环境 invoke / Mock 环境 fixture），组件不得直接 invoke。
// - 运行时形状守卫：所有响应必须精确匹配生成的 wire 类型；未知键、symbol 键、
//   非枚举键一律拒绝（Reflect.ownKeys 而非 Object.keys）。
// - **secret 不经过本模块的读路径**。`setAiProviderApiKey` 只单向提交，返回值里
//   没有任何密钥材料；状态只暴露 `configured` 布尔事实。
// - 模型目录的 state 是诚实状态：`no_credential` / `disabled` / `empty` 都表示
//   「无可用模型」，页面据此显示空态，绝不回退到示例模型名。

import { getHavenClient } from "@/lib/ipc/runtime";
import { toHavenError } from "@/lib/ipc/errors";
import type {
  AiModelCapabilityDto,
  AiProviderKindDto,
  AiProviderModelDto,
  AiProviderModelsCatalogDto,
  AiProviderModelsCatalogStateDto,
  AiProviderProfileDto,
  AiProviderProfileListResultDto,
  AiProviderProfileUpsertRequest,
  CredentialStatusDto,
} from "@/lib/ipc/generated/wire";

/** 「无可用模型」的显示文案：唯一的空态标签，禁止用示例模型名占位。 */
export const NO_AVAILABLE_MODEL = "无可用模型";

/**
 * 有可用模型、但用户还没选过时的显示文案。
 *
 * 必须与 `NO_AVAILABLE_MODEL` 分开：把"还没选"显示成"无可用模型"会在模型目录明明
 * 取到了的情况下谎报没有模型，而这两个状态对用户意味着完全不同的下一步操作。
 */
export const NO_MODEL_SELECTED = "未选择";

const PROVIDER_KINDS = new Set<string>(["openai_compatible"]);
const MODEL_CAPABILITIES = new Set<string>(["supported", "unsupported", "unknown"]);
const CATALOG_STATES = new Set<string>([
  "ready",
  "disabled",
  "no_credential",
  "empty",
]);

const PROFILE_FIELDS = [
  "schemaVersion",
  "profileId",
  "displayName",
  "kind",
  "endpoint",
  "enabled",
  "selectedModelId",
  "credentialConfigured",
  "revision",
  "createdAt",
  "updatedAt",
] as const;

const MODEL_FIELDS = [
  "modelId",
  "displayName",
  "created",
  "ownedBy",
  "chat",
  "vision",
  "embedding",
] as const;

const CATALOG_FIELDS = ["schemaVersion", "profileId", "state", "models"] as const;

/** 任何 secret 形状的键都不允许出现在 AI Provider 响应里。 */
const FORBIDDEN_FIELDS = [
  "secret",
  "apiKey",
  "api_key",
  "key",
  "token",
  "password",
  "authorization",
  "credentialRef",
  "credential_ref",
  "target",
  "bearer",
];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactFields(value: Record<string, unknown>, allowed: readonly string[]): boolean {
  const permitted = new Set<string>(allowed);
  for (const field of Reflect.ownKeys(value)) {
    if (typeof field !== "string") return false;
    if (!permitted.has(field)) return false;
  }
  for (const field of allowed) {
    if (!Object.prototype.hasOwnProperty.call(value, field)) return false;
  }
  return true;
}

function hasForbiddenField(value: Record<string, unknown>): boolean {
  return Reflect.ownKeys(value).some(
    (field) => typeof field === "string" && FORBIDDEN_FIELDS.includes(field),
  );
}

/** 可选文本字段：null 或非空字符串。**不**做 trim 之外的任何解释。 */
function isOptionalText(value: unknown): value is string | null {
  return value === null || (typeof value === "string" && value.length > 0);
}

export function guardAiProviderProfile(value: unknown): value is AiProviderProfileDto {
  if (!isRecord(value) || hasForbiddenField(value)) return false;
  if (!hasExactFields(value, PROFILE_FIELDS)) return false;
  return value.schemaVersion === 1
    && typeof value.profileId === "string"
    && value.profileId.length > 0
    && typeof value.displayName === "string"
    && value.displayName.length > 0
    && typeof value.kind === "string"
    && PROVIDER_KINDS.has(value.kind)
    && typeof value.endpoint === "string"
    && (value.endpoint.startsWith("https://") || value.endpoint.startsWith("http://"))
    && typeof value.enabled === "boolean"
    && isOptionalText(value.selectedModelId)
    && typeof value.credentialConfigured === "boolean"
    && typeof value.revision === "string"
    && value.revision.length > 0
    && typeof value.createdAt === "string"
    && typeof value.updatedAt === "string";
}

export function guardAiProviderProfileList(
  value: unknown,
): value is AiProviderProfileListResultDto {
  if (!isRecord(value)) return false;
  if (!hasExactFields(value, ["schemaVersion", "profiles"])) return false;
  return value.schemaVersion === 1
    && Array.isArray(value.profiles)
    && value.profiles.every(guardAiProviderProfile);
}

export function guardAiProviderModel(value: unknown): value is AiProviderModelDto {
  if (!isRecord(value) || hasForbiddenField(value)) return false;
  if (!hasExactFields(value, MODEL_FIELDS)) return false;
  return typeof value.modelId === "string"
    && value.modelId.length > 0
    && isOptionalText(value.displayName)
    && (value.created === null || typeof value.created === "number")
    && isOptionalText(value.ownedBy)
    && typeof value.chat === "string"
    && MODEL_CAPABILITIES.has(value.chat)
    && typeof value.vision === "string"
    && MODEL_CAPABILITIES.has(value.vision)
    && typeof value.embedding === "string"
    && MODEL_CAPABILITIES.has(value.embedding);
}

export function guardAiProviderModelsCatalog(
  value: unknown,
): value is AiProviderModelsCatalogDto {
  if (!isRecord(value) || hasForbiddenField(value)) return false;
  if (!hasExactFields(value, CATALOG_FIELDS)) return false;
  if (value.schemaVersion !== 1) return false;
  if (typeof value.profileId !== "string" || value.profileId.length === 0) return false;
  if (typeof value.state !== "string" || !CATALOG_STATES.has(value.state)) return false;
  if (!Array.isArray(value.models) || !value.models.every(guardAiProviderModel)) return false;
  // state 与 models 必须互相印证：只有 ready 才允许非空目录。
  if (value.state !== "ready" && value.models.length > 0) return false;
  if (value.state === "ready" && value.models.length === 0) return false;
  return true;
}

export function guardCredentialStatus(value: unknown): value is CredentialStatusDto {
  if (!isRecord(value) || hasForbiddenField(value)) return false;
  if (!hasExactFields(value, ["configured", "updatedAt"])) return false;
  return typeof value.configured === "boolean"
    && (value.updatedAt === null || typeof value.updatedAt === "string");
}

const INVALID_RESPONSE = "AI Provider 接口返回了非法数据";

function invalidResponse(): never {
  throw toHavenError({ code: "INTERNAL_ERROR", userMessage: INVALID_RESPONSE, retryable: false });
}

/** 列出全部 Provider Profile（无 profile 时为空数组，不是错误）。 */
export async function listAiProviderProfiles(): Promise<AiProviderProfileDto[]> {
  try {
    const value: unknown = await getHavenClient().aiProviderProfileList();
    if (!guardAiProviderProfileList(value)) invalidResponse();
    return value.profiles;
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 读取单个 Provider Profile。 */
export async function getAiProviderProfile(profileId: string): Promise<AiProviderProfileDto> {
  try {
    const value: unknown = await getHavenClient().aiProviderProfileGet({ profileId });
    if (!guardAiProviderProfile(value)) invalidResponse();
    return value;
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 写入 Provider Profile（非敏感字段）；CAS 冲突由后端返回。 */
export async function upsertAiProviderProfile(
  request: AiProviderProfileUpsertRequest,
): Promise<AiProviderProfileDto> {
  try {
    const value: unknown = await getHavenClient().aiProviderProfileUpsert(request);
    if (!guardAiProviderProfile(value)) invalidResponse();
    return value;
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 删除 Provider Profile（后端先清理 `haven:ai:<profileId>` 凭据再删行）。 */
export async function deleteAiProviderProfile(
  profileId: string,
  expectedRevision: string | null,
): Promise<{ profileId: string; credentialDeleted: boolean }> {
  try {
    const value: unknown = await getHavenClient().aiProviderProfileDelete({
      profileId,
      expectedRevision,
    });
    if (
      !isRecord(value)
      || !hasExactFields(value, ["profileId", "credentialDeleted"])
      || value.profileId !== profileId
      || typeof value.credentialDeleted !== "boolean"
    ) {
      invalidResponse();
    }
    return { profileId: value.profileId as string, credentialDeleted: value.credentialDeleted };
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 读取模型目录（诚实空态：no_credential / disabled / empty 都表示无可用模型）。 */
export async function listAiProviderModels(
  profileId: string,
): Promise<AiProviderModelsCatalogDto> {
  try {
    const value: unknown = await getHavenClient().aiProviderModelsList({ profileId });
    if (!guardAiProviderModelsCatalog(value)) invalidResponse();
    return value;
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 该 profile 的密钥是否已配置。只读事实，不返回密钥本身。 */
export async function aiProviderCredentialConfigured(profileId: string): Promise<boolean> {
  try {
    const value: unknown = await getHavenClient().credentialStatus({
      provider: "ai",
      profileId,
    });
    if (!guardCredentialStatus(value)) invalidResponse();
    return value.configured;
  } catch (error) {
    throw toHavenError(error);
  }
}

/**
 * 单向写入 API Key（`haven:ai:<profileId>`）。
 *
 * 密钥只作为参数传一次；本函数不返回、不缓存、不记录它，也不接受空值
 * （空值会让「已配置」变成一个无法验证的假状态）。
 */
export async function setAiProviderApiKey(profileId: string, secret: string): Promise<void> {
  if (secret.length === 0) {
    throw toHavenError({
      code: "INVALID_ARGUMENT",
      userMessage: "API Key 不能为空",
      retryable: false,
    });
  }
  try {
    await getHavenClient().credentialSet({ provider: "ai", profileId, secret });
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 清除 API Key（幂等）。 */
export async function clearAiProviderApiKey(profileId: string): Promise<void> {
  try {
    await getHavenClient().credentialDelete({ provider: "ai", profileId });
  } catch (error) {
    throw toHavenError(error);
  }
}

/** 目录状态 → 是否有可用模型。只有 `ready` 才允许渲染模型下拉。 */
export function catalogHasModels(
  catalog: AiProviderModelsCatalogDto | null,
): catalog is AiProviderModelsCatalogDto & { state: "ready" } {
  return catalog !== null && catalog.state === "ready" && catalog.models.length > 0;
}

/** 模型能力 → 中文标签。`unknown` 必须与 `unsupported` 区分显示。 */
export function capabilityLabel(value: AiModelCapabilityDto): string {
  switch (value) {
    case "supported":
      return "已声明支持";
    case "unsupported":
      return "已声明不支持";
    default:
      return "未声明";
  }
}

/** 目录状态 → 空态说明文案（页面在无可用模型时显示）。 */
export function catalogStateHint(state: AiProviderModelsCatalogStateDto | null): string {
  switch (state) {
    case "disabled":
      return "该 Provider 已停用，未请求模型列表。";
    case "no_credential":
      return "尚未配置 API Key，未请求模型列表。";
    case "empty":
      return "Provider 返回了空模型列表。";
    case "ready":
      return "";
    default:
      return "尚未读取模型列表。";
  }
}

/** 供 UI 选择器使用的最小模型投影（id + 显示名 + 能力标签）。 */
export type AiProviderModelOption = {
  modelId: string;
  label: string;
  visionLabel: string;
  embeddingLabel: string;
};

export function toModelOptions(
  catalog: AiProviderModelsCatalogDto | null,
): AiProviderModelOption[] {
  if (!catalogHasModels(catalog)) return [];
  return catalog.models.map((model) => ({
    modelId: model.modelId,
    label: model.displayName ?? model.modelId,
    visionLabel: capabilityLabel(model.vision),
    embeddingLabel: capabilityLabel(model.embedding),
  }));
}

/** 供测试与调用方复用的种类常量（与生成物同值）。 */
export const AI_PROVIDER_KIND_OPENAI_COMPATIBLE: AiProviderKindDto = "openai_compatible";
