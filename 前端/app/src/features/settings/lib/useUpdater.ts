import { useCallback, useRef, useState } from "react";
import type { HavenError } from "@/lib/ipc/errors";
import type { UpdaterCheckResult } from "@/lib/ipc/client";
import { updaterGateway } from "../ipc/updater-gateway";

export type UpdaterStatus = "idle" | "checking" | "up_to_date" | "available" | "installing" | "error";

export interface UpdaterState {
  status: UpdaterStatus;
  result: UpdaterCheckResult | null;
  error: HavenError | null;
  check: () => Promise<boolean>;
  install: () => Promise<boolean>;
}

/**
 * Updates 页面局部状态机。安装动作由官方插件接管，Windows 进程会在
 * 校验签名后退出并启动安装器；页面只负责展示可解释状态和可重试错误。
 */
export function useUpdater(): UpdaterState {
  const [status, setStatus] = useState<UpdaterStatus>("idle");
  const [result, setResult] = useState<UpdaterCheckResult | null>(null);
  const [error, setError] = useState<HavenError | null>(null);
  const requestId = useRef(0);

  const check = useCallback(async () => {
    const id = ++requestId.current;
    setStatus("checking");
    setError(null);
    try {
      const next = await updaterGateway.check();
      if (id !== requestId.current) return false;
      setResult(next);
      setStatus(next.status);
      return true;
    } catch (cause) {
      if (id !== requestId.current) return false;
      setError(cause as HavenError);
      setStatus("error");
      return false;
    }
  }, []);

  const install = useCallback(async () => {
    if (!result || result.status !== "available") return false;
    const id = ++requestId.current;
    setStatus("installing");
    setError(null);
    try {
      await updaterGateway.install();
      if (id !== requestId.current) return false;
      setStatus("up_to_date");
      return true;
    } catch (cause) {
      if (id !== requestId.current) return false;
      setError(cause as HavenError);
      setStatus("error");
      return false;
    }
  }, [result]);

  return { status, result, error, check, install };
}
