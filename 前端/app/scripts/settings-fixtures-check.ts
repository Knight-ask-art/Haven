// Settings Fixture 装载验证 + Mock 行为消费断言 + 表单状态机测试（FE-SETTINGS-001）。
// 与 fixtures-check.ts 同一模式：守卫先行（杜绝裸 as）→ 类型断言 → 语义断言 →
// MockHavenClient 消费 → 表单状态机（loading/dirty/saved/validation-error/REVISION_CONFLICT/retry）。
// 验收点：
// - 默认值加载（general/appearance/playback/reading/comic/downloads/privacy defaults + revision null）；
// - 成功更新（changed=true + 新 revision + 事件 revision 同源）；
// - 重复更新幂等（changed=false + 同一 revision + 不产生 settings.changed 事件）；
// - 空 patch 幂等（不写状态、不发事件、不制造假保存）；
// - stale revision 冲突（expected 校验先于一切；状态不被破坏）；
// - 相同值修改不进入 dirty、改回相同值清除 dirty、save 不制造空 patch；
// - secret 只以 configured/status 呈现：fixture 与 Mock DTO 均无 secret 字段。

import type {
  SettingsChangedDto,
  SettingsSnapshot,
  SettingsUpdateResult,
  SettingsValue,
} from "../src/lib/ipc/settings-wire";
import {
  buildSettingsPatch,
  guardSettingsChanged,
  guardSettingsSnapshot,
  guardSettingsUpdateResult,
  settingsValuesEqual,
} from "../src/lib/ipc/settings-wire.js";
import type { ErrorDto } from "../src/lib/ipc/generated/wire";
import { MockHavenClient } from "../src/lib/ipc/mock-client.js";
import { HavenError, isErrorDto, toHavenError } from "../src/lib/ipc/errors.js";
import type { SettingsFormState } from "../src/features/settings/lib/settingsForm";
import {
  formChange,
  formLoaded,
  formRebase,
  formRebaseFailed,
  formResetToDefaults,
  formRetrySave,
  formSave,
  formSaveFailure,
  formSaveSuccess,
  initialFormState,
} from "../src/features/settings/lib/settingsForm.js";
import type { SettingsGateway } from "../src/features/settings/ipc/gateway";
import {
  runSettingsRebaseFetch,
  runSettingsSaveOperation,
  settingsFormErrorInfo,
} from "../src/features/settings/lib/useSettingsForm.js";

import settingsGeneralDefault from "../../../contracts/ipc/v1/fixtures/settings/general.default.json" with { type: "json" };
import settingsAppearanceDefault from "../../../contracts/ipc/v1/fixtures/settings/appearance.default.json" with { type: "json" };
import settingsPlaybackDefault from "../../../contracts/ipc/v1/fixtures/settings/playback.default.json" with { type: "json" };
import settingsPrivacyDefault from "../../../contracts/ipc/v1/fixtures/settings/privacy.default.json" with { type: "json" };
import settingsGeneralSaved from "../../../contracts/ipc/v1/fixtures/settings/general.saved.json" with { type: "json" };
import settingsAppearanceSaved from "../../../contracts/ipc/v1/fixtures/settings/appearance.saved.json" with { type: "json" };
import settingsPlaybackSaved from "../../../contracts/ipc/v1/fixtures/settings/playback.saved.json" with { type: "json" };
import settingsReadingDefault from "../../../contracts/ipc/v1/fixtures/settings/reading.default.json" with { type: "json" };
import settingsReadingSaved from "../../../contracts/ipc/v1/fixtures/settings/reading.saved.json" with { type: "json" };
import settingsComicDefault from "../../../contracts/ipc/v1/fixtures/settings/comic.default.json" with { type: "json" };
import settingsComicSaved from "../../../contracts/ipc/v1/fixtures/settings/comic.saved.json" with { type: "json" };
import settingsDownloadsDefault from "../../../contracts/ipc/v1/fixtures/settings/downloads.default.json" with { type: "json" };
import settingsDownloadsSaved from "../../../contracts/ipc/v1/fixtures/settings/downloads.saved.json" with { type: "json" };
import settingsUpdateChanged from "../../../contracts/ipc/v1/fixtures/settings/update.result.changed.json" with { type: "json" };
import settingsUpdateIdempotent from "../../../contracts/ipc/v1/fixtures/settings/update.result.idempotent.json" with { type: "json" };
import settingsConflictError from "../../../contracts/ipc/v1/fixtures/settings/update.error-revision-conflict.json" with { type: "json" };
import settingsInvalidArgument from "../../../contracts/ipc/v1/fixtures/settings/update.error-invalid-argument.json" with { type: "json" };
import settingsChangedEvent from "../../../contracts/ipc/v1/fixtures/events/settings.changed.json" with { type: "json" };

function fail(msg: string, data?: unknown): never {
  throw new Error(`${msg}${data === undefined ? "" : `: ${JSON.stringify(data).slice(0, 200)}`}`);
}

function deepEqual(a: unknown, b: unknown): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

