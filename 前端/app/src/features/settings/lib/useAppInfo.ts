import { useCallback, useEffect, useRef, useState } from "react";
import type { AppDirectoryKindDto, AppInfoDto } from "@/lib/ipc/generated/wire";
import type { HavenError } from "@/lib/ipc/errors";
import { appInfoGateway } from "@/features/settings/ipc/app-info-gateway";

export interface AppInfoState {
  info: AppInfoDto | null;
  loading: boolean;
  error: HavenError | null;
  opening: AppDirectoryKindDto | null;
  openError: HavenError | null;
  reload: () => Promise<void>;
  openDirectory: (kind: AppDirectoryKindDto) => Promise<boolean>;
}
/** About 页面私有查询；响应不会在卸载或新请求后覆盖当前状态。 */
export function useAppInfo(): AppInfoState {
  const [info, setInfo] = useState<AppInfoDto | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<HavenError | null>(null);
  const [opening, setOpening] = useState<AppDirectoryKindDto | null>(null);
  const [openError, setOpenError] = useState<HavenError | null>(null);
  const requestId = useRef(0);

  const reload = useCallback(async () => {
    const id = ++requestId.current;
    setLoading(true);
    setError(null);
    try {
      const next = await appInfoGateway.get();
      if (id !== requestId.current) return;
      setInfo(next);
    } catch (cause) {
      if (id !== requestId.current) return;
      const nextError = cause as HavenError;
      setError(nextError);
    } finally {
      if (id === requestId.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    void reload();
    return () => {
      requestId.current += 1;
    };
  }, [reload]);

  const openDirectory = useCallback(async (kind: AppDirectoryKindDto) => {
    setOpening(kind);
    setOpenError(null);
    try {
      await appInfoGateway.openDirectory(kind);
      return true;
    } catch (cause) {
      setOpenError(cause as HavenError);
      return false;
    } finally {
      setOpening(null);
    }
  }, []);

  return { info, loading, error, opening, openError, reload, openDirectory };
}
