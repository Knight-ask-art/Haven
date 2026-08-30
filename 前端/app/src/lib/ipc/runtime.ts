// Client 运行时选择（IPC-FE-001：Mock/Tauri 可替换的注入边界）。
// - Tauri WebView 内 → TauriHavenClient（真实 IPC）
// - 浏览器 dev（vite）→ MockHavenClient（共享 Fixture，保持既有 DX 与 fixtures:check）
// - 浏览器 Mock 仅在开发模式或显式开关启用；生产浏览器默认 fail closed

import type { HavenClient } from "./client";
import { HavenError } from "./errors.js";
import { MockHavenClient } from "./mock-client.js";
import { TauriHavenClient } from "./tauri-client.js";

interface HavenImportMeta {
  readonly env?: {
    readonly DEV?: boolean;
    readonly VITE_HAVEN_CLIENT?: string;
    readonly VITE_HAVEN_ENABLE_MOCK_IPC?: string;
  };
}

/** Tauri WebView 注入的内部全局对象（官方判定方式；与 withGlobalTauri 无关）。 */
export function resolveTauriRuntime(
  detected: boolean,
  forced: string | undefined = (import.meta as unknown as HavenImportMeta).env?.VITE_HAVEN_CLIENT,
): boolean {
  if (detected) return true;
  if (forced === "tauri") return true;
  if (forced === "mock") return false;
  return false;
}

/** Tauri WebView 注入的内部全局对象（官方判定方式；与 withGlobalTauri 无关）。 */
export function isTauriRuntime(): boolean {
  const detected = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  return resolveTauriRuntime(detected);
}

export type HavenClientMode = "tauri" | "mock" | "unavailable";

/** 纯运行时选择：浏览器生产环境除非显式启用，否则拒绝伪造 IPC 数据。 */
export function selectHavenClientMode(input: {
  tauri: boolean;
  dev: boolean;
  mockEnabled: boolean;
}): HavenClientMode {
  if (input.tauri) return "tauri";
  if (input.dev || input.mockEnabled) return "mock";
  return "unavailable";
}

/** 返回当前页面应使用的客户端模式，供 Feature 明确区分 Demo 与生产路径。 */
export function getHavenClientMode(): HavenClientMode {
  const env = (import.meta as unknown as HavenImportMeta).env;
  return selectHavenClientMode({
    tauri: isTauriRuntime(),
    dev: env?.DEV === true,
    mockEnabled: env?.VITE_HAVEN_ENABLE_MOCK_IPC === "true"
      || env?.VITE_HAVEN_CLIENT === "mock",
  });
}

let cached: HavenClient | null = null;

/** 进程内单例：同一环境恒定返回同一实现，避免页面间 Mock 状态分裂。 */
export function getHavenClient(): HavenClient {
  if (cached) return cached;
  const mode = getHavenClientMode();
  if (mode === "unavailable") {
    throw new HavenError({
      code: "IPC_UNAVAILABLE",
      userMessage: "当前运行环境不支持应用数据访问",
      retryable: false,
    });
  }
  cached = mode === "tauri" ? new TauriHavenClient() : new MockHavenClient();
  return cached;
}