// ---- 守卫：守卫通过后才允许断言为 wire 类型（守卫先行，杜绝裸 as）----

const checkGeneralDefault: SettingsSnapshot = guardSettingsSnapshot(settingsGeneralDefault)
  ? settingsGeneralDefault
  : fail("general.default fixture 形状非法", settingsGeneralDefault);
const checkAppearanceDefault: SettingsSnapshot = guardSettingsSnapshot(settingsAppearanceDefault)
  ? settingsAppearanceDefault
  : fail("appearance.default fixture 形状非法", settingsAppearanceDefault);
const checkPlaybackDefault: SettingsSnapshot = guardSettingsSnapshot(settingsPlaybackDefault)
  ? settingsPlaybackDefault
  : fail("playback.default fixture 形状非法", settingsPlaybackDefault);
const checkReadingDefault: SettingsSnapshot = guardSettingsSnapshot(settingsReadingDefault)
  ? settingsReadingDefault
  : fail("reading.default fixture 形状非法", settingsReadingDefault);
const checkComicDefault: SettingsSnapshot = guardSettingsSnapshot(settingsComicDefault)
  ? settingsComicDefault
  : fail("comic.default fixture 形状非法", settingsComicDefault);
const checkDownloadsDefault: SettingsSnapshot = guardSettingsSnapshot(settingsDownloadsDefault)
  ? settingsDownloadsDefault
  : fail("downloads.default fixture 形状非法", settingsDownloadsDefault);
const checkPrivacyDefault: SettingsSnapshot = guardSettingsSnapshot(settingsPrivacyDefault)
  ? settingsPrivacyDefault
  : fail("privacy.default fixture 形状非法", settingsPrivacyDefault);
const checkGeneralSaved: SettingsSnapshot = guardSettingsSnapshot(settingsGeneralSaved)
  ? settingsGeneralSaved
  : fail("general.saved fixture 形状非法", settingsGeneralSaved);
const checkAppearanceSaved: SettingsSnapshot = guardSettingsSnapshot(settingsAppearanceSaved)
  ? settingsAppearanceSaved
  : fail("appearance.saved fixture 形状非法", settingsAppearanceSaved);
const checkPlaybackSaved: SettingsSnapshot = guardSettingsSnapshot(settingsPlaybackSaved)
  ? settingsPlaybackSaved
  : fail("playback.saved fixture 形状非法", settingsPlaybackSaved);
const checkReadingSaved: SettingsSnapshot = guardSettingsSnapshot(settingsReadingSaved)
  ? settingsReadingSaved
  : fail("reading.saved fixture 形状非法", settingsReadingSaved);
const checkComicSaved: SettingsSnapshot = guardSettingsSnapshot(settingsComicSaved)
  ? settingsComicSaved
  : fail("comic.saved fixture 形状非法", settingsComicSaved);
const checkDownloadsSaved: SettingsSnapshot = guardSettingsSnapshot(settingsDownloadsSaved)
  ? settingsDownloadsSaved
  : fail("downloads.saved fixture 形状非法", settingsDownloadsSaved);
const checkUpdateChanged: SettingsUpdateResult = guardSettingsUpdateResult(settingsUpdateChanged)
  ? settingsUpdateChanged
  : fail("update.result.changed fixture 形状非法", settingsUpdateChanged);
const checkUpdateIdempotent: SettingsUpdateResult = guardSettingsUpdateResult(settingsUpdateIdempotent)
  ? settingsUpdateIdempotent
  : fail("update.result.idempotent fixture 形状非法", settingsUpdateIdempotent);
const checkConflict: ErrorDto = isErrorDto(settingsConflictError)
  ? settingsConflictError
  : fail("update.error-revision-conflict fixture 形状非法", settingsConflictError);
const checkInvalidArgument: ErrorDto = isErrorDto(settingsInvalidArgument)
  ? settingsInvalidArgument
  : fail("update.error-invalid-argument fixture 形状非法", settingsInvalidArgument);
const checkChangedEvent: SettingsChangedDto = guardSettingsChanged(settingsChangedEvent)
  ? settingsChangedEvent
  : fail("settings.changed fixture 形状非法", settingsChangedEvent);

// 错误适配回归：已有 HavenError 必须保留稳定 code / message / retryable，不能
// 因为它不是裸 ErrorDto JSON 而被 toHavenError 降级成 INTERNAL_ERROR。
const revisionConflictError = new HavenError(checkConflict);
const adaptedRevisionConflict = toHavenError(revisionConflictError);
if (
  adaptedRevisionConflict !== revisionConflictError ||
  adaptedRevisionConflict.code !== "REVISION_CONFLICT" ||
  adaptedRevisionConflict.message !== revisionConflictError.message ||
  adaptedRevisionConflict.retryable !== false
) {
  fail("HavenError(REVISION_CONFLICT) 适配不得降级为 INTERNAL_ERROR", {
    code: adaptedRevisionConflict.code,
    message: adaptedRevisionConflict.message,
    retryable: adaptedRevisionConflict.retryable,
  });
}

// ---- 语义断言 ----

