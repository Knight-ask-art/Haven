// Updater Gateway（V02-UPDATE-FOUNDATION-001）。
// Settings 只消费脱敏状态；签名校验、下载和安装由 Tauri 官方插件完成。

import { toHavenError } from "@/lib/ipc/errors";
import { getHavenClient } from "@/lib/ipc/runtime";
import type { UpdaterCheckResult, UpdaterInstallResult } from "@/lib/ipc/client";

export interface UpdaterGateway {
  check(): Promise<UpdaterCheckResult>;
  install(): Promise<UpdaterInstallResult>;
}

export const updaterGateway: UpdaterGateway = {
  async check() {
    try {
      return await getHavenClient().updateCheck();
    } catch (error) {
      throw toHavenError(error);
    }
  },
  async install() {
    try {
      return await getHavenClient().updateInstall();
    } catch (error) {
      throw toHavenError(error);
    }
  },
};
