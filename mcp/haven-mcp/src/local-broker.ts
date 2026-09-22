// Haven Broker 的最小本地客户端。
//
// 这里是 MCP server 与 Rust Broker 之间唯一的生产传输适配器：
// - 只使用 outbound `node:net` 连接；不监听、不读文件、不读数据库、不读凭据；
// - 每次 MCP 调用建立一条短连接：connect -> hello/welcome -> 一个请求 -> close；
// - Rust Broker 仍然是权限与 Proposal/CAS 边界，Node 不实现 approve/apply。

import { connect, type Socket } from "node:net";

import type {
  HavenAgentBridge,
  HavenCapabilityReport,
  LibrarySummaryResult,
  MediaCapabilitiesResult,
  OnboardingStateResult,
  PreferenceScope,
  ProposalOutcome,
  ResourcePreferenceProposalRequest,
  ResourcePreferenceSnapshotResult,
  SettingSourcesResult,
  SettingsProposalRequest,
  SettingsSnapshotResult,
} from "./bridge.js";
import { BRIDGE_TIMEOUT_MS, ERROR_CODES, SERVER_VERSION } from "./constants.js";
import {
  HavenMcpError,
  capabilityUnavailable,
  endpointInvalid,
  bridgeProtocolError,
  bridgeTimeout,
} from "./errors.js";
import { DIGEST_PATTERN, UUID_PATTERN } from "./schemas.js";

export const HAVEN_ENDPOINT_ENV_VAR = "HAVEN_MCP_ENDPOINT";
export const BROKER_PROTOCOL_VERSION = 1;
export const BROKER_MAX_FRAME_BYTES = 256 * 1024;
export const BROKER_MAX_REQUEST_BYTES = 64 * 1024;
export const BROKER_MAX_RESPONSE_BYTES = 256 * 1024;
export const BROKER_HANDSHAKE_TIMEOUT_MS = 5_000;

type JsonRecord = Record<string, unknown>;
type Platform = NodeJS.Platform | "unix";

export interface EndpointValidationOptions {
  platform?: Platform;
  env?: Record<string, string | undefined>;
}

export type EndpointValidation =
  | { ok: true }
  | { ok: false; reason: "missing" | "invalid" };

function isRecord(value: unknown): value is JsonRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactKeys(value: JsonRecord, keys: readonly string[]): boolean {
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
}

function isSafeUnixRoot(value: string | undefined): value is string {
  return (
    value !== undefined &&
    value.startsWith("/") &&
    value.length > 1 &&
    value.length <= 80 &&
    !value.includes("..") &&
    !/[\x00-\x20\x7f]/.test(value)
  );
}

function expectedUnixEndpoints(env: Record<string, string | undefined>): string[] {
  const endpoints: string[] = [];
  if (isSafeUnixRoot(env.XDG_RUNTIME_DIR)) {
    endpoints.push(`${env.XDG_RUNTIME_DIR}/haven/agent-v1.sock`);
  }
  if (isSafeUnixRoot(env.HOME)) {
    endpoints.push(`${env.HOME}/.haven/run/agent-v1.sock`);
  }
  return endpoints.filter((endpoint) => endpoint.length <= 100);
}

/**
 * 只验证端点的闭合形态，不做探测、不扫描文件系统、不尝试连接。
 *
 * Windows 的用户 SID 由 Rust 端读取并落实到 Named Pipe DACL；纯 Node 适配器不引入
 * FFI、子进程或文件读取来复制这段平台逻辑。它仍然只接受 Rust Broker 的固定 pipe 形态，
 * 其它用户的连接会在 OS ACL 层被拒绝。
 */
export function validateHavenEndpoint(
  raw: string | undefined,
  options: EndpointValidationOptions = {},
): EndpointValidation {
  if (raw === undefined || raw.length === 0) return { ok: false, reason: "missing" };

  const platform = options.platform ?? process.platform;
  const env = options.env ?? process.env;
  if (raw.length > (platform === "win32" ? 256 : 100)) return { ok: false, reason: "invalid" };
  if (/[\x00-\x20\x7f]/.test(raw) || raw.includes("://") || raw.includes("..") || raw.includes("@")) {
    return { ok: false, reason: "invalid" };
  }

  if (platform === "win32") {
    return /^\\\\\.\\pipe\\haven-agent-v1-[0-9a-f]{8}$/.test(raw)
      ? { ok: true }
      : { ok: false, reason: "invalid" };
  }

  return expectedUnixEndpoints(env).includes(raw)
    ? { ok: true }
    : { ok: false, reason: "invalid" };
}

interface BrokerWelcome {
  type: "welcome";
  protocol_version: number;
  session_id: string;
  haven: {
    app_version: string;
    agent_api_version: number;
    capabilities: HavenCapabilityReport["capabilities"];
  };
  granted_requests: string[];
}

interface BrokerErrorFrame {
  type: "error";
  id: number;
  error: { code: string; message: string; retryable: boolean };
}

