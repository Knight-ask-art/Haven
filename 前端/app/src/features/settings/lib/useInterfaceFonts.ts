// 界面字体的 React 接线（BE-INTERFACE-FONT-001）。
//
// 页面不直接调用 invoke/Channel/Repository：全部经 `interfaceFontGateway`。
// 导入/删除后的资产列表同时写回设置运行时投影，保证 AppShell（真正应用字体的一方）
// 与设置页看到的是同一份列表。

import { useCallback, useEffect, useRef, useState } from "react";
import type { InterfaceFontAsset, InterfaceFontFamily } from "../../../lib/ipc/interface-font-wire.js";
import { toHavenError } from "../../../lib/ipc/errors.js";
import { interfaceFontGateway } from "../ipc/interface-font-gateway.js";
import { publishInterfaceFontAssets } from "./settings-runtime-state.js";

/**
 * 把一段 `@font-face` 文本注入到文档（同一 id 只有一个元素）。
 *
 * 预览必须能渲染**尚未保存**的草稿字体，所以设置页需要自己注入一条；
 * AppShell 注入的是已保存值的那条。两者用不同 id，互不覆盖。
 */
export function useInjectedFontFace(elementId: string, css: string | null): void {
  useEffect(() => {
    const existing = document.getElementById(elementId);
    if (css === null) {
      existing?.remove();
      return;
    }
    const style = existing instanceof HTMLStyleElement ? existing : document.createElement("style");
    if (!(existing instanceof HTMLStyleElement)) {
      style.id = elementId;
      document.head.appendChild(style);
    }
    // 只在内容真的变化时改写：重复赋值会让 WebView 重新解析 @font-face，
    // 造成预览字体短暂回退。
    if (style.textContent !== css) style.textContent = css;
  }, [elementId, css]);

  useEffect(
    () => () => {
      document.getElementById(elementId)?.remove();
    },
    [elementId],
  );
}

export type InterfaceFontAssetsState = {
  assets: InterfaceFontAsset[];
  /** 读取失败时为稳定的用户可读原因；成功为 null。 */
  error: string | null;
  /** 重新读取（导入/删除后调用）。 */
  reload: () => Promise<void>;
  /** 直接替换（已经在手的列表，例如 import 的返回值），避免多一次往返。 */
  replace: (assets: InterfaceFontAsset[]) => void;
  status: "loading" | "ready" | "error";
};

/**
 * 已导入字体资产列表。
 *
 * 读取失败不伪造空列表为「成功」：状态区分 loading/ready/error，
 * 页面据此显示重试入口，而不是把「读不到」显示成「一个都没有」。
 */
export function useInterfaceFontAssets(): InterfaceFontAssetsState {
  const [assets, setAssets] = useState<InterfaceFontAsset[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<"loading" | "ready" | "error">("loading");
  const requestEpoch = useRef(0);

  const apply = useCallback((next: InterfaceFontAsset[]) => {
    setAssets(next);
    setError(null);
    setStatus("ready");
    publishInterfaceFontAssets(next);
  }, []);

  const replace = useCallback(
    (next: InterfaceFontAsset[]) => {
      // 导入/删除的成功响应比旧读取更晚时，旧响应不得覆盖新列表。
      requestEpoch.current += 1;
      apply(next);
    },
    [apply],
  );

  const reload = useCallback(async () => {
    const epoch = ++requestEpoch.current;
    setError(null);
    setStatus("loading");
    try {
      const next = await interfaceFontGateway.assetList();
      if (epoch !== requestEpoch.current) return;
      apply(next);
    } catch (cause) {
      if (epoch !== requestEpoch.current) return;
      const haven = toHavenError(cause);
      setError(haven.message);
      setStatus("error");
    }
  }, [apply]);

  useEffect(() => {
    void reload();
    return () => {
      requestEpoch.current += 1;
    };
  }, [reload]);

  return { assets, error, reload, replace, status };
}

export type SystemFontFamiliesState = {
  families: InterfaceFontFamily[];
  status: "idle" | "loading" | "ready" | "error";
  error: string | null;
  reload: () => Promise<void>;
};

/**
 * 本机字体族列表。
 *
 * `enabled` 为 false 时不发请求（默认卡片不需要它）；枚举本身在 Rust 侧记忆化，
 * 因此重复挂载不会反复扫描字体目录。
 */
export function useSystemFontFamilies(enabled: boolean): SystemFontFamiliesState {
  const [families, setFamilies] = useState<InterfaceFontFamily[]>([]);
  const [status, setStatus] = useState<SystemFontFamiliesState["status"]>("idle");
  const [error, setError] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);

  useEffect(() => {
    if (!enabled) return;
    let active = true;
    setStatus("loading");
    void interfaceFontGateway
      .systemList()
      .then((list) => {
        if (!active) return;
        setFamilies(list);
        setError(null);
        setStatus("ready");
      })
      .catch((cause: unknown) => {
        if (!active) return;
        setError(toHavenError(cause).message);
        setStatus("error");
      });
    return () => {
      active = false;
    };
  }, [enabled, reloadToken]);

  const reload = useCallback(async () => {
    setReloadToken((token) => token + 1);
  }, []);

  return { families, status, error, reload };
}
