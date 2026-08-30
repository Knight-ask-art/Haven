import { describe, expect, it } from "vitest"

import { downloadErrorMessage, downloadErrorRetryable } from "./download-error"

describe("download error messages", () => {
  it("provides actionable copy for storage and directory failures", () => {
    expect(downloadErrorMessage("DOWNLOAD_DISK_SPACE_LOW")).toContain("磁盘空间不足")
    expect(downloadErrorMessage("DOWNLOAD_DIRECTORY_UNAVAILABLE")).toContain("下载目录不可用")
    expect(downloadErrorMessage("DOWNLOAD_PERMISSION_DENIED")).toContain("没有权限")
  })

  it("does not expose unknown error details", () => {
    expect(downloadErrorMessage("C:\\private\\secret.txt")).toBe("下载任务需要处理，请重试。")
  })

  it("only marks transient worker failures as retryable", () => {
    expect(downloadErrorRetryable("DOWNLOAD_IO_FAILED")).toBe(true)
    expect(downloadErrorRetryable("DOWNLOAD_PARTIAL_INVALID")).toBe(true)
    expect(downloadErrorRetryable("DOWNLOAD_DISK_SPACE_LOW")).toBe(false)
    expect(downloadErrorRetryable("DOWNLOAD_DIRECTORY_UNAVAILABLE")).toBe(false)
    expect(downloadErrorRetryable("DOWNLOAD_PERMISSION_DENIED")).toBe(false)
    expect(downloadErrorRetryable("DOWNLOAD_SOURCE_UNAVAILABLE")).toBe(false)
    expect(downloadErrorRetryable("UNKNOWN")).toBe(false)
  })
})