type ConnectFn = (endpoint: string) => Socket;

function defaultConnect(endpoint: string): Socket {
  return connect(endpoint);
}

function protocolFailure(): HavenMcpError {
  return bridgeProtocolError("Haven Broker 返回了不符合协议的响应。");
}

function transportFailure(): HavenMcpError {
  return new HavenMcpError(
    ERROR_CODES.BRIDGE_UNAVAILABLE,
    "无法连接到正在运行的栖阅；请确认外部 Agent 接入已开启。",
    true,
  );
}

function isSafeText(value: unknown, max = 512): value is string {
  return typeof value === "string" && value.length <= max && !/[\x00-\x1f\x7f]/.test(value);
}

function assertString(value: unknown, max = 512): asserts value is string {
  if (!isSafeText(value, max)) throw protocolFailure();
}

function assertNullableString(value: unknown, max = 512): asserts value is string | null {
  if (value !== null) assertString(value, max);
}

function assertBoolean(value: unknown): asserts value is boolean {
  if (typeof value !== "boolean") throw protocolFailure();
}

function assertInteger(value: unknown): asserts value is number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) throw protocolFailure();
}

function assertFrameRecord(frame: unknown): asserts frame is JsonRecord {
  if (!isRecord(frame)) throw protocolFailure();
}

function assertWelcome(frame: unknown): BrokerWelcome {
  assertFrameRecord(frame);
  if (!hasExactKeys(frame, ["type", "protocol_version", "session_id", "haven", "granted_requests"])) {
    throw protocolFailure();
  }
  if (frame.type !== "welcome") throw protocolFailure();
  assertInteger(frame.protocol_version);
  if (frame.protocol_version !== BROKER_PROTOCOL_VERSION) throw protocolFailure();
  assertString(frame.session_id, 128);

  if (!isRecord(frame.haven) || !hasExactKeys(frame.haven, ["app_version", "agent_api_version", "capabilities"])) {
    throw protocolFailure();
  }
  assertString(frame.haven.app_version, 128);
  assertInteger(frame.haven.agent_api_version);
  if (!isRecord(frame.haven.capabilities)) throw protocolFailure();
  const capabilityKeys = [
    "settings_read",
    "settings_proposal",
    "library_summary_read",
    "setting_sources_read",
    "resource_preference_read",
    "resource_preference_proposal",
    "media_capabilities_read",
    "onboarding_read",
    "metadata_proposal",
    "rename_proposal",
    "secret_read",
    "filesystem_write",
  ] as const;
  if (!hasExactKeys(frame.haven.capabilities, capabilityKeys)) throw protocolFailure();
  for (const key of capabilityKeys) assertBoolean(frame.haven.capabilities[key]);

  const requestCapabilities: ReadonlyArray<readonly [string, keyof BrokerWelcome["haven"]["capabilities"]]> = [
    ["context", "settings_read"],
    ["setting_sources", "setting_sources_read"],
    ["resource_preference", "resource_preference_read"],
    ["library_summary", "library_summary_read"],
    ["media_capabilities", "media_capabilities_read"],
    ["onboarding", "onboarding_read"],
    ["create_proposal", "settings_proposal"],
    ["create_resource_proposal", "resource_preference_proposal"],
  ];
  if (!Array.isArray(frame.granted_requests)) {
    throw protocolFailure();
  }
  let previousIndex = -1;
  const granted = new Set<string>();
  for (const value of frame.granted_requests) {
    if (typeof value !== "string") throw protocolFailure();
    const index = requestCapabilities.findIndex(([request]) => request === value);
    if (index <= previousIndex || index < 0) throw protocolFailure();
    const capability = requestCapabilities[index]?.[1];
    if (capability === undefined || frame.haven.capabilities[capability] !== true) {
      throw protocolFailure();
    }
    previousIndex = index;
    granted.add(value);
  }
  for (const [request, capability] of requestCapabilities) {
    if (frame.haven.capabilities[capability] === true && !granted.has(request)) {
      throw protocolFailure();
    }
  }

  return frame as unknown as BrokerWelcome;
}

function assertErrorFrame(frame: unknown): BrokerErrorFrame {
  assertFrameRecord(frame);
  if (!hasExactKeys(frame, ["type", "id", "error"]) || frame.type !== "error") throw protocolFailure();
  assertInteger(frame.id);
  if (!isRecord(frame.error) || !hasExactKeys(frame.error, ["code", "message", "retryable"])) {
    throw protocolFailure();
  }
  assertString(frame.error.code, 64);
  assertString(frame.error.message, 1024);
  assertBoolean(frame.error.retryable);
  return frame as unknown as BrokerErrorFrame;
}

function assertOkFrame(frame: unknown, id: number): JsonRecord {
  assertFrameRecord(frame);
  if (!hasExactKeys(frame, ["type", "id", "payload"]) || frame.type !== "ok") throw protocolFailure();
  assertInteger(frame.id);
  if (frame.id !== id || !isRecord(frame.payload)) throw protocolFailure();
  return frame;
}

