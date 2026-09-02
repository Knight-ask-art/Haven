import { useCallback, useState, useRef, useEffect, useLayoutEffect } from "react"
import { useParams, useNavigate } from "react-router"
import { VideoControls } from "../components/VideoControls"
import { EpisodeDrawer, type EpisodeItem } from "../components/EpisodeDrawer"
import { recordHistory } from "@/lib/havenState"
import { useMediaSession } from "@/features/session/useMediaSession"
import { selectPlayerSessionView } from "../lib/player-session-view"
import { clampVideoSeek, createVideoProgressController, restoreVideoProgress, type VideoProgressController } from "../lib/video-progress-controller"
import { captureVideoKeyframe } from "../lib/video-keyframe"
import { videoSecondsToMilliseconds } from "../lib/video-marker-position"
import { findVideoBookmark } from "../lib/video-marker-match"
import { createMarker, deleteMarker, listMarkers, videoMarkerLocator } from "@/features/markers/ipc/marker-gateway"
import type { MarkerDto } from "@/lib/ipc/generated/wire"
import { getHavenClient, getHavenClientMode } from "@/lib/ipc/runtime"
import { artworkRequestUri, controlledResourceUri } from "@/lib/artwork-url"
import { getEdition } from "@/features/media/ipc/edition-gateway"
import { normalizeEditionError } from "@/features/media/ipc/edition-gateway"
import { getWorkDetail } from "@/features/media/ipc/work-gateway"
import { loadDemoPlayerData, recordDemoPlayerHistory, resolvePlayerRuntimeState } from "../lib/player-runtime-state"
import { playbackMediaErrorForActiveSource, type PlaybackMediaErrorView } from "../lib/playback-media-error"
import { usePlaybackSettings } from "../lib/usePlaybackSettings"
import { selectNextEpisodeId } from "../lib/select-next-episode"
import { isVideoScreenshotShortcut, saveVideoScreenshot } from "../lib/video-screenshot"
import { defaultCoverPath } from "@/lib/default-cover"
import { useNotice } from "@/app/notice-center/notice-context"

// 模拟播放展示数据（实际内容源始终来自媒体会话）
interface PlayerPresentation {
  title: string
  subtitle?: string
  episodes?: EpisodeItem[]
}

const SAMPLE_VIDEOS: Record<string, PlayerPresentation> = {
  "2": {
    title: "沙丘2 Dune: Part Two",
    subtitle: "4K IMAX 杜比视界高码率正片",
    episodes: [
      { id: "m1", number: "第01集", title: "沙丘2 正片 (4K IMAX)", durationOrPages: "166 分钟", progress: 45 }
    ]
  },
  "4": {
    title: "怪奇物语：1985故事集 第一季",
    subtitle: "第04集 · 湖底的秘密尸体",
    episodes: [
      { id: "e1", number: "第01集", title: "威尔·拜尔斯的失踪", durationOrPages: "48 分钟", progress: 100 },
      { id: "e2", number: "第02集", title: "枫树街上的怪人", durationOrPages: "55 分钟", progress: 100 },
      { id: "e3", number: "第03集", title: "节日灯光与密码", durationOrPages: "51 分钟", progress: 100 },
      { id: "e4", number: "第04集", title: "湖底的秘密尸体", durationOrPages: "50 分钟", progress: 45 },
      { id: "e5", number: "第05集", title: "跳蚤与手风琴理论", durationOrPages: "53 分钟", progress: 0 }
    ]
  }
}