if (checkGeneralDefault.revision !== null) fail("general 从未保存 → revision 必须 null");
if (checkAppearanceDefault.revision !== null) fail("appearance 从未保存 → revision 必须 null");
if (checkPlaybackDefault.revision !== null) fail("playback 从未保存 → revision 必须 null");
if (checkReadingDefault.revision !== null) fail("reading 从未保存 → revision 必须 null");
if (checkComicDefault.revision !== null) fail("comic 从未保存 → revision 必须 null");
if (checkDownloadsDefault.revision !== null) fail("downloads 从未保存 → revision 必须 null");
if (checkPrivacyDefault.revision !== null) fail("privacy 从未保存 → revision 必须 null");
if (checkPrivacyDefault.value.section !== "privacy"
  || !checkPrivacyDefault.value.searchHistory
  || !checkPrivacyDefault.value.playbackHistory) {
  fail("privacy.default 必须开启搜索历史、播放与阅读历史并使用 privacy section");
}
if (checkPlaybackDefault.value.section !== "playback" || !checkPlaybackDefault.value.autoNext) {
  fail("playback.default 必须开启自动下一集并使用 playback section");
}
if (checkPlaybackSaved.value.section !== "playback" || checkPlaybackSaved.value.autoNext) {
  fail("playback.saved 必须覆盖自动下一集设置并使用 playback section");
}
if (checkComicDefault.value.section !== "comic"
  || checkComicDefault.value.viewMode !== "single"
  || checkComicDefault.value.direction !== "rtl"
  || checkComicDefault.value.pageGap !== "twelve"
  || checkComicDefault.value.preloadPages !== "three") {
  fail("comic.default 必须使用单页/RTL/12px/3页安全默认值");
}
if (checkComicSaved.value.section !== "comic"
  || checkComicSaved.value.viewMode !== "double"
  || checkComicSaved.value.direction !== "ltr"
  || checkComicSaved.value.pageGap !== "twenty_four"
  || checkComicSaved.value.preloadPages !== "five") {
  fail("comic.saved 必须覆盖漫画全局默认设置");
}
if (!checkGeneralSaved.revision || !checkAppearanceSaved.revision || !checkPlaybackSaved.revision || !checkReadingSaved.revision || !checkComicSaved.revision || !checkDownloadsSaved.revision) fail("saved fixture 必须携带 revision");
if (new Set([checkGeneralSaved.revision, checkAppearanceSaved.revision, checkPlaybackSaved.revision, checkReadingSaved.revision, checkComicSaved.revision, checkDownloadsSaved.revision]).size !== 6) fail("分区 revision 必须独立");
if (settingsValuesEqual(checkGeneralDefault.value, checkGeneralSaved.value)) {
  fail("saved 与 default 必须不同（已保存 ≠ 默认值）");
}
if (!checkUpdateChanged.changed) fail("changed fixture 必须 changed=true");
if (!checkUpdateChanged.revision) fail("changed fixture 必须携带 revision");
if (checkUpdateIdempotent.changed) fail("idempotent fixture 必须 changed=false");
if (checkUpdateIdempotent.revision !== checkUpdateChanged.revision) {
  fail("幂等结果必须返回当前 revision（与 changed fixture 同源）");
}
if (checkConflict.code !== "REVISION_CONFLICT" || checkConflict.retryable !== false) {
  fail("conflict fixture 必须 REVISION_CONFLICT 且不可盲目重试");
}
if (checkInvalidArgument.code !== "INVALID_ARGUMENT" || checkInvalidArgument.retryable !== false) {
  fail("invalid-argument fixture 必须 INVALID_ARGUMENT");
}
if (checkChangedEvent.schemaVersion !== 1 || checkChangedEvent.section !== "general" || checkChangedEvent.sequence !== 1) {
  fail("settings.changed 事件不变量");
}
if (checkChangedEvent.revision !== checkUpdateChanged.revision) {
  fail("settings.changed 必须与 SettingsUpdateResult 使用同一 revision（P1-8 同源）");
}

// ---- Secret 边界：fixture 与 DTO 均不得包含 secret 原文/字段（只允许 configured/status）----

const SECRET_KEY_PATTERN = /(apiKey|api_key|secret|token|password|credential|cookie)$/i;
function assertNoSecretKeys(value: unknown, path: string): void {
  if (Array.isArray(value)) {
    value.forEach((item, index) => assertNoSecretKeys(item, `${path}[${index}]`));
    return;
  }
  if (typeof value === "object" && value !== null) {
    for (const [key, item] of Object.entries(value as Record<string, unknown>)) {
      if (SECRET_KEY_PATTERN.test(key)) {
        fail(`${path}.${key} 疑似 secret 字段（Secret 不得进入设置 DTO / Fixture）`);
      }
      assertNoSecretKeys(item, `${path}.${key}`);
    }
  }
}