function brokerError(frame: BrokerErrorFrame): HavenMcpError {
  // Haven 的错误码是稳定的，但仍由 HavenMcpError 再做一次形状校验；消息最终还会经过
  // toErrorPayload 的脱敏/长度限制。
  return new HavenMcpError(frame.error.code, frame.error.message, frame.error.retryable);
}

class BrokerClient {
  private buffer = Buffer.alloc(0);
  private readonly frames: unknown[] = [];
  private readonly waiters: Array<{
    resolve: (frame: unknown) => void;
    reject: (error: unknown) => void;
    timer: ReturnType<typeof setTimeout>;
  }> = [];
  private closed = false;

  constructor(private readonly socket: Socket) {
    socket.on("data", (chunk: Buffer) => this.onData(chunk));
    socket.on("error", () => this.fail(transportFailure()));
    socket.on("close", () => this.fail(transportFailure()));
  }

  async send(frame: JsonRecord): Promise<void> {
    if (this.closed) throw transportFailure();
    const payload = Buffer.from(JSON.stringify(frame), "utf8");
    if (payload.length > BROKER_MAX_REQUEST_BYTES) throw protocolFailure();
    const output = Buffer.allocUnsafe(4 + payload.length);
    output.writeUInt32BE(payload.length, 0);
    payload.copy(output, 4);
    await new Promise<void>((resolve, reject) => {
      try {
        this.socket.write(output, () => resolve());
      } catch {
        reject(transportFailure());
      }
    });
  }

  async nextFrame(timeoutMs: number): Promise<unknown> {
    if (this.frames.length > 0) return this.frames.shift();
    if (this.closed) throw transportFailure();
    return await new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.removeWaiter(resolve, reject);
        reject(bridgeTimeout("Haven Broker 本地请求"));
      }, timeoutMs);
      this.waiters.push({ resolve, reject, timer });
    });
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.rejectWaiters(transportFailure());
    this.socket.destroy();
  }

  private onData(chunk: Buffer): void {
    if (this.closed || chunk.length === 0) return;
    if (this.buffer.length + chunk.length > BROKER_MAX_RESPONSE_BYTES + 4) {
      this.fail(protocolFailure());
      return;
    }
    this.buffer = Buffer.concat([this.buffer, chunk]);
    while (this.buffer.length >= 4) {
      const length = this.buffer.readUInt32BE(0);
      if (length > BROKER_MAX_FRAME_BYTES) {
        this.fail(protocolFailure());
        return;
      }
      if (this.buffer.length < length + 4) return;
      const payload = this.buffer.subarray(4, length + 4);
      this.buffer = this.buffer.subarray(length + 4);
      let frame: unknown;
      try {
        frame = JSON.parse(payload.toString("utf8")) as unknown;
      } catch {
        this.fail(protocolFailure());
        return;
      }
      this.deliver(frame);
    }
  }

  private deliver(frame: unknown): void {
    const waiter = this.waiters.shift();
    if (waiter) {
      clearTimeout(waiter.timer);
      waiter.resolve(frame);
    } else {
      this.frames.push(frame);
    }
  }

  private fail(error: HavenMcpError): void {
    if (this.closed) return;
    this.closed = true;
    this.rejectWaiters(error);
    this.socket.destroy();
  }

  private rejectWaiters(error: HavenMcpError): void {
    for (const waiter of this.waiters.splice(0)) {
      clearTimeout(waiter.timer);
      waiter.reject(error);
    }
  }

  private removeWaiter(resolve: (frame: unknown) => void, reject: (error: unknown) => void): void {
    const index = this.waiters.findIndex((waiter) => waiter.resolve === resolve && waiter.reject === reject);
    if (index >= 0) this.waiters.splice(index, 1);
  }
}

async function openClient(endpoint: string, connectFn: ConnectFn): Promise<BrokerClient> {
  const socket = await new Promise<Socket>((resolve, reject) => {
    let settled = false;
    const fail = (): void => {
      if (settled) return;
      settled = true;
      candidate?.destroy();
      reject(transportFailure());
    };
    let candidate: Socket | undefined;
    try {
      candidate = connectFn(endpoint);
    } catch {
      fail();
      return;
    }
    candidate.once("connect", () => {
      if (settled) return;
      settled = true;
      candidate.setTimeout(0);
      resolve(candidate);
    });
    candidate.once("error", fail);
    candidate.setTimeout(BRIDGE_TIMEOUT_MS, fail);
  });

  const client = new BrokerClient(socket);
  try {
    await client.send({
      type: "hello",
      protocol_version: BROKER_PROTOCOL_VERSION,
      client: { name: "haven-mcp-server", version: SERVER_VERSION },
    });
    const welcome = assertWelcome(await client.nextFrame(BROKER_HANDSHAKE_TIMEOUT_MS));
    // 让连接对象持有 welcome，调用者无需重复完成握手。
    (client as BrokerClient & { welcome?: BrokerWelcome }).welcome = welcome;
    return client;
  } catch (error) {
    client.close();
    throw error;
  }
}

