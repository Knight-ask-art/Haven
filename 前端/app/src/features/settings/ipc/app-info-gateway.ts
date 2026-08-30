// About / Diagnostics Gateway（V02-SETTINGS-ABOUT-DIAGNOSTICS-008）。
// 目录操作只映射到三个固定 Typed Client 方法，不接受任意路径。

import { toHavenError } from "@/lib/ipc/errors";
import { getHavenClient } from "@/lib/ipc/runtime";
import type { AppDirectoryKindDto, AppInfoDto } from "@/lib/ipc/generated/wire";

export interface AppInfoGateway {
  get(): Promise<AppInfoDto>;
  openDirectory(kind: AppDirectoryKindDto): Promise<void>;
}

export const appInfoGateway: AppInfoGateway = {
  async get() {
    try {
      return await getHavenClient().appInfoGet();
    } catch (error) {
      throw toHavenError(error);
    }
  },
  async openDirectory(kind) {
    try {
      const client = getHavenClient();
      if (kind === "data") return await client.openDataDirectory();
      if (kind === "logs") return await client.openLogsDirectory();
      return await client.openCacheDirectory();
    } catch (error) {
      throw toHavenError(error);
    }
  },
};