export const loadedSettingsFixtureSamples: readonly unknown[] = [
  settingsGeneralDefault,
  settingsAppearanceDefault,
  settingsPlaybackDefault,
  settingsReadingDefault,
  settingsComicDefault,
  settingsDownloadsDefault,
  settingsPrivacyDefault,
  settingsGeneralSaved,
  settingsAppearanceSaved,
  settingsPlaybackSaved,
  settingsReadingSaved,
  settingsComicSaved,
  settingsDownloadsSaved,
  settingsUpdateChanged,
  settingsUpdateIdempotent,
  settingsConflictError,
  settingsInvalidArgument,
  settingsChangedEvent,
];
for (const fixture of loadedSettingsFixtureSamples) assertNoSecretKeys(fixture, "fixture");

// ---- Mock 行为消费：契约样例用于 guard/基线，动态 CAS 结果由运行时语义断言 ----

export async function verifySettingsMockConsumption(): Promise<void> {
  // 1. 播种实例：get 与 saved fixture 同源（默认值加载的"已保存"形态）
  const mock = new MockHavenClient();
  const seededGeneral = await mock.settingsGet("general");
  if (!guardSettingsSnapshot(seededGeneral)) fail("Mock settingsGet 返回形状非法");
  if (seededGeneral.revision !== checkGeneralSaved.revision) fail("Mock get 必须与 saved fixture 同 revision");
  if (!deepEqual(seededGeneral.value, checkGeneralSaved.value)) fail("Mock get 必须与 saved fixture 同值");

  const seededPlayback = await mock.settingsGet("playback");
  if (!guardSettingsSnapshot(seededPlayback)) fail("Mock playback settingsGet 返回形状非法");
  if (seededPlayback.revision !== checkPlaybackSaved.revision) fail("Mock playback get 必须与 saved fixture 同 revision");
  if (!deepEqual(seededPlayback.value, checkPlaybackSaved.value)) fail("Mock playback get 必须与 saved fixture 同值");

  const seededReading = await mock.settingsGet("reading");
  if (!guardSettingsSnapshot(seededReading)) fail("Mock reading settingsGet 返回形状非法");
  if (seededReading.revision !== checkReadingSaved.revision) fail("Mock reading get 必须与 saved fixture 同 revision");
  if (!deepEqual(seededReading.value, checkReadingSaved.value)) fail("Mock reading get 必须与 saved fixture 同值");

  const seededComic = await mock.settingsGet("comic");
  if (!guardSettingsSnapshot(seededComic)) fail("Mock comic settingsGet 返回形状非法");
  if (seededComic.revision !== checkComicSaved.revision) fail("Mock comic get 必须与 saved fixture 同 revision");
  if (!deepEqual(seededComic.value, checkComicSaved.value)) fail("Mock comic get 必须与 saved fixture 同值");

  const seededDownloads = await mock.settingsGet("downloads");
  if (!guardSettingsSnapshot(seededDownloads)) fail("Mock downloads settingsGet 返回形状非法");
  if (seededDownloads.revision !== checkDownloadsSaved.revision) fail("Mock downloads get 必须与 saved fixture 同 revision");
  if (!deepEqual(seededDownloads.value, checkDownloadsSaved.value)) fail("Mock downloads get 必须与 saved fixture 同值");

  // 2. stale revision 冲突：expected 校验先于一切（即使提交相同值）
  const stalePatch = { section: "general" as const, launchPage: "home" as const };
  try {
    await mock.settingsUpdate({ section: "general", expectedRevision: "set-0000000000000000-0", patch: stalePatch });
    fail("过期 revision 必须 REVISION_CONFLICT");
  } catch (err) {
    if (!(err instanceof HavenError) || err.code !== "REVISION_CONFLICT") {
      fail("过期 revision 必须归一化为 HavenError(REVISION_CONFLICT)", err);
    }
  }
  const afterConflict = await mock.settingsGet("general");
  if (!guardSettingsSnapshot(afterConflict)) fail("冲突后的 Mock settingsGet 返回形状非法");
  if (afterConflict.revision !== checkGeneralSaved.revision) fail("冲突不得破坏状态（revision 不变）");
  const generalSavedValue = checkGeneralSaved.value;
  if (generalSavedValue.section !== "general") fail("fixture 前提：general saved");
  if (afterConflict.value.section !== "general") fail("冲突不得覆盖值");
  if (generalSavedValue.launchPage !== afterConflict.value.launchPage) {
    fail("冲突不得覆盖值");
  }

  // 3. 成功更新：changed=true + 新 revision + 事件同源
  const updated = await mock.settingsUpdate({
    section: "general",
    expectedRevision: seededGeneral.revision,
    patch: { section: "general", launchPage: "continue" },
  });
  if (!guardSettingsUpdateResult(updated)) fail("Mock settingsUpdate 返回形状非法");
  if (!updated.changed) fail("实际变化必须 changed=true");
  if (!updated.revision || updated.revision === seededGeneral.revision) fail("变化必须生成新 revision");
  if (updated.value.section !== "general") fail("值必须应用 patch（section）");
  if (updated.value.launchPage !== "continue") fail("值必须应用 patch（launchPage）");
  if (mock.settingsChangedEvents.length !== 1) fail("实际变化必须发布一次 settings.changed");
  const event = mock.settingsChangedEvents[0];
  if (!event || !guardSettingsChanged(event)) fail("Mock settings.changed 事件形状非法");
  if (
    event.schemaVersion !== 1 ||
    event.section !== "general" ||
    event.revision !== updated.revision ||
    event.sequence < 1 ||
    event.operationId.length === 0 ||
    event.at.length === 0
  ) {
    fail("settings.changed 事件必须与 Result revision 同源（P1-8）");
  }

  // 4. 重复更新幂等：changed=false + 同一 revision + 不产生新事件
  const repeated = await mock.settingsUpdate({
    section: "general",
    expectedRevision: updated.revision,
    patch: { section: "general", launchPage: "continue" },
  });
  if (!guardSettingsUpdateResult(repeated)) fail("Mock 幂等 settingsUpdate 返回形状非法");
  if (repeated.changed) fail("相同值重复更新不得视为变更");
  if (repeated.revision !== updated.revision) fail("幂等必须返回当前 revision");
  if (!settingsValuesEqual(repeated.value, updated.value)) fail("幂等结果必须返回同一 authoritative value");
  if (mock.settingsChangedEvents.length !== 1) fail("changed=false 不产生 changed 事件提示");

  // 5. 空 patch 幂等：不写状态、不发事件
  const emptyPatch = await mock.settingsUpdate({
    section: "general",
    expectedRevision: updated.revision,
    patch: { section: "general" },
  });
  if (emptyPatch.changed) fail("空 patch 必须幂等（changed=false）");
  if (emptyPatch.revision !== updated.revision) fail("空 patch 返回当前 revision");
  if (mock.settingsChangedEvents.length !== 1) fail("空 patch 不产生 changed 事件");

  // 6. 默认值加载 + 从未保存 CAS 边界（fresh 实例，未播种）
  const fresh = new MockHavenClient(false, { seedSettings: false });
  const freshGeneral = await fresh.settingsGet("general");
  if (freshGeneral.revision !== null) fail("fresh 实例 general 必须 revision=null");
  if (!deepEqual(freshGeneral, checkGeneralDefault)) fail("fresh 实例必须返回共享默认 fixture");
  const freshAppearance = await fresh.settingsGet("appearance");
  if (!deepEqual(freshAppearance, checkAppearanceDefault)) fail("fresh 实例 appearance 必须返回共享默认 fixture");
  const freshReading = await fresh.settingsGet("reading");
  if (!deepEqual(freshReading, checkReadingDefault)) fail("fresh 实例 reading 必须返回共享默认 fixture");
  const freshComic = await fresh.settingsGet("comic");
  if (!deepEqual(freshComic, checkComicDefault)) fail("fresh 实例 comic 必须返回共享默认 fixture");
  // 从未保存 + 非空 expected → 冲突
  try {
    await fresh.settingsUpdate({ section: "appearance", expectedRevision: "set-x", patch: { section: "appearance", theme: "dark" } });
    fail("从未保存 + 非空 expected 必须冲突");
  } catch (err) {
    if (!(err instanceof HavenError) || err.code !== "REVISION_CONFLICT") fail("从未保存 expected 冲突语义", err);
  }
  // 从未保存 + expected=null + 变化 → 成功
  const firstSave = await fresh.settingsUpdate({ section: "appearance", expectedRevision: null, patch: { section: "appearance", theme: "dark" } });
  if (!firstSave.changed || !firstSave.revision) fail("首次保存必须 changed=true 并携带 revision");
  // 已有行 + expected=null → 冲突（客户端无状态不能盲目提交）
  try {
    await fresh.settingsUpdate({ section: "appearance", expectedRevision: null, patch: { section: "appearance", density: "compact" } });
    fail("已有行 + expected=null 必须冲突");
  } catch (err) {
    if (!(err instanceof HavenError) || err.code !== "REVISION_CONFLICT") fail("已有行 expected=None 冲突语义", err);
  }

  // 7. validation-error：未知 section / patch 与 section 不一致 → INVALID_ARGUMENT
  try {
    await mock.settingsGet("bogus");
    fail("未知 section 必须 INVALID_ARGUMENT");
  } catch (err) {
    if (!(err instanceof HavenError) || err.code !== "INVALID_ARGUMENT") fail("未知 section 语义", err);
  }
  try {
    await mock.settingsUpdate({
      section: "general",
      expectedRevision: null,
      patch: { section: "appearance", theme: "dark" } as never,
    });
    fail("patch 与 section 不一致必须 INVALID_ARGUMENT");
  } catch (err) {
    if (!(err instanceof HavenError) || err.code !== "INVALID_ARGUMENT") fail("patch/section 不一致语义", err);
  }

  // 8. Secret 边界：Mock 返回的 DTO 无 secret 字段
  assertNoSecretKeys(seededGeneral, "mock.settingsGet");
  assertNoSecretKeys(updated, "mock.settingsUpdate");

  verifyFormStateMachine();
  await verifySettingsAsyncOperations();
}