function welcomeOf(client: BrokerClient): BrokerWelcome {
  const welcome = (client as BrokerClient & { welcome?: BrokerWelcome }).welcome;
  if (welcome === undefined) throw protocolFailure();
  return welcome;
}

function ensureCapability(welcome: BrokerWelcome, capability: keyof HavenCapabilityReport["capabilities"]): void {
  if (!welcome.haven.capabilities[capability]) throw capabilityUnavailable(capability);
}

type BrokerRequestType =
  | "context"
  | "setting_sources"
  | "resource_preference"
  | "library_summary"
  | "media_capabilities"
  | "onboarding"
  | "create_proposal"
  | "create_resource_proposal";

async function requestPayload(
  client: BrokerClient,
  id: number,
  type: BrokerRequestType,
  payload: JsonRecord,
): Promise<JsonRecord> {
  await client.send({ type, id, payload });
  const frame = await client.nextFrame(BRIDGE_TIMEOUT_MS);
  assertFrameRecord(frame);
  if (frame.type === "error") {
    const errorFrame = assertErrorFrame(frame);
    // 与 ok 帧一样，error 帧必须回带本次请求的 id，否则视为协议违规而不是业务错误。
    if (errorFrame.id !== id) throw protocolFailure();
    throw brokerError(errorFrame);
  }
  return assertOkFrame(frame, id);
}

function assertContextPayload(payload: JsonRecord): SettingsSnapshotResult {
  if (
    !hasExactKeys(payload, ["schemaVersion", "contextId", "contextHash", "subject", "revision", "reading", "capabilities"]) ||
    payload.schemaVersion !== 1 ||
    !isRecord(payload.subject) ||
    !hasExactKeys(payload.subject, ["section"]) ||
    payload.subject.section !== "reading" ||
    !isRecord(payload.reading)
  ) {
    throw protocolFailure();
  }
  assertString(payload.contextId, 128);
  if (!UUID_PATTERN.test(payload.contextId)) throw protocolFailure();
  assertString(payload.contextHash, 64);
  if (!DIGEST_PATTERN.test(payload.contextHash)) throw protocolFailure();
  assertNullableString(payload.revision, 128);

  const readingKeys = [
    "section",
    "fontFamily",
    "customFontFamily",
    "fontSize",
    "lineHeight",
    "contentWidth",
    "theme",
    "customBackground",
    "customText",
    "fontWeight",
    "letterSpacing",
    "systemAuto",
    "pagination",
    "redactedFields",
  ] as const;
  if (!hasExactKeys(payload.reading, readingKeys) || payload.reading.section !== "reading") throw protocolFailure();
  for (const key of [
    "fontFamily",
    "fontSize",
    "lineHeight",
    "contentWidth",
    "theme",
    "fontWeight",
    "letterSpacing",
    "pagination",
  ] as const) {
    assertString(payload.reading[key], 64);
  }
  for (const key of ["customFontFamily", "customBackground", "customText"] as const) {
    assertNullableString(payload.reading[key], 200);
  }
  assertBoolean(payload.reading.systemAuto);
  if (
    !Array.isArray(payload.reading.redactedFields) ||
    payload.reading.redactedFields.some((value) => !isSafeText(value, 128))
  ) {
    throw protocolFailure();
  }
  // Context DTO 使用 Wire 的 camelCase，而 welcome 使用 Broker 的 snake_case；两套形状
  // 都必须闭合，不能把未知字段直接投影给 MCP。
  if (!isRecord(payload.capabilities) || !hasExactKeys(payload.capabilities, ["agentApiVersion", "capabilities"])) {
    throw protocolFailure();
  }
  assertInteger(payload.capabilities.agentApiVersion);
  if (!isRecord(payload.capabilities.capabilities)) throw protocolFailure();
  const contextCapabilityKeys = [
    "settingsRead",
    "settingsProposal",
    "librarySummaryRead",
    "settingSourcesRead",
    "resourcePreferenceRead",
    "resourcePreferenceProposal",
    "mediaCapabilitiesRead",
    "onboardingRead",
    "metadataProposal",
    "renameProposal",
    "secretRead",
    "filesystemWrite",
  ] as const;
  if (!hasExactKeys(payload.capabilities.capabilities, contextCapabilityKeys)) throw protocolFailure();
  for (const key of contextCapabilityKeys) assertBoolean(payload.capabilities.capabilities[key]);

  const snakeSettings: Record<string, unknown> = {
    font_family: payload.reading.fontFamily,
    custom_font_family: payload.reading.customFontFamily,
    font_size: payload.reading.fontSize,
    line_height: payload.reading.lineHeight,
    content_width: payload.reading.contentWidth,
    theme: payload.reading.theme,
    custom_background: payload.reading.customBackground,
    custom_text: payload.reading.customText,
    font_weight: payload.reading.fontWeight,
    letter_spacing: payload.reading.letterSpacing,
    system_auto: payload.reading.systemAuto,
    pagination: payload.reading.pagination,
  };
  return {
    subject_scope: "global",
    section: "reading",
    context_id: payload.contextId,
    context_hash: payload.contextHash,
    revision: payload.revision,
    settings: snakeSettings,
    redacted_fields: payload.reading.redactedFields.map((value) =>
      value.replace(/[A-Z]/g, (letter: string) => `_${letter.toLowerCase()}`),
    ),
  };
}

