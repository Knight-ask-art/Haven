import type {
  VideoScreenshotBeginResultDto,
  VideoScreenshotResultDto,
} from "@/lib/ipc/generated/wire"
import type { HavenClient } from "@/lib/ipc/client"
import { HavenError, toHavenError } from "@/lib/ipc/errors"

export const VIDEO_SCREENSHOT_MAX_EDGE = 4096
export const VIDEO_SCREENSHOT_MAX_CHUNK_BYTES = 64 * 1024
export const VIDEO_SCREENSHOT_MAX_TOTAL_BYTES = 8 * 1024 * 1024

export function isVideoScreenshotShortcut(
  event: Pick<KeyboardEvent, "code" | "ctrlKey" | "shiftKey" | "altKey" | "metaKey" | "repeat">,
): boolean {
  return event.code === "KeyS"
    && event.ctrlKey
    && event.shiftKey
    && !event.altKey
    && !event.metaKey
    && !event.repeat
}

export type VideoScreenshotCapture = {
  bytes: Uint8Array
  width: number
  height: number
}

function screenshotError(code: string, userMessage: string, retryable: boolean): HavenError {
  return new HavenError({ code, userMessage, retryable })
}

/** 把当前 video 帧编码为有界 JPEG；不触碰进度、Marker 或播放状态。 */
export async function captureVideoScreenshotFrame(
  video: HTMLVideoElement,
  maxEdge = VIDEO_SCREENSHOT_MAX_EDGE,
): Promise<VideoScreenshotCapture> {
  if (video.readyState < HTMLMediaElement.HAVE_CURRENT_DATA || video.videoWidth <= 0 || video.videoHeight <= 0) {
    throw screenshotError("SCREENSHOT_NOT_READY", "视频当前帧尚未准备好，请稍后重试。", true)
  }
  const scale = Math.min(1, maxEdge / Math.max(video.videoWidth, video.videoHeight))
  const width = Math.max(1, Math.round(video.videoWidth * scale))
  const height = Math.max(1, Math.round(video.videoHeight * scale))
  const canvas = document.createElement("canvas")
  canvas.width = width
  canvas.height = height
  const context = canvas.getContext("2d")
  if (!context) {
    throw screenshotError("SCREENSHOT_NOT_READY", "无法读取视频当前帧，请稍后重试。", true)
  }
  try {
    context.drawImage(video, 0, 0, width, height)
  } catch {
    throw screenshotError("SCREENSHOT_NOT_READY", "无法读取视频当前帧，请稍后重试。", true)
  }
  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/jpeg", 0.92))
  if (!blob) {
    throw screenshotError("SCREENSHOT_NOT_READY", "无法编码视频当前帧，请稍后重试。", true)
  }
  if (blob.size === 0 || blob.size > VIDEO_SCREENSHOT_MAX_TOTAL_BYTES) {
    throw screenshotError("SCREENSHOT_TOO_LARGE", "截图尺寸或数据过大，请降低视频窗口大小后重试。", false)
  }
  return { bytes: new Uint8Array(await blob.arrayBuffer()), width, height }
}

function validateBeginResult(result: VideoScreenshotBeginResultDto): void {
  if (
    result.schemaVersion !== 1
    || !result.uploadId
    || !Number.isInteger(result.maxChunkBytes)
    || result.maxChunkBytes <= 0
    || result.maxChunkBytes > VIDEO_SCREENSHOT_MAX_CHUNK_BYTES
    || !Number.isInteger(result.maxTotalBytes)
    || result.maxTotalBytes <= 0
    || result.maxTotalBytes > VIDEO_SCREENSHOT_MAX_TOTAL_BYTES
  ) {
    throw screenshotError("SCREENSHOT_PAYLOAD_INVALID", "截图上传能力无效，请重试。", true)
  }
}

/** 通过有界分块上传当前帧，并在异常时尽力取消服务端临时状态。 */
export async function saveVideoScreenshot(
  client: HavenClient,
  video: HTMLVideoElement,
  signal?: AbortSignal,
): Promise<VideoScreenshotResultDto> {
  const capture = await captureVideoScreenshotFrame(video)
  if (signal?.aborted) {
    throw screenshotError("SCREENSHOT_UPLOAD_EXPIRED", "截图操作已取消。", true)
  }
  let begin: VideoScreenshotBeginResultDto | null = null
  try {
    begin = await client.videoScreenshotBegin()
    validateBeginResult(begin)
    const chunkSize = Math.min(begin.maxChunkBytes, VIDEO_SCREENSHOT_MAX_CHUNK_BYTES)
    if (capture.bytes.byteLength > begin.maxTotalBytes) {
      throw screenshotError("SCREENSHOT_TOO_LARGE", "截图数据过大，请重试。", false)
    }
    let sequence = 0
    for (let offset = 0; offset < capture.bytes.byteLength; offset += chunkSize) {
      if (signal?.aborted) throw screenshotError("SCREENSHOT_UPLOAD_EXPIRED", "截图操作已取消。", true)
      const chunk = capture.bytes.slice(offset, Math.min(offset + chunkSize, capture.bytes.byteLength))
      await client.videoScreenshotChunk({ uploadId: begin.uploadId, sequence, bytes: Array.from(chunk) })
      sequence += 1
    }
    return await client.videoScreenshotCommit(begin.uploadId)
  } catch (cause) {
    if (begin?.uploadId) {
      try {
        await client.videoScreenshotCancel(begin.uploadId)
      } catch {
        // 上传已过期/窗口已关闭时无需覆盖原始错误。
      }
    }
    throw toHavenError(cause)
  }
}