// ---- Hook 生产路径异步操作回归（使用可控 SettingsGateway，不依赖 React 测试包）----

async function verifySettingsAsyncOperations(): Promise<void> {
  const ready = formLoaded(checkGeneralDefault);
  const dirty = formChange(ready, { section: "general", launchPage: "library" });
  const prepared = formSave(dirty);
  if (prepared.state.status !== "saving") fail("异步 save 测试前提必须是 saving");

  // 这些测试只验证 global Settings 表单；资源偏好由独立测试覆盖。
  // 显式提供 unavailable stub，避免为 unrelated settings scenarios 构造假资源数据。
  const unavailablePreferences = {
    preferenceGet: async () => { throw new HavenError({ code: "UNAVAILABLE", userMessage: "resource preferences unavailable", retryable: false }); },
    preferenceUpdate: async () => { throw new HavenError({ code: "UNAVAILABLE", userMessage: "resource preferences unavailable", retryable: false }); },
  };

  const successGateway: SettingsGateway = {
    ...unavailablePreferences,
    settingsGet: async () => checkGeneralDefault,
    settingsUpdate: async () => checkUpdateChanged,
  };
  const success = await runSettingsSaveOperation(successGateway, "general", prepared.state);
  if (success.state.status !== "ready" || !success.changed) fail("异步 save success 必须完成 saving → ready");

  const conflictGateway: SettingsGateway = {
    ...unavailablePreferences,
    settingsGet: async () => checkGeneralDefault,
    settingsUpdate: async () => { throw new HavenError(checkConflict); },
  };
  const conflict = await runSettingsSaveOperation(conflictGateway, "general", prepared.state);
  if (conflict.state.status !== "conflict") fail("异步 HavenError(REVISION_CONFLICT) 必须进入 conflict");

  const invalidGateway: SettingsGateway = {
    ...unavailablePreferences,
    settingsGet: async () => checkGeneralDefault,
    settingsUpdate: async () => { throw new HavenError(checkInvalidArgument); },
  };
  const invalid = await runSettingsSaveOperation(invalidGateway, "general", prepared.state);
  if (invalid.state.status !== "validation-error") fail("异步 HavenError(INVALID_ARGUMENT) 必须进入 validation-error");

  let retryCalls = 0;
  const retryGateway: SettingsGateway = {
    ...unavailablePreferences,
    settingsGet: async () => checkGeneralDefault,
    settingsUpdate: async () => {
      retryCalls += 1;
      if (retryCalls === 1) {
        throw new HavenError({ code: "DATABASE_ERROR", userMessage: "数据库错误", retryable: true });
      }
      return checkUpdateChanged;
    },
  };
  const firstAttempt = await runSettingsSaveOperation(retryGateway, "general", prepared.state);
  if (firstAttempt.state.status !== "save-error") fail("异步瞬时错误必须进入 save-error");
  const retryState = formRetrySave(firstAttempt.state);
  if (retryState.status !== "saving") fail("save-error retry 测试必须重新进入 saving");
  const retried = await runSettingsSaveOperation(retryGateway, "general", retryState);
  if (retried.state.status !== "ready" || !retried.changed || retryCalls !== 2) {
    fail("save-error retry 必须经同一异步 save helper 重试并成功");
  }

  let releaseRebase!: () => void;
  const pendingSnapshot = new Promise<SettingsSnapshot>((resolve) => { releaseRebase = () => resolve(checkGeneralSaved); });
  const pendingGateway: SettingsGateway = {
    ...unavailablePreferences,
    settingsGet: async () => pendingSnapshot,
    settingsUpdate: async () => checkUpdateChanged,
  };
  const pendingFetch = runSettingsRebaseFetch(pendingGateway, "general");
  const newerDraft = formChange(dirty, { section: "general", notifications: false });
  if (newerDraft.status !== "dirty") fail("pending rebase 测试必须产生最新 dirty 草稿");
  releaseRebase();
  const pendingResult = await pendingFetch;
  if (pendingResult.kind !== "success") fail("pending rebase fetch 必须成功");
  const rebasedLatest = formRebase(newerDraft, pendingResult.snapshot);
  if (rebasedLatest.status !== "dirty") fail("rebase 必须基于最新 state，不能覆盖 pending 期间的新草稿");
  if (rebasedLatest.draft.section !== "general" || rebasedLatest.draft.notifications !== false) {
    fail("rebase 必须基于最新 state，不能覆盖 pending 期间的新草稿");
  }

  const failingGateway: SettingsGateway = {
    ...unavailablePreferences,
    settingsGet: async () => { throw new HavenError(checkConflict); },
    settingsUpdate: async () => checkUpdateChanged,
  };
  const failedFetch = await runSettingsRebaseFetch(failingGateway, "general");
  if (failedFetch.kind !== "failure") fail("rebase fetch failure 必须返回可保留状态的失败结果");
  const conflictState = formSaveFailure(prepared.state, settingsFormErrorInfo(new HavenError(checkConflict)));
  if (conflictState.status !== "conflict") fail("rebase failure 测试前提必须是 conflict");
  if (failedFetch.kind === "failure") {
    const retryFailure = formRebaseFailed(conflictState, failedFetch.error.message);
    if (retryFailure.status !== "conflict") fail("rebase fetch failure 后再次重试必须保留 conflict 草稿");
    if (retryFailure.draft.section !== "general" || retryFailure.draft.launchPage !== "library") {
      fail("rebase fetch failure 后再次重试必须保留 conflict 草稿");
    }
    const failedAgain = await runSettingsRebaseFetch(failingGateway, "general");
    if (failedAgain.kind !== "failure") fail("rebase second retry failure 测试前提");
    const retryFailureAgain = formRebaseFailed(retryFailure, failedAgain.error.message);
    if (retryFailureAgain.status !== "conflict" || retryFailureAgain.draft.section !== "general"
      || retryFailureAgain.draft.launchPage !== "library") {
      fail("rebase fetch failure 后再次重试仍必须保留 conflict 草稿");
    }

    const validationState = formSaveFailure(prepared.state, settingsFormErrorInfo(new HavenError(checkInvalidArgument)));
    if (validationState.status !== "validation-error") fail("rebase validation 测试前提必须是 validation-error");
    const validationRetryFailure = formRebaseFailed(validationState, failedFetch.error.message);
    if (validationRetryFailure.status !== "validation-error") fail("rebase fetch failure 不得丢失 validation-error 状态");
    if (validationRetryFailure.draft.section !== "general" || validationRetryFailure.draft.launchPage !== "library") {
      fail("rebase fetch failure 后 validation 草稿必须保留");
    }

    const dirtyDuringRetry = formChange(conflictState, { section: "general", notifications: false });
    if (dirtyDuringRetry.status !== "dirty") fail("rebase failure 期间的最新编辑必须是 dirty");
    const dirtyAfterFailure = formRebaseFailed(dirtyDuringRetry, failedFetch.error.message);
    if (dirtyAfterFailure.status !== "dirty" || dirtyAfterFailure.draft.section !== "general" || dirtyAfterFailure.draft.notifications !== false) {
      fail("current 已变为 dirty 时 rebase failure 不得覆盖最新草稿");
    }
  }
}