function assertSchemaVersion(payload: JsonRecord): void {
  if (payload.schemaVersion !== 1) throw protocolFailure();
}

function assertArray(value: unknown, max: number): asserts value is unknown[] {
  if (!Array.isArray(value) || value.length > max) throw protocolFailure();
}

function assertNonNegativeInteger(value: unknown): asserts value is number {
  assertInteger(value);
  if (value < 0) throw protocolFailure();
}

function assertEnum(value: unknown, allowed: readonly string[]): asserts value is string {
  assertString(value, 64);
  if (!allowed.includes(value)) throw protocolFailure();
}

function assertOptionalPatch(
  value: unknown,
  keys: readonly string[],
  booleanKeys: readonly string[] = [],
): Record<string, unknown> | null {
  if (value === null) return null;
  if (!isRecord(value)) throw protocolFailure();
  const actual = Object.keys(value);
  if (actual.some((key) => !keys.includes(key))) throw protocolFailure();
  for (const key of actual) {
    const item = value[key];
    if (item === null) continue;
    if (booleanKeys.includes(key)) assertBoolean(item);
    else assertString(item, 200);
  }
  return { ...value };
}

function projectPreferencePatch(value: unknown, kind: "reading" | "comic"): Record<string, unknown> | null {
  if (kind === "reading") {
    const patch = assertOptionalPatch(
      value,
      [
        "fontFamily",
        "customFontFamily",
        "fontSize",
        "lineHeight",
        "contentWidth",
        "theme",
        "customBackground",
        "customText",
        "fontWeight",
        "letterSpacing",
        "systemAuto",
        "pagination",
      ],
      ["systemAuto"],
    );
    if (patch === null) return null;
    const names: Record<string, string> = {
      fontFamily: "font_family",
      customFontFamily: "custom_font_family",
      fontSize: "font_size",
      lineHeight: "line_height",
      contentWidth: "content_width",
      customBackground: "custom_background",
      customText: "custom_text",
      fontWeight: "font_weight",
      letterSpacing: "letter_spacing",
      systemAuto: "system_auto",
    };
    return Object.fromEntries(Object.entries(patch).map(([key, item]) => [names[key] ?? key, item]));
  }

  const patch = assertOptionalPatch(value, ["viewMode", "direction", "pageGap", "preloadPages"]);
  if (patch === null) return null;
  const names: Record<string, string> = {
    viewMode: "view_mode",
    pageGap: "page_gap",
    preloadPages: "preload_pages",
  };
  return Object.fromEntries(Object.entries(patch).map(([key, item]) => [names[key] ?? key, item]));
}

function assertSettingSourcesPayload(payload: JsonRecord): SettingSourcesResult {
  if (!hasExactKeys(payload, ["schemaVersion", "section", "revision", "layers"])) throw protocolFailure();
  assertSchemaVersion(payload);
  if (payload.section !== "reading") throw protocolFailure();
  assertNullableString(payload.revision, 128);
  assertArray(payload.layers, 4);
  const layers: SettingSourcesResult["layers"] = [];
  for (const layer of payload.layers) {
    if (!isRecord(layer) || !hasExactKeys(layer, ["layer", "present", "revision"])) throw protocolFailure();
    assertEnum(layer.layer, ["default", "global", "edition", "media_item"]);
    assertBoolean(layer.present);
    assertNullableString(layer.revision, 128);
    layers.push({
      layer: layer.layer as SettingSourcesResult["layers"][number]["layer"],
      present: layer.present,
      revision: layer.revision,
    });
  }
  return { section: "reading", revision: payload.revision, layers };
}

