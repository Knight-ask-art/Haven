import { invoke } from "@tauri-apps/api/core";
import { check, type Update, type DownloadEvent } from "@tauri-apps/plugin-updater";
import type { UpdaterCheckResult, UpdaterInstallResult, UpdaterProgress } from "./client";
import { HavenError, toHavenError } from "./errors";

const CHECK_TIMEOUT = 15_000;
const DOWNLOAD_TIMEOUT = 120_000;

function safeUpdateText(value: string | undefined): string | null {
  if (!value) return null;
  const text = Array.from(value, (character) => {
    const code = character.codePointAt(0) ?? 0;
    return code <= 0x1f || code === 0x7f ? " " : character;
  }).join("").trim();
  return text ? text.slice(0, 2000) : null;
}

function updaterError(cause: unknown, phase: "check" | "download" | "install"): HavenError {
  if (cause instanceof HavenError) return cause;
  // The official plugin serializes errors as strings, not a tagged DTO. Do not
  // return or log that string: it can contain transport URLs and query secrets.
  const message = typeof cause === "string" ? cause : cause instanceof Error ? cause.message : "";
  if (phase === "download" && /signature|public key|key id/i.test(message)) {
    return new HavenError({ code: "UPDATER_SIGNATURE_INVALID", userMessage: "更新包签名校验失败，已阻止安装。请重新检查更新或联系维护者。", retryable: false });
  }
  const errors = {
    check: { code: "UPDATER_CHECK_FAILED", userMessage: "无法获取有效的更新信息。请检查网络后重试。", retryable: true },
    download: { code: "UPDATER_DOWNLOAD_FAILED", userMessage: "更新包下载或校验未完成，尚未安装任何更新。可以重试。", retryable: true },
    install: { code: "UPDATER_INSTALL_FAILED", userMessage: "未能启动更新安装。当前版本仍可使用，可以重试。", retryable: true },
  };
  return new HavenError(errors[phase]);
}

/** The only owner of native updater handles. Startup and Settings share it. */
export class TauriUpdaterClient {
  private pending: Update | null = null;
  private downloaded = false;
  private checking: Promise<UpdaterCheckResult> | null = null;
  private installing: Promise<UpdaterInstallResult> | null = null;

  check(): Promise<UpdaterCheckResult> {
    if (this.installing) return Promise.reject(new HavenError({ code: "UPDATER_BUSY", userMessage: "更新正在安装，请稍候。", retryable: true }));
    if (this.checking) return this.checking;
    this.checking = this.performCheck().finally(() => { this.checking = null; });
    return this.checking;
  }

  private async performCheck(): Promise<UpdaterCheckResult> {
    let next: Update | null;
    try {
      next = await check({ timeout: CHECK_TIMEOUT });
    } catch (cause) {
      // A transport failure must not discard the last verified candidate.
      throw updaterError(cause, "check");
    }
    const previous = this.pending;
    this.pending = next;
    this.downloaded = false;
    if (previous) await previous.close().catch(() => undefined);
    return next ? {
      status: "available", currentVersion: next.currentVersion, availableVersion: next.version,
      releaseNotes: safeUpdateText(next.body), publishedAt: safeUpdateText(next.date),
    } : { status: "up_to_date", currentVersion: null, availableVersion: null, releaseNotes: null, publishedAt: null };
  }

  install(onProgress?: (progress: UpdaterProgress) => void): Promise<UpdaterInstallResult> {
    if (this.installing) return this.installing;
    if (this.checking) return Promise.reject(new HavenError({ code: "UPDATER_BUSY", userMessage: "正在检查更新，请稍候。", retryable: true }));
    const update = this.pending;
    if (!update) return Promise.reject(new HavenError({ code: "UPDATER_NO_UPDATE", userMessage: "没有可安装的更新，请先检查更新。", retryable: true }));
    this.installing = this.performInstall(update, onProgress).finally(() => { this.installing = null; });
    return this.installing;
  }

  private async performInstall(update: Update, onProgress?: (progress: UpdaterProgress) => void): Promise<UpdaterInstallResult> {
    let downloadedBytes = 0;
    let totalBytes: number | null = null;
    const report = (phase: UpdaterProgress["phase"]) => {
      // A presentation callback must never interrupt signature verification or
      // turn a completed native installation into a transport failure.
      try { onProgress?.({ phase, downloadedBytes, totalBytes }); } catch { /* presentation only */ }
    };
    if (!this.downloaded) {
      report("downloading");
      try {
        await update.download((event: DownloadEvent) => {
          if (event.event === "Started") totalBytes = event.data.contentLength ?? null;
          if (event.event === "Progress") downloadedBytes += event.data.chunkLength;
          // Finished means bytes received, NOT signature verified. Only a
          // resolved download() authorizes the preparation/install step below.
          report(event.event === "Finished" ? "verifying" : "downloading");
        }, { timeout: DOWNLOAD_TIMEOUT });
        this.downloaded = true;
      } catch (cause) {
        throw updaterError(cause, "download");
      }
    }
    report("installing");
    let prepared = false;
    try {
      // Drain the active window's sessions before the plugin's Windows exit.
      await invoke<void>("app_update_prepare").catch((cause: unknown) => { throw toHavenError(cause); });
      prepared = true;
      await update.install({ restartAfterInstall: true });
    } catch (cause) {
      if (prepared) await this.releasePreparation();
      // Verified bytes stay with this handle for a retry; no duplicate download
      // or orphan native DownloadedBytes resource on installer failure.
      throw updaterError(cause, "install");
    }
    // Windows exits inside install(). Platforms which return must release the
    // guard too; only the native failure/success handoff can reopen sessions.
    await this.releasePreparation();
    this.pending = null;
    this.downloaded = false;
    await update.close().catch(() => undefined);
    return { status: "installed" };
  }

  private async releasePreparation(): Promise<void> {
    try { await invoke<void>("app_update_cancel"); }
    catch { throw new HavenError({ code: "UPDATER_RECOVERY_FAILED", userMessage: "更新安装交接未完成，请退出并重新打开应用后重试。", retryable: false }); }
  }
}