// ---- 表单状态机（纯逻辑；loading/dirty/saved/validation-error/REVISION_CONFLICT/retry）----

function verifyFormStateMachine(): void {
  const section = "general" as const;
  const appearance = "appearance" as const;

  // loading → loaded → ready
  const loading = initialFormState(section);
  if (loading.status !== "loading") fail("初始状态必须 loading");
  const ready = formLoaded(checkGeneralDefault);
  if (ready.status !== "ready") fail("加载成功必须进入 ready");
  if (ready.revision !== null) fail("默认快照 revision=null");

  // 相同值修改 → 不进入 dirty（不显示"已修改"）
  const same = formChange(ready, { section: "general", launchPage: "home" });
  if (same.status !== "ready") fail("相同值修改不得进入 dirty");

  // 真实修改 → dirty；改回相同值 → 清除 dirty
  const dirty = formChange(ready, { section: "general", launchPage: "library" });
  if (dirty.status !== "dirty") fail("真实修改必须进入 dirty");
  const back = formChange(dirty, { section: "general", launchPage: "home" });
  if (back.status !== "ready") fail("改回相同值必须清除 dirty（不制造假保存）");

  // buildPatch：只含变化字段；无变化 → null（空 patch 不提交）
  if (dirty.status !== "dirty") fail("dirty 状态前提");
  const patch = buildSettingsPatch(dirty.saved, dirty.draft);
  if (!patch || patch.section !== "general" || patch.launchPage !== "library") fail("patch 必须只含变化字段");
  if (Object.keys(patch).length !== 2) fail("patch 不得包含未变化字段");
  if (buildSettingsPatch(checkGeneralDefault.value, checkGeneralDefault.value) !== null) {
    fail("相同值必须产生空 patch（null）");
  }

  // ready 上 save → 无操作（不制造假保存状态）
  const noOp = formSave(ready);
  if (noOp.patch !== null || noOp.state.status !== "ready") fail("ready 上 save 必须为空操作");

  // dirty 上 save → saving + patch
  const prepared = formSave(dirty);
  if (prepared.patch === null || prepared.state.status !== "saving") fail("dirty 上 save 必须进入 saving");
  if (prepared.patch.section !== "general") fail("saving patch 前提");
  if (prepared.patch.launchPage !== "library") fail("saving 必须携带变化 patch");

  // saving 期间 reset 不得改写 pending operation 的 draft/patch/revision。
  const resetWhileSaving = formResetToDefaults(prepared.state, section);
  if (resetWhileSaving.status !== "saving" || resetWhileSaving !== prepared.state) {
    fail("saving 期间 resetToDefaults 必须 no-op，保留同一 saving state");
  }
  if (resetWhileSaving.status === "saving" && (
    resetWhileSaving.revision !== prepared.state.revision ||
    resetWhileSaving.patch !== prepared.state.patch ||
    resetWhileSaving.draft !== prepared.state.draft
  )) {
    fail("saving 期间 resetToDefaults 不得改变 patch/draft/revision");
  }

  // saveSuccess changed=true → ready + 提示标志
  const saved = formSaveSuccess(prepared.state, checkUpdateChanged);
  if (saved.state.status !== "ready") fail("保存成功必须回到 ready");
  if (!saved.changed) fail("changed=true 必须产生提示标志");
  if (saved.state.revision !== checkUpdateChanged.revision) fail("保存成功必须采用结果 revision");

  // changed=false（幂等）→ 不提示
  const idempotent = formSaveSuccess(prepared.state, checkUpdateIdempotent);
  if (idempotent.changed) fail("changed=false 不得产生提示标志");

  // REVISION_CONFLICT → conflict；retry（rebase）语义
  const conflictInfo = settingsFormErrorInfo(new HavenError(checkConflict));
  if (conflictInfo.code !== "REVISION_CONFLICT") fail("Hook 错误适配不得把 REVISION_CONFLICT 降级");
  const conflict = formSaveFailure(prepared.state, conflictInfo);
  if (conflict.status !== "conflict") fail("REVISION_CONFLICT 必须进入 conflict");
  const conflictBase = conflict as Extract<SettingsFormState, { status: "conflict" }>;
  if (conflictBase.message.length === 0) fail("conflict 必须携带可展示文案");
  // 其他窗口已保存相同草稿 → rebase 后 ready（草稿与新快照一致）
  const rebasedSame = formRebase(conflictBase, { value: conflictBase.draft, revision: "set-fresh-1" });
  if (rebasedSame.status !== "ready" || rebasedSame.revision !== "set-fresh-1") fail("rebase 相同值必须回到 ready");
  // 其他窗口保存了不同值 → rebase 后保持 dirty（草稿保留，以新 revision 重新比对）
  const otherValue: SettingsValue = { section: "general", launchPage: "last_session", restoreSession: false, language: "zh_cn", notifications: true };
  const rebasedDiff = formRebase(conflictBase, { value: otherValue, revision: "set-fresh-2" });
  if (rebasedDiff.status !== "dirty") fail("rebase 不同值必须保持 dirty");
  if (rebasedDiff.status === "dirty" && rebasedDiff.draft.section === "general" && rebasedDiff.draft.launchPage !== "library") {
    fail("rebase 必须保留草稿");
  }

  // INVALID_ARGUMENT → validation-error
  const invalidInfo = settingsFormErrorInfo(new HavenError(checkInvalidArgument));
  if (invalidInfo.code !== "INVALID_ARGUMENT") fail("Hook 错误适配不得把 INVALID_ARGUMENT 降级");
  const invalid = formSaveFailure(prepared.state, invalidInfo);
  if (invalid.status !== "validation-error") fail("INVALID_ARGUMENT 必须进入 validation-error");

  // 其他失败（瞬时错误）→ save-error + retryable 语义 + patch 保留（可重试同一 patch）
  const transient = formSaveFailure(prepared.state, { code: "DATABASE_ERROR", message: "数据库错误", retryable: true });
  if (transient.status !== "save-error") fail("瞬时错误必须进入 save-error");
  if (transient.status === "save-error" && (!transient.retryable || transient.patch.section !== "general" || transient.patch.launchPage !== "library")) {
    fail("save-error 必须保留 retryable 与 patch（retry = 重试同一 patch）");
  }

  // save-error retry：携带同一 patch 重新进入 saving → 成功后 ready + 提示标志
  const retried = formRetrySave(transient);
  if (retried.status !== "saving") fail("save-error retry 必须重新进入 saving");
  if (retried.status === "saving" && (retried.patch.section !== "general" || retried.patch.launchPage !== "library")) {
    fail("retry 必须复用同一 patch");
  }
  const retriedSaved = formSaveSuccess(retried, checkUpdateChanged);
  if (retriedSaved.state.status !== "ready") fail("retry 成功必须回到 ready");
  if (!retriedSaved.changed) fail("retry 成功（changed=true）必须产生提示标志");
  if (retriedSaved.state.status === "ready" && retriedSaved.state.revision !== checkUpdateChanged.revision) {
    fail("retry 成功必须采用结果 revision");
  }

  // resetToDefaults：默认值与已保存不同 → dirty；patch 覆盖全部字段
  const appReady = formLoaded(checkAppearanceSaved);
  if (appReady.status !== "ready") fail("appearance 加载前提");
  const appReset = formResetToDefaults(appReady, appearance);
  if (appReset.status !== "dirty") fail("恢复默认（与已保存不同）必须进入 dirty");
  if (appReset.status === "dirty") {
    const resetPatch = buildSettingsPatch(appReady.saved, appReset.draft);
    if (!resetPatch || resetPatch.section !== "appearance") fail("恢复默认必须产生 patch");
    if (resetPatch.theme !== "system" || resetPatch.density !== "comfortable"
      || resetPatch.sidebar !== "auto" || resetPatch.reduceMotion !== false) {
      fail("恢复默认 patch 必须覆盖全部字段到契约默认值");
    }
  }
}