function assertResourcePreferencePayload(payload: JsonRecord): ResourcePreferenceSnapshotResult {
  if (
    !hasExactKeys(payload, [
      "schemaVersion",
      "contextId",
      "contextHash",
      "targetScope",
      "editionId",
      "mediaItemId",
      "revision",
      "reading",
      "comic",
    ])
  ) throw protocolFailure();
  assertSchemaVersion(payload);
  assertString(payload.contextId, 128);
  if (!UUID_PATTERN.test(payload.contextId)) throw protocolFailure();
  assertString(payload.contextHash, 64);
  if (!DIGEST_PATTERN.test(payload.contextHash)) throw protocolFailure();
  assertEnum(payload.targetScope, ["edition", "media_item"]);
  assertString(payload.editionId, 128);
  if (!UUID_PATTERN.test(payload.editionId)) throw protocolFailure();
  assertNullableString(payload.mediaItemId, 128);
  if (payload.mediaItemId !== null && !UUID_PATTERN.test(payload.mediaItemId)) throw protocolFailure();
  assertNullableString(payload.revision, 128);
  return {
    context_id: payload.contextId,
    context_hash: payload.contextHash,
    target_scope: payload.targetScope as PreferenceScope,
    edition_id: payload.editionId,
    media_item_id: payload.mediaItemId,
    revision: payload.revision,
    reading: projectPreferencePatch(payload.reading, "reading"),
    comic: projectPreferencePatch(payload.comic, "comic"),
  };
}

function assertLibrarySummaryPayload(payload: JsonRecord): LibrarySummaryResult {
  if (!hasExactKeys(payload, ["schemaVersion", "counts", "categories", "recent", "truncated"])) throw protocolFailure();
  assertSchemaVersion(payload);
  if (!isRecord(payload.counts) || !hasExactKeys(payload.counts, ["works", "editions", "mediaItems", "favorites", "inProgress"])) {
    throw protocolFailure();
  }
  for (const key of ["works", "editions", "mediaItems", "favorites", "inProgress"] as const) {
    assertNonNegativeInteger(payload.counts[key]);
  }
  const counts = payload.counts as {
    works: number;
    editions: number;
    mediaItems: number;
    favorites: number;
    inProgress: number;
  };
  assertArray(payload.categories, 8);
  const categories: LibrarySummaryResult["categories"] = [];
  for (const category of payload.categories) {
    if (!isRecord(category) || !hasExactKeys(category, ["category", "workCount"])) throw protocolFailure();
    assertString(category.category, 64);
    assertNonNegativeInteger(category.workCount);
    categories.push({ category: category.category, work_count: category.workCount });
  }
  assertArray(payload.recent, 50);
  const recent: LibrarySummaryResult["recent"] = [];
  for (const item of payload.recent) {
    if (!isRecord(item) || !hasExactKeys(item, ["workId", "title", "category", "progressRatio"])) throw protocolFailure();
    assertString(item.workId, 128);
    if (!UUID_PATTERN.test(item.workId)) throw protocolFailure();
    assertString(item.title, 256);
    assertString(item.category, 64);
    if (item.progressRatio !== null) {
      if (typeof item.progressRatio !== "number" || !Number.isFinite(item.progressRatio) || item.progressRatio < 0 || item.progressRatio > 1) {
        throw protocolFailure();
      }
    }
    recent.push({
      work_id: item.workId,
      title: item.title,
      category: item.category,
      progress_ratio: item.progressRatio,
    });
  }
  assertBoolean(payload.truncated);
  return {
    counts: {
      works: counts.works,
      editions: counts.editions,
      media_items: counts.mediaItems,
      favorites: counts.favorites,
      in_progress: counts.inProgress,
    },
    categories,
    recent,
    truncated: payload.truncated,
  };
}

function assertMediaCapabilitiesPayload(payload: JsonRecord): MediaCapabilitiesResult {
  if (!hasExactKeys(payload, ["schemaVersion", "items", "truncated"])) throw protocolFailure();
  assertSchemaVersion(payload);
  assertArray(payload.items, 50);
  const items: MediaCapabilitiesResult["items"] = [];
  for (const item of payload.items) {
    if (
      !isRecord(item) ||
      !hasExactKeys(item, [
        "mediaItemId",
        "mediaType",
        "availability",
        "canOpenSession",
        "canExtractText",
        "canRenderPages",
        "declaredCapabilities",
      ])
    ) throw protocolFailure();
    assertString(item.mediaItemId, 128);
    if (!UUID_PATTERN.test(item.mediaItemId)) throw protocolFailure();
    assertString(item.mediaType, 64);
    assertString(item.availability, 64);
    for (const key of ["canOpenSession", "canExtractText", "canRenderPages"] as const) assertBoolean(item[key]);
    assertArray(item.declaredCapabilities, 8);
    item.declaredCapabilities.forEach((capability) => assertString(capability, 64));
    const media = item as {
      mediaItemId: string;
      mediaType: string;
      availability: string;
      canOpenSession: boolean;
      canExtractText: boolean;
      canRenderPages: boolean;
      declaredCapabilities: string[];
    };
    items.push({
      media_item_id: media.mediaItemId,
      media_type: media.mediaType,
      availability: media.availability,
      can_open_session: media.canOpenSession,
      can_extract_text: media.canExtractText,
      can_render_pages: media.canRenderPages,
      declared_capabilities: media.declaredCapabilities,
    });
  }
  assertBoolean(payload.truncated);
  return { items, truncated: payload.truncated };
}

