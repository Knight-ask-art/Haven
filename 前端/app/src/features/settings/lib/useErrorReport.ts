import { useCallback, useRef, useState } from "react";
import type {
  ErrorReportActionResultDto,
  ErrorReportLevelDto,
  ErrorReportPreviewDto,
} from "@/lib/ipc/generated/wire";
import type { HavenError } from "@/lib/ipc/errors";
import { errorReportGateway } from "../ipc/error-report-gateway";

export type ErrorReportAction = "export" | "issue" | null;

export interface ErrorReportState {
  level: ErrorReportLevelDto;
  preview: ErrorReportPreviewDto | null;
  confirmed: boolean;
  loading: boolean;
  action: ErrorReportAction;
  error: HavenError | null;
  actionResult: ErrorReportActionResultDto | null;
  setLevel: (level: ErrorReportLevelDto) => void;
  generate: (stableErrorCodes?: string[]) => Promise<boolean>;
  confirm: () => Promise<boolean>;
  exportReport: () => Promise<boolean>;
  openIssue: () => Promise<boolean>;
  clearError: () => void;
}

/** Settings About 内的诊断报告工作流；请求 token 防止旧预览覆盖新等级。 */
export function useErrorReport(): ErrorReportState {
  const [level, setLevel] = useState<ErrorReportLevelDto>("standard");
  const [preview, setPreview] = useState<ErrorReportPreviewDto | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [loading, setLoading] = useState(false);
  const [action, setAction] = useState<ErrorReportAction>(null);
  const [error, setError] = useState<HavenError | null>(null);
  const [actionResult, setActionResult] = useState<ErrorReportActionResultDto | null>(null);
  const requestId = useRef(0);

  const changeLevel = useCallback((nextLevel: ErrorReportLevelDto) => {
    // Changing the level invalidates an in-flight preview and any confirmation
    // attached to the previous report. This prevents a late response from
    // reintroducing a report for a selection the user has already changed.
    requestId.current += 1;
    setLevel(nextLevel);
    setPreview(null);
    setConfirmed(false);
    setActionResult(null);
    setError(null);
    setLoading(false);
  }, []);

  const generate = useCallback(async (stableErrorCodes: string[] = []) => {
    const id = ++requestId.current;
    setLoading(true);
    setError(null);
    setActionResult(null);
    setConfirmed(false);
    try {
      const next = await errorReportGateway.preview({ level, stableErrorCodes });
      if (id !== requestId.current) return false;
      setPreview(next);
      return true;
    } catch (cause) {
      if (id !== requestId.current) return false;
      setError(cause as HavenError);
      return false;
    } finally {
      if (id === requestId.current) setLoading(false);
    }
  }, [level]);

  const confirm = useCallback(async () => {
    if (!preview) return false;
    setError(null);
    try {
      const result = await errorReportGateway.confirm({ reportId: preview.reportId });
      setConfirmed(result.confirmed);
      return result.confirmed;
    } catch (cause) {
      setError(cause as HavenError);
      return false;
    }
  }, [preview]);

  const runAction = useCallback(async (nextAction: Exclude<ErrorReportAction, null>) => {
    if (!preview || !confirmed) return false;
    setAction(nextAction);
    setError(null);
    try {
      const result = nextAction === "export"
        ? await errorReportGateway.export({ reportId: preview.reportId })
        : await errorReportGateway.openIssue({ reportId: preview.reportId });
      setActionResult(result);
      return true;
    } catch (cause) {
      setError(cause as HavenError);
      return false;
    } finally {
      setAction(null);
    }
  }, [confirmed, preview]);

  const clearError = useCallback(() => setError(null), []);

  return {
    level,
    preview,
    confirmed,
    loading,
    action,
    error,
    actionResult,
    setLevel: changeLevel,
    generate,
    confirm,
    exportReport: () => runAction("export"),
    openIssue: () => runAction("issue"),
    clearError,
  };
}
