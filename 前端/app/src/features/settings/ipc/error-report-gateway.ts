// 错误诊断报告 Gateway（V02-OPEN-SOURCE-DIAGNOSTICS-001）。
// 页面只选择报告等级并消费脱敏 DTO；固定 URL、报告内容和导出目录都由后端拥有。

import { toHavenError } from "@/lib/ipc/errors";
import { getHavenClient } from "@/lib/ipc/runtime";
import type {
  ErrorReportActionRequest,
  ErrorReportActionResultDto,
  ErrorReportConfirmRequest,
  ErrorReportConfirmResultDto,
  ErrorReportLevelDto,
  ErrorReportPreviewDto,
  ErrorReportPreviewRequest,
} from "@/lib/ipc/generated/wire";

export interface ErrorReportGateway {
  preview(request: ErrorReportPreviewRequest): Promise<ErrorReportPreviewDto>;
  confirm(request: ErrorReportConfirmRequest): Promise<ErrorReportConfirmResultDto>;
  export(request: ErrorReportActionRequest): Promise<ErrorReportActionResultDto>;
  openIssue(request: ErrorReportActionRequest): Promise<ErrorReportActionResultDto>;
}

export const errorReportGateway: ErrorReportGateway = {
  async preview(request) {
    try {
      return await getHavenClient().errorReportPreviewGet(request);
    } catch (error) {
      throw toHavenError(error);
    }
  },
  async confirm(request) {
    try {
      return await getHavenClient().errorReportConfirm(request);
    } catch (error) {
      throw toHavenError(error);
    }
  },
  async export(request) {
    try {
      return await getHavenClient().errorReportExport(request);
    } catch (error) {
      throw toHavenError(error);
    }
  },
  async openIssue(request) {
    try {
      return await getHavenClient().errorReportOpenIssue(request);
    } catch (error) {
      throw toHavenError(error);
    }
  },
};

export const ERROR_REPORT_LEVEL_LABELS: Record<ErrorReportLevelDto, string> = {
  basic: "基础",
  standard: "标准",
  detailed: "详细",
};
