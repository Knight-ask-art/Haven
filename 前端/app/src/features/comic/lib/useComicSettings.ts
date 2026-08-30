// Comic Reader 的全局默认设置读取。
// 设置是 Rust + SQLite 的事实源；Reader 只在新 Session 建立时消费默认投影。

import { useEffect, useState } from "react";
import type { ComicSettingsValue } from "@/lib/ipc/settings-wire";
import { defaultSettingsValue } from "@/lib/ipc/settings-wire";
import { toHavenError } from "@/lib/ipc/errors";
import { comicSettingsGateway, getComicSettingsForResource } from "../ipc/comic-settings-gateway";

export type ComicSettingsState =
  | { status: "loading"; value: ComicSettingsValue; error: null; scopeKey: string }
  | { status: "ready"; value: ComicSettingsValue; error: null; scopeKey: string }
  | { status: "degraded"; value: ComicSettingsValue; error: string; scopeKey: string };

function defaultComicSettings(): ComicSettingsValue {
  const value = defaultSettingsValue("comic");
  if (value.section === "comic") return value;
  return { section: "comic", viewMode: "single", direction: "rtl", pageGap: "twelve", preloadPages: "three" };
}

function resourceScopeKey(mediaItemId?: string, editionId?: string): string {
  const media = mediaItemId?.trim();
  const edition = editionId?.trim();
  return media && edition ? `resource:${media}:${edition}` : "global";
}

export function useComicSettings(mediaItemId?: string, editionId?: string): ComicSettingsState {
  const scopeKey = resourceScopeKey(mediaItemId, editionId);
  const [state, setState] = useState<ComicSettingsState>(() => ({
    status: "loading",
    value: defaultComicSettings(),
    error: null,
    scopeKey: "global",
  }));

  useEffect(() => {
    let active = true;
    setState({ status: "loading", value: defaultComicSettings(), error: null, scopeKey });
    const load = scopeKey === "global"
      ? comicSettingsGateway.settingsGet().then((snapshot) => snapshot.value)
      : mediaItemId && editionId
        ? getComicSettingsForResource(mediaItemId, editionId).then((snapshot) => snapshot.effectiveComic)
        : Promise.resolve(defaultComicSettings());
    void load
      .then((value) => {
        if (!active) return;
        if (value.section !== "comic") {
          setState({ status: "degraded", value: defaultComicSettings(), error: "漫画设置分区数据无效，已使用安全默认值", scopeKey });
          return;
        }
        setState({ status: "ready", value, error: null, scopeKey });
      })
      .catch((error: unknown) => {
        if (!active) return;
        const normalized = toHavenError(error);
        setState({ status: "degraded", value: defaultComicSettings(), error: normalized.message, scopeKey });
      });
    return () => {
      active = false;
    };
  }, [editionId, mediaItemId, scopeKey]);

  return state.scopeKey === scopeKey
    ? state
    : { status: "loading", value: defaultComicSettings(), error: null, scopeKey };
}