function assertOnboardingPayload(payload: JsonRecord): OnboardingStateResult {
  if (!hasExactKeys(payload, ["schemaVersion", "completedSteps", "nextStep", "hasStorageLocation", "hasLibraryContent", "hasAiProvider"])) {
    throw protocolFailure();
  }
  assertSchemaVersion(payload);
  assertArray(payload.completedSteps, 16);
  payload.completedSteps.forEach((step) => assertString(step, 64));
  assertNullableString(payload.nextStep, 64);
  assertBoolean(payload.hasStorageLocation);
  assertBoolean(payload.hasLibraryContent);
  assertBoolean(payload.hasAiProvider);
  const completedSteps = payload.completedSteps as string[];
  const onboarding = payload as {
    nextStep: string | null;
    hasStorageLocation: boolean;
    hasLibraryContent: boolean;
    hasAiProvider: boolean;
  };
  return {
    completed_steps: completedSteps,
    next_step: onboarding.nextStep,
    has_storage_location: onboarding.hasStorageLocation,
    has_library_content: onboarding.hasLibraryContent,
    has_ai_provider: onboarding.hasAiProvider,
  };
}

function assertProposalPayload(payload: JsonRecord): ProposalOutcome {
  const settingsShape = [
    "schemaVersion",
    "proposalId",
    "status",
    "subject",
    "targetLabel",
    "baseRevision",
    "digest",
    "createdAt",
    "expiresAt",
    "changes",
  ];
  const resourceShape = [
    "schemaVersion",
    "proposalId",
    "status",
    "targetScope",
    "targetLabel",
    "editionId",
    "mediaItemId",
    "baseRevision",
    "digest",
    "createdAt",
    "expiresAt",
    "changes",
  ];
  const isSettings = hasExactKeys(payload, settingsShape);
  const isResource = hasExactKeys(payload, resourceShape);
  if ((!isSettings && !isResource) || payload.status !== "pending") throw protocolFailure();
  assertSchemaVersion(payload);
  assertString(payload.proposalId, 128);
  if (!UUID_PATTERN.test(payload.proposalId)) throw protocolFailure();
  assertString(payload.targetLabel, 256);
  assertNullableString(payload.baseRevision, 128);
  assertString(payload.digest, 64);
  if (!DIGEST_PATTERN.test(payload.digest)) throw protocolFailure();
  assertString(payload.createdAt, 128);
  assertString(payload.expiresAt, 128);
  let subjectScope: ProposalOutcome["subject_scope"];
  let section: SettingsProposalRequest["section"] | null;
  let editionId: string | null = null;
  let mediaItemId: string | null = null;
  if (isSettings) {
    if (!isRecord(payload.subject) || !hasExactKeys(payload.subject, ["section"]) || payload.subject.section !== "reading") throw protocolFailure();
    subjectScope = "global";
    section = "reading";
  } else {
    assertEnum(payload.targetScope, ["edition", "media_item"]);
    assertString(payload.editionId, 128);
    if (!UUID_PATTERN.test(payload.editionId)) throw protocolFailure();
    assertNullableString(payload.mediaItemId, 128);
    if (payload.mediaItemId !== null && !UUID_PATTERN.test(payload.mediaItemId)) throw protocolFailure();
    if (payload.targetScope === "media_item" && payload.mediaItemId === null) throw protocolFailure();
    if (payload.targetScope === "edition" && payload.mediaItemId !== null) throw protocolFailure();
    subjectScope = payload.targetScope as ProposalOutcome["subject_scope"];
    section = null;
    editionId = payload.editionId;
    mediaItemId = payload.mediaItemId;
  }
  assertArray(payload.changes, 64);
  const changes: { key: string; before: string; after: string }[] = [];
  for (const change of payload.changes) {
    if (!isRecord(change) || !hasExactKeys(change, ["key", "before", "after"])) throw protocolFailure();
    assertString(change.key, 128);
    assertString(change.before, 256);
    assertString(change.after, 256);
    changes.push({ key: change.key, before: change.before, after: change.after });
  }
  return {
    proposal_id: payload.proposalId,
    status: "pending",
    digest: payload.digest,
    target_label: payload.targetLabel,
    subject_scope: subjectScope,
    section,
    edition_id: editionId,
    media_item_id: mediaItemId,
    base_revision: payload.baseRevision,
    created_at: payload.createdAt,
    expires_at: payload.expiresAt,
    changes,
  };
}

export interface LiveHavenAgentBridgeOptions extends EndpointValidationOptions {
  connect?: ConnectFn;
}

export class LiveHavenAgentBridge implements HavenAgentBridge {
  readonly kind = "live" as const;
  readonly available: boolean;
  readonly statusDetail: string;
  private readonly endpoint: string;
  private readonly endpointError: HavenMcpError | null;
  private readonly connectFn: ConnectFn;

