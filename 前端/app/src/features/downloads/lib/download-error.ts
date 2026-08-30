/**
 * Maps stable Worker error codes to safe, actionable user copy.
 *
 * The Worker never exposes paths, URLs, or operating-system error text. Keeping
 * this mapping in the feature layer also lets the notice center and tests share
 * one source without coupling the page to transport details.
 */
export function downloadErrorMessage(code: string): string {
  switch (code) {
    case "DOWNLOAD_DISK_SPACE_LOW":
      return "磁盘空间不足，下载已暂停。请清理空间或更换下载位置后重试。"
    case "DOWNLOAD_DIRECTORY_UNAVAILABLE":
      return "下载目录不可用，请检查目录是否存在或重新绑定存储位置。"
    case "DOWNLOAD_PERMISSION_DENIED":
      return "没有权限写入下载目录，请更换目录或检查系统权限。"
    case "DOWNLOAD_PARTIAL_INVALID":
      return "临时下载文件已失效，重试后将重新校验并继续下载。"
    case "DOWNLOAD_IO_FAILED":
      return "下载过程中发生本地读写错误，请稍后重试。"
    case "DOWNLOAD_SOURCE_UNAVAILABLE":
      return "下载来源当前不可用，请检查原始媒体位置。"
    default:
      return "下载任务需要处理，请重试。"
  }
}

/**
 * Worker 终态事件只携带稳定错误码；重试语义必须由受控码表决定，
 * 不能把所有失败都误报成可立即重试（例如磁盘不足或权限拒绝）。
 */
export function downloadErrorRetryable(code: string): boolean {
  switch (code) {
    case "DOWNLOAD_PARTIAL_INVALID":
    case "DOWNLOAD_IO_FAILED":
      return true
    case "DOWNLOAD_DISK_SPACE_LOW":
    case "DOWNLOAD_DIRECTORY_UNAVAILABLE":
    case "DOWNLOAD_PERMISSION_DENIED":
    case "DOWNLOAD_SOURCE_UNAVAILABLE":
    default:
      return false
  }
}