export function PlayerPage() {
  const { mediaItemId } = useParams<{ mediaItemId: string }>()
  const navigate = useNavigate()
  const clientMode = getHavenClientMode()
  const runtimeState = resolvePlayerRuntimeState(clientMode)
  const demoMode = runtimeState === "demo"
  const productionMode = runtimeState === "production"
  const { push } = useNotice()
  const playbackSettings = usePlaybackSettings()
  const { state, retry, registerReleaseBarrier } = useMediaSession(mediaItemId, "playback")
  const sessionView = selectPlayerSessionView(state, mediaItemId)
  // Windows WebView 需把自定义协议转 http 形式，否则 XHR/MSE 加载器无法请求。
  const playbackSourceUri = controlledResourceUri(sessionView.contentUri)
  const sessionContentUri = playbackSourceUri
  // 受控流的种类由后端明确投影；旧会话没有该字段时保留 HLS URI 的兼容推断。
  const streamKind = sessionView.streamKind
    ?? (sessionContentUri.includes("haven-resource.stream") ? "hls" : null)

  useEffect(() => {
    if (mediaItemId) recordDemoPlayerHistory(clientMode, mediaItemId, recordHistory)
  }, [clientMode, mediaItemId])

  const videoData: PlayerPresentation = loadDemoPlayerData(clientMode, () => SAMPLE_VIDEOS[mediaItemId || "4"] || {
    title: "又被杀掉了呢，侦探大人",
    subtitle: "第01集",
    episodes: [
      { id: "b1", number: "第01集", title: "追月朔也与名侦探父亲", progress: 100 },
      { id: "b2", number: "第02集", title: "寻猫委托与神秘迷案", progress: 0 },
      { id: "b3", number: "第03集", title: "密室里的第三名被害者", progress: 0 },
      { id: "b4", number: "第04集", title: "被遗忘的嫌疑人", progress: 0 },
      { id: "b5", number: "第05集", title: "终极谎言与真相大白", progress: 0 }
    ]
  }) ?? { title: "正在播放" }

  const [demoCurrentEpisodeId, setDemoCurrentEpisodeId] = useState(
    videoData.episodes?.find((episode) => episode.progress && episode.progress > 0 && episode.progress < 100)?.id
      || videoData.episodes?.[0]?.id
      || ""
  )

  // 生产模式：从会话携带的 Edition 拉取真实选集列表，供右侧选集面板使用。
  const editionId = state.status === "ready" ? state.session.editionId : null
  const [editionEpisodes, setEditionEpisodes] = useState<EpisodeItem[] | null>(null)
  const [editionTitle, setEditionTitle] = useState<string | null>(null)

  useEffect(() => {
    if (!productionMode || !editionId) {
      setEditionEpisodes(null)
      setEditionTitle(null)
      return undefined
    }
    let active = true
    void getEdition(editionId)
      .then((detail) => {
        if (!active) return
        setEditionTitle(detail.title)
        const episodes: EpisodeItem[] = detail.items
          .slice()
          .sort((a, b) => {
            const ea = a.episodeNumber ?? 999999
            const eb = b.episodeNumber ?? 999999
            if (ea !== eb) return ea - eb
            return a.indexLabel.localeCompare(b.indexLabel, "zh-Hans-CN", { numeric: true })
          })
          .map((item) => ({
            id: item.mediaItemId,
            number: item.indexLabel,
            title: item.title,
            durationOrPages: item.durationMs ? `${Math.round(item.durationMs / 60000)} 分钟` : undefined,
          }))
        setEditionEpisodes(episodes.length > 0 ? episodes : null)
      })
      .catch((error: unknown) => {
        if (!active) return
        console.warn(normalizeEditionError(error).code)
        setEditionEpisodes(null)
        setEditionTitle(null)
      })
    return () => {
      active = false
    }
  }, [productionMode, editionId])

  const drawerEpisodes = demoMode ? videoData.episodes : (editionEpisodes ?? undefined)
  const playerTitle = demoMode ? videoData.title : (editionTitle ?? "正在播放")
  const currentEpisodeId = demoMode ? demoCurrentEpisodeId : (mediaItemId ?? "")
  const currentEp = drawerEpisodes?.find((e) => e.id === currentEpisodeId)
  const displayTitle = currentEp ? `${playerTitle} ${currentEp.number}` : playerTitle
  const displaySubtitle = currentEp ? `${currentEp.number} · ${currentEp.title}` : (demoMode ? videoData.subtitle : undefined)

  // 海报与抽屉元数据：从 Work 详情获取真实海报/年份/简介
  const workIdForPoster = state.status === "ready" ? state.session.workId : null
  const [posterRawUri, setPosterRawUri] = useState<string | null>(null)
  const posterUri = artworkRequestUri(posterRawUri) || undefined
  const [drawerMeta, setDrawerMeta] = useState<{ year?: number; description?: string } | null>(null)
  useEffect(() => {
    if (!productionMode || !workIdForPoster) {
      setPosterRawUri(null)
      setDrawerMeta(null)
      return undefined
    }
    let active = true
    void getWorkDetail(workIdForPoster)
      .then((detail) => {
        if (!active) return
        setPosterRawUri(detail.posterUri ?? null)
        setDrawerMeta({ year: detail.releaseYear ?? undefined, description: detail.description ?? undefined })
      })
      .catch(() => {
        if (!active) return
        setPosterRawUri(null)
        setDrawerMeta(null)
      })
    return () => { active = false }
  }, [productionMode, workIdForPoster])

  const videoRef = useRef<HTMLVideoElement>(null)
  const activePlaybackSourceRef = useRef<string | null>(sessionContentUri)
  const progressControllerRef = useRef<VideoProgressController | null>(null)
  const restoredProgressRef = useRef<string | null>(null)
  const metadataReadyRef = useRef(false)
  const bookmarkOperationRef = useRef(0)
  const markerListRequestRef = useRef(0)
  const containerRef = useRef<HTMLDivElement>(null)

  const [isPlaying, setIsPlaying] = useState(true)
  const [currentTime, setCurrentTime] = useState(0)
  const [duration, setDuration] = useState(0)
  const [volume, setVolume] = useState(0.8)
  const [isMuted, setIsMuted] = useState(false)
  const [playbackRate, setPlaybackRate] = useState(1.0)
  const [quality, setQuality] = useState("1080P")
  const [isFullscreen, setIsFullscreen] = useState(false)
  const [showControls, setShowControls] = useState(true)
  const [isSidePanelOpen, setIsSidePanelOpen] = useState(true)
  const [playbackError, setPlaybackError] = useState<PlaybackMediaErrorView | null>(null)
  const [isBookmarked, setIsBookmarked] = useState(false)
  const [bookmarkMarkerId, setBookmarkMarkerId] = useState<string | null>(null)
  const [isBookmarkPending, setIsBookmarkPending] = useState(false)
  const [markersLoaded, setMarkersLoaded] = useState(false)
  const [sessionMarkers, setSessionMarkers] = useState<MarkerDto[]>([])
  const [isBuffering, setIsBuffering] = useState(false)
  const [bufferedRanges, setBufferedRanges] = useState<Array<[number, number]>>([])
  const lastAudibleVolumeRef = useRef(0.8)
  const playbackRateInitializedRef = useRef(false)
  const bufferingTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const watchdogRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const screenshotBusyRef = useRef(false)
  const screenshotAbortRef = useRef<AbortController | null>(null)
  const lastTimeRef = useRef<number>(0)
  const lastTimeUpdateRef = useRef<number>(Date.now())
  // 持久化播放偏好（localStorage，仅 UI 偏好，符合 AGENTS 约束）
  const PLAYBACK_PREFS_KEY = "haven:ui:playback-prefs"

  const hideTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  // 持久化偏好加载
  useEffect(() => {
    try {
      const raw = localStorage.getItem(PLAYBACK_PREFS_KEY)
      if (raw) {
        const prefs = JSON.parse(raw) as { volume?: number; muted?: boolean; rate?: number }
        if (typeof prefs.volume === "number" && prefs.volume >= 0 && prefs.volume <= 1) {
          setVolume(prefs.volume)
          lastAudibleVolumeRef.current = prefs.volume || 0.8
        }
        if (typeof prefs.muted === "boolean") setIsMuted(prefs.muted)
      }
    } catch { /* ignore */ }
  }, [])

  // 默认倍速来自 Rust Settings；播放器内临时改速只作用于当前会话。
  useEffect(() => {
    playbackRateInitializedRef.current = false
  }, [mediaItemId])

  useEffect(() => {
    if (playbackSettings.status !== "ready" || playbackRateInitializedRef.current) return
    playbackRateInitializedRef.current = true
    setPlaybackRate(playbackSettings.defaultPlaybackRate)
    if (videoRef.current) videoRef.current.playbackRate = playbackSettings.defaultPlaybackRate
  }, [mediaItemId, playbackSettings.defaultPlaybackRate, playbackSettings.status])

  // Settings 与媒体 metadata 是两个独立的异步来源。若视频先触发 metadata，
  // 不能用 hook 的默认值（autoResume=true）提前恢复，避免用户关闭自动继续后仍跳回旧进度。
  const sessionForProgress = state.status === "ready" ? state.session : null
  const restoreProgressIfAllowed = useCallback(() => {
    const video = videoRef.current
    if (
      !video ||
      !metadataReadyRef.current ||
      playbackSettings.status === "loading" ||
      !playbackSettings.autoResume ||
      sessionView.status !== "ready" ||
      sessionForProgress === null
    ) return
    restoreVideoProgress(video, sessionForProgress, restoredProgressRef)
  }, [playbackSettings.autoResume, playbackSettings.status, sessionForProgress, sessionView.status])

  useEffect(() => {
    restoreProgressIfAllowed()
  }, [restoreProgressIfAllowed])

  // 鼠标无动作 3 秒自动隐藏 HUD
  const resetHideTimer = () => {
    setShowControls(true)
    if (hideTimeoutRef.current) clearTimeout(hideTimeoutRef.current)
    hideTimeoutRef.current = setTimeout(() => {
      if (isPlaying) {
        setShowControls(false)
      }
    }, 3500)
  }

  useEffect(() => {
    resetHideTimer()
    return () => {
      if (hideTimeoutRef.current) clearTimeout(hideTimeoutRef.current)
    }
  }, [isPlaying])

  // 全屏状态同步：双击/ESC 等外部触发亦能更新 isFullscreen
  useEffect(() => {
    const onFsChange = () => setIsFullscreen(!!document.fullscreenElement)
    document.addEventListener("fullscreenchange", onFsChange)
    return () => document.removeEventListener("fullscreenchange", onFsChange)
  }, [])

  // 缓冲态 + 看门狗：waiting 300ms 后显 spinner，playing/canplay 消；4s 无进展兜底 nudge
  useEffect(() => {
    const video = videoRef.current
    if (!video) return undefined
    const onWaiting = () => {
      if (bufferingTimeoutRef.current) clearTimeout(bufferingTimeoutRef.current)
      bufferingTimeoutRef.current = setTimeout(() => setIsBuffering(true), 300)
    }
    const onPlaying = () => {
      if (bufferingTimeoutRef.current) clearTimeout(bufferingTimeoutRef.current)
      setIsBuffering(false)
      lastTimeRef.current = video.currentTime
      lastTimeUpdateRef.current = Date.now()
    }
    const onCanPlay = () => {
      if (bufferingTimeoutRef.current) clearTimeout(bufferingTimeoutRef.current)
      setIsBuffering(false)
    }
    const onStalled = () => {
      if (bufferingTimeoutRef.current) clearTimeout(bufferingTimeoutRef.current)
      bufferingTimeoutRef.current = setTimeout(() => setIsBuffering(true), 300)
    }
    video.addEventListener("waiting", onWaiting)
    video.addEventListener("playing", onPlaying)
    video.addEventListener("canplay", onCanPlay)
    video.addEventListener("stalled", onStalled)
    // buffered 轮询
    const bufferedTimer = window.setInterval(() => {
      try {
        const ranges: Array<[number, number]> = []
        for (let i = 0; i < video.buffered.length; i++) {
          ranges.push([video.buffered.start(i), video.buffered.end(i)])
        }
        setBufferedRanges(ranges)
      } catch { /* ignore */ }
    }, 500)
    // 看门狗：!paused && readyState<=2 持续 >4s 则 nudge
    watchdogRef.current = window.setInterval(() => {
      const v = videoRef.current
      if (!v || v.paused || v.ended) return
      const now = Date.now()
      const progressed = Math.abs(v.currentTime - lastTimeRef.current) > 0.05
      if (progressed) {
        lastTimeRef.current = v.currentTime
        lastTimeUpdateRef.current = now
        return
      }
      if (v.readyState <= 2 && now - lastTimeUpdateRef.current > 4000) {
        console.warn("[watchdog] stalled, nudging", { currentTime: v.currentTime, readyState: v.readyState })
        try {
          v.currentTime = v.currentTime + 0.1
          lastTimeUpdateRef.current = now
        } catch { /* ignore */ }
      }
    }, 1000) as unknown as ReturnType<typeof setInterval>
    return () => {
      video.removeEventListener("waiting", onWaiting)
      video.removeEventListener("playing", onPlaying)
      video.removeEventListener("canplay", onCanPlay)
      video.removeEventListener("stalled", onStalled)
      window.clearInterval(bufferedTimer)
      if (watchdogRef.current) window.clearInterval(watchdogRef.current as unknown as number)
      if (bufferingTimeoutRef.current) clearTimeout(bufferingTimeoutRef.current)
    }
  }, [])

  // Clear media element state whenever the route or validated session changes.
  useLayoutEffect(() => {
    screenshotAbortRef.current?.abort()
    screenshotAbortRef.current = null
    screenshotBusyRef.current = false
    activePlaybackSourceRef.current = sessionContentUri
    metadataReadyRef.current = false
    restoredProgressRef.current = null
    bookmarkOperationRef.current += 1
    markerListRequestRef.current += 1
    setIsBookmarked(false)
    setBookmarkMarkerId(null)
    setIsBookmarkPending(false)
    setSessionMarkers([])
    setMarkersLoaded(false)
    setCurrentTime(0)
    setDuration(0)
    setIsPlaying(false)
    setPlaybackError(null)
    videoRef.current?.pause()
    videoRef.current?.load()
  }, [mediaItemId, sessionContentUri])

  useEffect(() => () => {
    screenshotAbortRef.current?.abort()
  }, [])

  // HLS 远端流经 hls.js；普通 MP4/WebM 远端流和本地资源走原生 src。
  useEffect(() => {
    const video = videoRef.current
    if (!video || !sessionContentUri || streamKind !== "hls") {
      if (video && sessionContentUri && streamKind === "direct") {
        video.src = sessionContentUri
        video.load()
      }
      return undefined
    }
    let destroyed = false
    let hls: import("hls.js").default | null = null
    void import("hls.js")
      .then(({ default: Hls }) => {
        if (destroyed || videoRef.current !== video) return
        if (Hls.isSupported()) {
          hls = new Hls({
            enableWorker: true,
            lowLatencyMode: false,
            maxBufferLength: 30,
            maxMaxBufferLength: 60,
            maxBufferSize: 60 * 1000 * 1000,
            backBufferLength: 30,
            maxBufferHole: 0.5,
            maxFragLookUpTolerance: 0.25,
            highBufferWatchdogPeriod: 3,
            nudgeOffset: 0.1,
            nudgeMaxRetry: 3,
            capLevelToPlayerSize: true,
            capLevelOnFPSDrop: false,
            abrEwmaDefaultEstimate: 500_000,
            abrEwmaDefaultEstimateMax: 5_000_000,
            abrBandWidthFactor: 0.95,
            abrBandWidthUpFactor: 0.7,
            fragLoadPolicy: {
              default: {
                maxTimeToFirstByteMs: 9000,
                maxLoadTimeMs: 100_000,
                timeoutRetry: { maxNumRetry: 2, retryDelayMs: 0, maxRetryDelayMs: 0 },
                errorRetry: { maxNumRetry: 5, retryDelayMs: 3000, maxRetryDelayMs: 15000, backoff: "linear" as const },
              },
            },
            manifestLoadPolicy: {
              default: {
                maxTimeToFirstByteMs: 9000,
                maxLoadTimeMs: 15000,
                timeoutRetry: { maxNumRetry: 2, retryDelayMs: 0, maxRetryDelayMs: 0 },
                errorRetry: { maxNumRetry: 3, retryDelayMs: 2000, maxRetryDelayMs: 8000, backoff: "linear" as const },
              },
            },
          })
          hls.loadSource(sessionContentUri)
          hls.attachMedia(video)
          // 暴露最后一次错误细节供 CDP 诊断（不落盘，仅内存）。
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          ;(window as unknown as Record<string, unknown>).__havenLastHlsError = null
          let networkRetryCount = 0
          let mediaRetryCount = 0
          hls.on(Hls.Events.ERROR, (_event, data) => {
            if (destroyed) return
            // eslint-disable-next-line @typescript-eslint/no-explicit-any
            ;(window as unknown as Record<string, unknown>).__havenLastHlsError = data
            // 通用空洞跳过：优先使用 hls.js 提供的 nextStart
            if (data.details === "bufferStalledError") {
              const nextStart = (data as unknown as { nextStart?: number }).nextStart
              console.warn("[hls] bufferStalled, attempting skip", data, { nextStart })
              try {
                const v = videoRef.current
                if (v) {
                  const target = typeof nextStart === "number" && Number.isFinite(nextStart) ? nextStart + 0.1 : v.currentTime + 0.25
                  // 避免跳进同一空洞
                  if (Math.abs(target - v.currentTime) > 0.05) v.currentTime = target
                }
                hls?.startLoad()
              } catch { /* ignore */ }
              return
            }
            if (!data.fatal) {
              console.warn("[hls] non-fatal", data.type, data.details, data)
              return
            }
            console.error("[hls] fatal", data.type, data.details, data)
            // 410 过期：片源已过期，触发会话重开（将命中后台已刷新的新 locator）
            const responseCode = (data as unknown as { response?: { code?: number } }).response?.code
            const is410 = responseCode === 410 || String((data as unknown as { networkDetails?: unknown }).networkDetails ?? "").includes("410")
            if (is410) {
              console.warn("[hls] 410 expired, triggering session retry for fresh locator")
              setPlaybackError({
                code: "RESOURCE_UNAVAILABLE",
                state: "failed",
                title: "片源已过期",
                message: "正在切换备用源，请稍候…",
                retryable: true,
              })
              setTimeout(() => { if (!destroyed) retry() }, 1500)
              return
            }
            if (data.type === Hls.ErrorTypes.NETWORK_ERROR) {
              networkRetryCount += 1
              if (networkRetryCount <= 3) {
                const backoffMs = [1000, 2000, 4000][networkRetryCount - 1] ?? 4000
                console.warn(`[hls] network fatal retry ${networkRetryCount}/3 after ${backoffMs}ms`)
                setTimeout(() => {
                  if (!destroyed) try { hls?.startLoad(videoRef.current?.currentTime ?? undefined) } catch { /* ignore */ }
                }, backoffMs)
                return
              }
            }
            if (data.type === Hls.ErrorTypes.MEDIA_ERROR) {
              mediaRetryCount += 1
              if (mediaRetryCount <= 2) {
                console.warn(`[hls] media fatal recover ${mediaRetryCount}/2`)
                try { hls?.recoverMediaError() } catch { /* ignore */ }
                return
              }
              // 第二次仍失败，尝试音频编解码切换 + 恢复
              if (mediaRetryCount === 3) {
                try {
                  const h = hls as unknown as { swapAudioCodec?: () => void }
                  h.swapAudioCodec?.()
                  hls?.recoverMediaError()
                  return
                } catch { /* ignore */ }
              }
            }
            setPlaybackError({
              code: "DECODER_FAILED",
              state: "failed",
              title: "流媒体加载失败",
              message: "无法加载该在线视频流，请稍后重试。",
              retryable: true,
            })
            setIsPlaying(false)
          })
        } else {
          // 环境原生支持 HLS 时直接回退 src。
          video.src = sessionContentUri
        }
      })
      .catch(() => {
        setPlaybackError({
          code: "RESOURCE_OPEN_FAILED",
          state: "failed",
          title: "播放组件加载失败",
          message: "播放组件未能初始化，请稍后重试。",
          retryable: true,
        })
      })
    return () => {
      destroyed = true
      hls?.destroy()
    }
  }, [sessionContentUri, streamKind])

  useEffect(() => {
    const requestId = ++markerListRequestRef.current
    if (!productionMode || !mediaItemId || sessionView.status !== "ready") return

    void listMarkers(mediaItemId)
      .then((markers) => {
        if (markerListRequestRef.current === requestId) {
          setSessionMarkers(markers)
          setMarkersLoaded(true)
        }
      })
      .catch(() => {
        if (markerListRequestRef.current === requestId) {
          setSessionMarkers([])
          setMarkersLoaded(true)
        }
      })
  }, [mediaItemId, productionMode, sessionContentUri, sessionView.status])

  useEffect(() => {
    if (!productionMode || !mediaItemId || sessionView.status !== "ready" || isBookmarkPending || !markersLoaded) return
    const marker = findVideoBookmark(sessionMarkers, mediaItemId, currentTime)
    setIsBookmarked(marker !== null)
    setBookmarkMarkerId(marker?.markerId ?? null)
  }, [currentTime, isBookmarkPending, markersLoaded, mediaItemId, productionMode, sessionMarkers, sessionView.status])

  const toggleVideoBookmark = () => {
    if (!mediaItemId || sessionView.status !== "ready" || isBookmarkPending || (productionMode && !markersLoaded)) return
    if (productionMode && isBookmarked && bookmarkMarkerId === null) return

    const nextIsBookmarked = !isBookmarked
    const removingMarkerId = bookmarkMarkerId
    setIsBookmarked(nextIsBookmarked)
    if (!nextIsBookmarked) setBookmarkMarkerId(null)

    // Browser preview owns only an ephemeral projection and must never issue Marker IPC.
    if (!productionMode) return

    markerListRequestRef.current += 1
    const operation = ++bookmarkOperationRef.current
    setIsBookmarkPending(true)
    if (nextIsBookmarked) {
      const positionSeconds = videoRef.current?.currentTime ?? currentTime
      void createMarker({
        mediaItemId,
        locator: videoMarkerLocator(videoSecondsToMilliseconds(positionSeconds)),
        markerType: "bookmark",
        title: null,
        excerpt: null,
        note: null,
      })
        .then((marker) => {
          if (bookmarkOperationRef.current !== operation) return
          setSessionMarkers((current) => [...current.filter((item) => item.markerId !== marker.markerId), marker])
          setBookmarkMarkerId(marker.markerId)
        })
        .catch(() => {
          if (bookmarkOperationRef.current !== operation) return
          setIsBookmarked(false)
          setBookmarkMarkerId(null)
        })
        .finally(() => {
          if (bookmarkOperationRef.current === operation) setIsBookmarkPending(false)
        })
      return
    }

    if (removingMarkerId === null) {
      setIsBookmarked(true)
      setIsBookmarkPending(false)
      return
    }
    const removedMarker = sessionMarkers.find((marker) => marker.markerId === removingMarkerId) ?? null
    setSessionMarkers((current) => current.filter((marker) => marker.markerId !== removingMarkerId))
    void deleteMarker(removingMarkerId)
      .catch(() => {
        if (bookmarkOperationRef.current !== operation) return
        if (removedMarker) setSessionMarkers((current) => [...current, removedMarker])
        setIsBookmarked(true)
        setBookmarkMarkerId(removingMarkerId)
      })
      .finally(() => {
        if (bookmarkOperationRef.current === operation) setIsBookmarkPending(false)
      })
  }

  useEffect(() => {
    progressControllerRef.current = null
    if (sessionView.status !== "ready" || state.status !== "ready") return
    const controller = createVideoProgressController({
      session: state.session,
      retry,
      captureKeyframe: async () => {
        const v = videoRef.current
        if (!v) return null
        return captureVideoKeyframe(v)
      },
    })
    progressControllerRef.current = controller
    registerReleaseBarrier(() => controller.cleanup())
    return () => { progressControllerRef.current = null; registerReleaseBarrier(null) }
  }, [mediaItemId, sessionContentUri, sessionView.status, state, retry, registerReleaseBarrier])

  // 播放 / 暂停切换
  const togglePlay = () => {
    if (!videoRef.current || sessionView.status !== "ready") return
    if (isPlaying) {
      videoRef.current.pause()
      setIsPlaying(false)
    } else {
      void videoRef.current.play()
        .then(() => setIsPlaying(true))
        .catch(() => setIsPlaying(false))
    }
  }

  // 进度跳转
  const handleSeek = (time: number) => {
    if (!videoRef.current || sessionView.status !== "ready") return
    const nextTime = clampVideoSeek(time, videoRef.current.duration)
    videoRef.current.currentTime = nextTime
    setCurrentTime(nextTime)
  }

  // 音量切换
  const handleVolumeChange = (vol: number) => {
    if (!videoRef.current) return
    videoRef.current.volume = vol
    videoRef.current.muted = vol === 0
    setVolume(vol)
    setIsMuted(vol === 0)
    if (vol > 0) lastAudibleVolumeRef.current = vol
    try { localStorage.setItem(PLAYBACK_PREFS_KEY, JSON.stringify({ volume: vol, muted: vol === 0 })) } catch { /* ignore */ }
  }

  const toggleMute = () => {
    if (!videoRef.current) return
    if (isMuted) {
      const nextVolume = volume > 0 ? volume : lastAudibleVolumeRef.current
      videoRef.current.volume = nextVolume
      videoRef.current.muted = false
      setVolume(nextVolume)
      setIsMuted(false)
      try { localStorage.setItem(PLAYBACK_PREFS_KEY, JSON.stringify({ volume: nextVolume, muted: false })) } catch { /* ignore */ }
    } else {
      if (volume > 0) lastAudibleVolumeRef.current = volume
      videoRef.current.muted = true
      setIsMuted(true)
      try { localStorage.setItem(PLAYBACK_PREFS_KEY, JSON.stringify({ volume, muted: true })) } catch { /* ignore */ }
    }
  }

  // 倍速切换
  const handleRateChange = (rate: number) => {
    if (!videoRef.current) return
    videoRef.current.playbackRate = rate
    // 保持音调不变
    try {
      const v = videoRef.current as HTMLVideoElement & { preservesPitch?: boolean; mozPreservesPitch?: boolean; webkitPreservesPitch?: boolean }
      v.preservesPitch = true
      if ("mozPreservesPitch" in v) v.mozPreservesPitch = true
      if ("webkitPreservesPitch" in v) v.webkitPreservesPitch = true
    } catch { /* ignore */ }
    playbackRateInitializedRef.current = true
    setPlaybackRate(rate)
  }

  // 全屏切换
  const toggleFullscreen = () => {
    if (!containerRef.current) return
    if (!document.fullscreenElement) {
      containerRef.current.requestFullscreen().then(() => setIsFullscreen(true)).catch(() => {})
    } else {
      document.exitFullscreen().then(() => setIsFullscreen(false)).catch(() => {})
    }
  }

  const advanceToNextEpisode = useCallback(() => {
    // Settings 仍在加载时不能依据安全默认值误切集：先保持结束态，
    // 避免已持久化的 autoNext=false 被异步读取前短暂绕过。
    if (playbackSettings.status === "loading" || !playbackSettings.autoNext) return
    const nextEpisodeId = selectNextEpisodeId(drawerEpisodes, currentEpisodeId)
    if (!nextEpisodeId) return

    if (demoMode) {
      setDemoCurrentEpisodeId(nextEpisodeId)
      const video = videoRef.current
      if (!video) return
      video.currentTime = 0
      setCurrentTime(0)
      void video.play()
        .then(() => setIsPlaying(true))
        .catch(() => setIsPlaying(false))
      return
    }

    navigate(`/player/${nextEpisodeId}`)
  }, [currentEpisodeId, demoMode, drawerEpisodes, navigate, playbackSettings.autoNext, playbackSettings.status])

  const handleVideoEnded = useCallback(() => {
    setIsPlaying(false)
    const positionSeconds = videoRef.current?.currentTime
    const completion = positionSeconds === undefined
      ? undefined
      : progressControllerRef.current?.ended(positionSeconds)
    if (!completion) {
      advanceToNextEpisode()
      return
    }
    // 先提交完成进度，再切换到下一项；写入失败也不能把用户卡在结束画面。
    void completion.then(advanceToNextEpisode, advanceToNextEpisode)
  }, [advanceToNextEpisode])

  const showScreenshotNotice = useCallback((
    message: string,
    kind: "info" | "success" | "warning" | "error" = "info",
    code?: string,
  ) => {
    const title = code === "SCREENSHOT_DIALOG_START_FAILED"
      ? "无法打开保存对话框"
      : kind === "success" ? "截图已保存" : kind === "error" ? "截图失败" : "截图"
    push({
      kind,
      title,
      message,
      code,
      retryable: kind === "error",
      dedupeKey: code ? `screenshot:${code}` : `screenshot:${message}`,
    })
  }, [push])

  const handleVideoScreenshot = useCallback(async () => {
    if (screenshotBusyRef.current) return
    const video = videoRef.current
    if (!video || !productionMode || sessionView.status !== "ready") {
      showScreenshotNotice("视频当前帧尚未准备好，请稍后重试。", "warning", "SCREENSHOT_NOT_READY")
      return
    }
    screenshotBusyRef.current = true
    const controller = new AbortController()
    screenshotAbortRef.current = controller
    try {
      const result = await saveVideoScreenshot(getHavenClient(), video, controller.signal)
      showScreenshotNotice(
        result.status === "saved" ? "截图已保存。" : "已取消保存截图。",
        result.status === "saved" ? "success" : "info",
      )
    } catch (cause) {
      const error = cause as { code?: unknown; message?: unknown }
      const code = typeof error.code === "string" ? error.code : "SCREENSHOT_SAVE_FAILED"
      const message = typeof error.message === "string" ? error.message : "截图保存失败，请重试。"
      showScreenshotNotice(message, code === "SCREENSHOT_UPLOAD_EXPIRED" ? "info" : "error", code)
    } finally {
      if (screenshotAbortRef.current === controller) screenshotAbortRef.current = null
      screenshotBusyRef.current = false
    }
  }, [productionMode, sessionView.status, showScreenshotNotice])

  // 固定快捷键 Ctrl+Shift+S；只在 PlayerPage 注册，避免设置/阅读页面误触发。
  useEffect(() => {
    const isEditableTarget = (target: EventTarget | null) => {
      const element = target instanceof HTMLElement ? target : null
      if (!element) return false
      return element.isContentEditable
        || element instanceof HTMLInputElement
        || element instanceof HTMLTextAreaElement
        || element instanceof HTMLSelectElement
        || element instanceof HTMLDialogElement
        || element.closest("[contenteditable='true'], [role='dialog'], dialog") !== null
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (!isVideoScreenshotShortcut(event) || isEditableTarget(event.target)) return
      event.preventDefault()
      void handleVideoScreenshot()
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [handleVideoScreenshot])

  // 快捷键支持 (Space / F)
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.code === "Space") {
        e.preventDefault()
        togglePlay()
        resetHideTimer()
      } else if (e.code === "KeyF") {
        e.preventDefault()
        toggleFullscreen()
      } else if (e.code === "ArrowRight") {
        if (videoRef.current) handleSeek(videoRef.current.currentTime + 5)
        resetHideTimer()
      } else if (e.code === "ArrowLeft") {
        if (videoRef.current) handleSeek(videoRef.current.currentTime - 5)
        resetHideTimer()
      }
    }

    window.addEventListener("keydown", handleKeyDown)
    return () => window.removeEventListener("keydown", handleKeyDown)
  }, [isPlaying, sessionView.status])

  return (
    <div 
      ref={containerRef}
      onMouseMove={resetHideTimer}
      className="relative w-screen h-screen bg-black overflow-hidden select-none flex items-stretch"
    >
      {/* 左侧：视频播放画板区域 */}
      <div className="relative flex-1 h-full bg-black flex items-center justify-center overflow-hidden">
        {/* HTML5 Video Element */}
        <video
          key={sessionContentUri ?? "no-session"}
          ref={videoRef}
          src={streamKind === "hls" ? undefined : (sessionContentUri ?? undefined)}
          data-session-source={sessionContentUri ?? ""}
          poster={posterUri ?? defaultCoverPath("video", mediaItemId ?? "player")}
          preload="metadata"
          autoPlay
          playsInline
          onTimeUpdate={() => {
            if (videoRef.current) {
              setCurrentTime(videoRef.current.currentTime)
              lastTimeRef.current = videoRef.current.currentTime
              lastTimeUpdateRef.current = Date.now()
              progressControllerRef.current?.timeupdate(videoRef.current.currentTime)
            }
          }}
          onLoadedMetadata={(event) => {
            const video = event.currentTarget
            if (video.dataset.sessionSource !== activePlaybackSourceRef.current) return
            metadataReadyRef.current = true
            video.volume = volume
            video.muted = isMuted
            try {
              const v = video as HTMLVideoElement & { preservesPitch?: boolean; mozPreservesPitch?: boolean; webkitPreservesPitch?: boolean }
              v.preservesPitch = true
              if ("mozPreservesPitch" in v) v.mozPreservesPitch = true
              if ("webkitPreservesPitch" in v) v.webkitPreservesPitch = true
              v.playbackRate = playbackRate
            } catch { /* ignore */ }
            setDuration(video.duration)
            restoreProgressIfAllowed()
            setPlaybackError(null)
          }}
          onPlay={() => setIsPlaying(true)}
          onPause={() => { setIsPlaying(false); if (videoRef.current) void progressControllerRef.current?.pause(videoRef.current.currentTime) }}
          onSeeked={() => { if (videoRef.current) void progressControllerRef.current?.seeked(videoRef.current.currentTime) }}
          onEnded={handleVideoEnded}
          onError={(event) => {
            if (sessionView.status !== "ready") return
            const error = playbackMediaErrorForActiveSource(
              event.currentTarget.error?.code,
              event.currentTarget.dataset.sessionSource ?? "",
              activePlaybackSourceRef.current,
            )
            if (error === null) return
            setPlaybackError(error)
            setIsPlaying(false)
          }}
          className="w-full h-full object-contain cursor-pointer"
          onClick={togglePlay}
        />

        {sessionView.status === "opening" && (
          <div className="absolute inset-0 z-20 flex items-center justify-center bg-black/45 px-6 text-center text-white">
            <p className="text-sm font-medium text-white/80">正在准备播放…</p>
          </div>
        )}

        {(sessionView.status === "retryable_error" || sessionView.status === "terminal_error") && (
          <div className="absolute inset-0 z-20 flex items-center justify-center bg-black/80 px-6 text-center text-white">
            <div className="max-w-sm space-y-3">
              <p className="text-base font-semibold">{sessionView.message}</p>
              {sessionView.retryable && (
                <button
                  type="button"
                  onClick={retry}
                  className="rounded-full border border-white/20 px-[16px] py-[8px] text-sm font-semibold text-white transition-colors hover:bg-white/10"
                >
                  重试
                </button>
              )}
            </div>
          </div>
        )}

        {sessionView.status === "ready" && playbackError && (
          <div className="absolute inset-0 z-20 flex items-center justify-center bg-black/80 px-6 text-center text-white">
            <div className="max-w-sm space-y-3">
              <p className="text-base font-semibold">{playbackError.title}</p>
              <p className="text-sm text-white/60">{playbackError.message}</p>
              {playbackError.retryable && (
                <button
                  type="button"
                  onClick={() => {
                    const video = videoRef.current
                    setPlaybackError(null)
                    if (!video || video.dataset.sessionSource !== activePlaybackSourceRef.current) return
                    video.load()
                    void video.play().catch(() => {
                      const error = playbackMediaErrorForActiveSource(
                        video.error?.code,
                        video.dataset.sessionSource ?? "",
                        activePlaybackSourceRef.current,
                      )
                      if (error !== null) setPlaybackError(error)
                    })
                  }}
                  className="rounded-full border border-white/20 px-[16px] py-[8px] text-sm font-semibold text-white transition-colors hover:bg-white/10"
                >
                  重试
                </button>
              )}
            </div>
          </div>
        )}

        {demoMode && (
          <div className="absolute bottom-[96px] inset-x-0 text-center pointer-events-none z-20">
            <span className="px-[16px] py-1.5 rounded bg-black/75 text-white font-bold text-lg md:text-xl shadow-2xl backdrop-blur-xs tracking-wide">
              把犯人辛苦堆出来的沙堡。
            </span>
          </div>
        )}

        {/* HUD 控制界面 Overlay */}
        <VideoControls
          title={displayTitle}
          subtitle={displaySubtitle}
          isPlaying={isPlaying}
          currentTime={currentTime}
          duration={duration}
          volume={volume}
          isMuted={isMuted}
          playbackRate={playbackRate}
          quality={demoMode ? quality : undefined}
          isFullscreen={isFullscreen}
          showControls={showControls}
          isBookmarked={isBookmarked}
          isBookmarkPending={isBookmarkPending}
          isBookmarkDisabled={!mediaItemId || sessionView.status !== "ready" || isBookmarkPending || (productionMode && !markersLoaded)}
          isBuffering={isBuffering}
          bufferedRanges={bufferedRanges}
          onPlayPause={togglePlay}
          onSeek={handleSeek}
          onVolumeChange={handleVolumeChange}
          onToggleMute={toggleMute}
          onRateChange={handleRateChange}
          onQualityChange={demoMode ? setQuality : undefined}
          onToggleFullscreen={toggleFullscreen}
          onBack={() => navigate(-1)}
          onOpenEpisodes={drawerEpisodes ? () => setIsSidePanelOpen(!isSidePanelOpen) : undefined}
          onToggleBookmark={toggleVideoBookmark}
        />
      </div>

      {/* 右侧：动漫/剧集选集与详情面板 */}
      {drawerEpisodes && (
        <EpisodeDrawer
          isOpen={isSidePanelOpen}
          onClose={() => setIsSidePanelOpen(false)}
          episodes={drawerEpisodes}
          currentEpisodeId={currentEpisodeId}
          castMediaItemId={currentEpisodeId || mediaItemId}
          onSelectEpisode={(ep) => {
            if (demoMode) {
              setDemoCurrentEpisodeId(ep.id)
              if (videoRef.current) {
                videoRef.current.currentTime = 0
                void videoRef.current.play()
                  .then(() => setIsPlaying(true))
                  .catch(() => setIsPlaying(false))
              }
              return
            }
            navigate(`/player/${ep.id}`)
          }}
          onOpenDetails={() => navigate(`/work/${state.status === "ready" ? state.session.workId : (mediaItemId || "4")}`)}
          mediaTitle={playerTitle}
          secondaryTitle={currentEp ? `${currentEp.number}` : (demoMode ? (videoData.subtitle || "正片") : undefined)}
          mediaYear={drawerMeta?.year}
          mediaDescription={drawerMeta?.description}
        />
      )}
    </div>
  )
}