  constructor(endpoint: string, options: LiveHavenAgentBridgeOptions = {}) {
    this.endpoint = endpoint;
    this.connectFn = options.connect ?? defaultConnect;
    const validation = validateHavenEndpoint(endpoint, options);
    this.available = validation.ok;
    this.endpointError = validation.ok ? null : endpointInvalid();
    this.statusDetail = validation.ok
      ? "已配置 Haven 本地 Broker 端点；连接状态将在调用时确认。"
      : "HAVEN_MCP_ENDPOINT 无效；未尝试连接。";
  }

  async getCapabilityManifest(): Promise<HavenCapabilityReport> {
    const client = await this.open();
    try {
      return this.toCapabilityReport(welcomeOf(client));
    } finally {
      client.close();
    }
  }

  async getSettingsSnapshot(): Promise<SettingsSnapshotResult> {
    const client = await this.open();
    try {
      const welcome = welcomeOf(client);
      ensureCapability(welcome, "settings_read");
      const response = await requestPayload(client, 1, "context", {});
      return assertContextPayload(response.payload as JsonRecord);
    } finally {
      client.close();
    }
  }

  async proposeSettingsPatch(request: SettingsProposalRequest): Promise<ProposalOutcome> {
    const client = await this.open();
    try {
      const welcome = welcomeOf(client);
      ensureCapability(welcome, "settings_proposal");
      const response = await requestPayload(client, 1, "create_proposal", {
        section: request.section,
        context_id: request.context_id,
        context_hash: request.context_hash,
        base_revision: request.base_revision,
        patch: request.patch,
      });
      return assertProposalPayload(response.payload as JsonRecord);
    } finally {
      client.close();
    }
  }

  async getSettingSources(): Promise<SettingSourcesResult> {
    const client = await this.open();
    try {
      const welcome = welcomeOf(client);
      ensureCapability(welcome, "setting_sources_read");
      const response = await requestPayload(client, 1, "setting_sources", { section: "reading" });
      return assertSettingSourcesPayload(response.payload as JsonRecord);
    } finally {
      client.close();
    }
  }

  async getResourcePreferenceSnapshot(request: {
    target_scope: PreferenceScope;
    edition_id: string;
    media_item_id: string | null;
  }): Promise<ResourcePreferenceSnapshotResult> {
    const client = await this.open();
    try {
      const welcome = welcomeOf(client);
      ensureCapability(welcome, "resource_preference_read");
      const response = await requestPayload(client, 1, "resource_preference", {
        target_scope: request.target_scope,
        edition_id: request.edition_id,
        media_item_id: request.media_item_id,
      });
      return assertResourcePreferencePayload(response.payload as JsonRecord);
    } finally {
      client.close();
    }
  }

  async getLibrarySummary(request: { limit: number }): Promise<LibrarySummaryResult> {
    const client = await this.open();
    try {
      const welcome = welcomeOf(client);
      ensureCapability(welcome, "library_summary_read");
      const response = await requestPayload(client, 1, "library_summary", { limit: request.limit });
      return assertLibrarySummaryPayload(response.payload as JsonRecord);
    } finally {
      client.close();
    }
  }

  async getMediaCapabilities(request: { media_item_id: string | null; limit: number }): Promise<MediaCapabilitiesResult> {
    const client = await this.open();
    try {
      const welcome = welcomeOf(client);
      ensureCapability(welcome, "media_capabilities_read");
      const response = await requestPayload(client, 1, "media_capabilities", {
        media_item_id: request.media_item_id,
        limit: request.limit,
      });
      return assertMediaCapabilitiesPayload(response.payload as JsonRecord);
    } finally {
      client.close();
    }
  }

  async getOnboardingState(): Promise<OnboardingStateResult> {
    const client = await this.open();
    try {
      const welcome = welcomeOf(client);
      ensureCapability(welcome, "onboarding_read");
      const response = await requestPayload(client, 1, "onboarding", {});
      return assertOnboardingPayload(response.payload as JsonRecord);
    } finally {
      client.close();
    }
  }

  async proposeResourcePreferencePatch(request: ResourcePreferenceProposalRequest): Promise<ProposalOutcome> {
    const client = await this.open();
    try {
      const welcome = welcomeOf(client);
      ensureCapability(welcome, "resource_preference_proposal");
      const response = await requestPayload(client, 1, "create_resource_proposal", {
        target_scope: request.target_scope,
        edition_id: request.edition_id,
        media_item_id: request.media_item_id,
        context_id: request.context_id,
        context_hash: request.context_hash,
        base_revision: request.base_revision,
        patch: request.patch,
      });
      return assertProposalPayload(response.payload as JsonRecord);
    } finally {
      client.close();
    }
  }

  private async open(): Promise<BrokerClient> {
    if (this.endpointError !== null) throw this.endpointError;
    return openClient(this.endpoint, this.connectFn);
  }

  private toCapabilityReport(welcome: BrokerWelcome): HavenCapabilityReport {
    return {
      agent_api_version: welcome.haven.agent_api_version,
      capabilities: { ...welcome.haven.capabilities },
    };
  }
}
