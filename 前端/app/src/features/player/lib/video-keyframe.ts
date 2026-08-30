/**
 * 截取 video 当前帧为 JPEG dataURL（Haven 关键帧）。
 * 限制最大边 960，避免 base64 过大（~80KB）。
 * 失败静默返回 null，不阻塞进度保存。
 */
export async function captureVideoKeyframe(
  video: HTMLVideoElement,
  maxWidth = 960,
): Promise<string | null> {
  try {
    if (video.videoWidth === 0 || video.videoHeight === 0) return null
    if (video.readyState < 2) return null
    const w = video.videoWidth
    const h = video.videoHeight
    const scale = Math.min(1, maxWidth / w)
    const cw = Math.round(w * scale)
    const ch = Math.round(h * scale)
    const canvas = document.createElement("canvas")
    canvas.width = cw
    canvas.height = ch
    const ctx = canvas.getContext("2d")
    if (!ctx) return null
    ctx.drawImage(video, 0, 0, cw, ch)
    // 0.72 与 960 的平衡：单帧约 60-120KB base64，IPC 可接受
    const dataUrl = canvas.toDataURL("image/jpeg", 0.72)
    // 过大截断保护（>200KB 则放弃，避免 IPC 超限）
    if (dataUrl.length > 280_000) return null
    return dataUrl
  } catch {
    return null
  }
}
