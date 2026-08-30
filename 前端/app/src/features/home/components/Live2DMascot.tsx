import { useState, useEffect, useRef, useCallback } from "react"
import { useNavigate } from "react-router"
import {
  Sparkles,
  Settings2,
  X,
  RefreshCw,
} from "lucide-react"
import { cn } from "@/lib/utils"
import {
  readMascotConfig,
  saveMascotConfig,
  getCharacterById,
  getTimeGreeting,
  type MascotConfig,
} from "@/lib/mascotState"
import { PokeBallWidget } from "./PokeBallWidget"
import {
  loadOptionalOml2dRuntime,
  type Oml2dRuntimeModule,
} from "./live2d-runtime"
import {
  bundledLive2dModelCatalog,
  type Live2dModelCatalog,
} from "@/lib/live2d/model-catalog"

type Oml2dInstance = ReturnType<Oml2dRuntimeModule["loadOml2d"]>
type Oml2dInternals = {
  pixiApp?: {
    unMount?: () => void
    app?: {
      destroy?: (
        removeView?: boolean,
        options?: { children?: boolean; texture?: boolean; baseTexture?: boolean },
      ) => void
    }
  }
  stage?: { unMount?: () => void }
  statusBar?: { unMount?: () => void }
  tips?: { unmount?: () => void }
  menus?: { unmount?: () => void }
}

const disposingOml2dInstances = new WeakSet<object>()

/**
 * oh-my-live2d 0.19.3 没有公开 destroy/dispose API。
 * 这里通过它实际创建的内部舞台和 Pixi 应用完成卸载，避免 React
 * StrictMode 或角色切换留下仍在运行的旧舞台。
 */
function disposeOml2d(instance: Oml2dInstance | null, isLoadComplete: () => boolean = () => true) {
  if (!instance || disposingOml2dInstances.has(instance)) return
  disposingOml2dInstances.add(instance)

  try {
    instance.stopTipsIdle()
    instance.clearTips()
    instance.statusBarClearEvents()
  } catch {
    // 实例可能还在异步初始化，清理操作需要保持幂等。
  }

  const internals = instance as unknown as Oml2dInternals
  let finalized = false

  const finalize = () => {
    if (finalized) return
    finalized = true

    internals.pixiApp?.unMount?.()
    internals.pixiApp?.app?.destroy?.(true, {
      children: true,
      texture: false,
      baseTexture: false,
    })
    internals.stage?.unMount?.()
    internals.statusBar?.unMount?.()
    internals.tips?.unmount?.()
    internals.menus?.unmount?.()
  }

  // 第三方库的模型加载链没有暴露 Promise。若在加载期间立即 destroy，
  // 它后续的内部 .then 会继续访问已销毁的 Pixi 应用，产生未处理 Promise。
  // 等 load 事件完成后再销毁，既能处理 StrictMode 清理，也不留下运行实例。
  if (isLoadComplete()) {
    window.setTimeout(finalize, 0)
    return
  }

  let fallbackTimer: number | undefined
  const finishAfterLoad = (status: "loading" | "success" | "fail") => {
    if (status === "loading") return
    if (fallbackTimer !== undefined) window.clearTimeout(fallbackTimer)
    window.setTimeout(finalize, 0)
  }
  instance.onLoad(finishAfterLoad)
  fallbackTimer = window.setTimeout(finalize, 15000)
}

interface Live2DMascotProps {
  modelCatalog?: Live2dModelCatalog
}

export function Live2DMascot({
  modelCatalog = bundledLive2dModelCatalog,
}: Live2DMascotProps = {}) {
  const navigate = useNavigate()
  const [config, setConfig] = useState<MascotConfig>(readMascotConfig)
  const [currentQuoteIndex, setCurrentQuoteIndex] = useState(0)
  const [bubbleText, setBubbleText] = useState("")
  const [showBubble, setShowBubble] = useState(true)
  const [isHovered, setIsHovered] = useState(false)
  const [isRecalling, setIsRecalling] = useState(false)
  const [showQuickMenu, setShowQuickMenu] = useState(false)
  const [unavailableModelUrl, setUnavailableModelUrl] = useState<string | null>(null)
  
  const containerRef = useRef<HTMLDivElement | null>(null)
  const stageRef = useRef<HTMLDivElement | null>(null)
  const oml2dRef = useRef<Oml2dInstance | null>(null)
  const bubbleTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const resolvedCharacter = modelCatalog.resolve(config.selectedCharacterId)
  const character = resolvedCharacter?.character ?? getCharacterById(config.selectedCharacterId)
  const live2dModel = resolvedCharacter?.model ?? null
  const modelEntryUrl = live2dModel?.entryUrl
  const modelId = live2dModel?.id
  const modelPositionX = live2dModel?.position[0]
  const modelPositionY = live2dModel?.position[1]
  const modelScale = live2dModel?.scale
  const runtimeUnavailable = unavailableModelUrl === modelEntryUrl

  // 真正加载 Live2D 模型（Cubism 2/3/4/5 由 oh-my-live2d 自动处理）
  useEffect(() => {
    if (
      !config.enabled
      || runtimeUnavailable
      || !modelEntryUrl
      || !modelId
      || modelPositionX === undefined
      || modelPositionY === undefined
      || modelScale === undefined
      || !stageRef.current
    ) return

    // 每个实例使用独立宿主。第三方库的异步加载完成后只会回写自己的
    // 宿主，即使 React StrictMode 已经清理过该实例，也不会污染新舞台。
    const stageContainer = stageRef.current
    const instanceHost = document.createElement("div")
    instanceHost.className = "haven-live2d-instance-host"
    stageContainer.replaceChildren(instanceHost)

    let disposed = false
    let loadComplete = false
    let oml2d: Oml2dInstance | null = null
    const runtimeAbortController = new AbortController()

    const markRuntimeUnavailable = () => {
      if (disposed) return
      if (oml2d) disposeOml2d(oml2d, () => loadComplete)
      if (oml2dRef.current === oml2d) oml2dRef.current = null
      instanceHost.replaceChildren()
      instanceHost.remove()
      setUnavailableModelUrl(modelEntryUrl)
    }

    void loadOptionalOml2dRuntime(
      undefined,
      undefined,
      undefined,
      runtimeAbortController.signal,
    ).then((runtime) => {
      if (disposed) return
      if (!runtime) {
        markRuntimeUnavailable()
        return
      }

      try {
        oml2d = runtime.loadOml2d({
          parentElement: instanceHost,
          dockedPosition: "right",
          primaryColor: "#007aff",
          stageStyle: {
            background: "transparent",
            backgroundColor: "transparent",
            border: "none",
            boxShadow: "none",
          },
          sayHello: false,
          transitionTime: 400,
          models: [
            {
              name: modelId,
              path: modelEntryUrl,
              scale: modelScale,
              position: [modelPositionX, modelPositionY],
              motionPreloadStrategy: "IDLE",
            },
          ],
          statusBar: {
            disable: true,
          },
          tips: {
            idleTips: {
              wordTheDay: false,
              interval: 0,
              message: [],
            },
            welcomeTips: {
              message: {
                daybreak: "",
                morning: "",
                noon: "",
                afternoon: "",
                dusk: "",
                night: "",
                lateNight: "",
                weeHours: "",
              },
            },
            copyTips: {
              message: [],
            },
          },
          menus: {
            items: [],
          },
        })
        oml2d.onLoad((status) => {
          if (status === "success" || status === "fail") loadComplete = true
          if (status === "fail") markRuntimeUnavailable()
        })
        oml2dRef.current = oml2d
      } catch {
        markRuntimeUnavailable()
      }
    })

    return () => {
      disposed = true
      runtimeAbortController.abort()
      if (oml2d) disposeOml2d(oml2d, () => loadComplete)
      if (oml2dRef.current === oml2d) oml2dRef.current = null
      instanceHost.replaceChildren()
      instanceHost.remove()
    }
  }, [
    config.enabled,
    modelEntryUrl,
    modelId,
    modelPositionX,
    modelPositionY,
    modelScale,
    runtimeUnavailable,
  ])

  // 监听外部配置变更 (如在 SettingsPage 中修改)
  useEffect(() => {
    const handleConfigChange = (e: Event) => {
      const customEvent = e as CustomEvent<MascotConfig>
      if (customEvent.detail) {
        setConfig(customEvent.detail)
      }
    }
    window.addEventListener("haven:mascot-changed", handleConfigChange)
    return () => window.removeEventListener("haven:mascot-changed", handleConfigChange)
  }, [])

  // 初始问候语与气泡逻辑
  useEffect(() => {
    if (!config.enabled || config.isRecalled) return
    const greeting = getTimeGreeting(character)
    setBubbleText(greeting)
    setShowBubble(true)

    if (bubbleTimerRef.current) clearTimeout(bubbleTimerRef.current)
    bubbleTimerRef.current = setTimeout(() => {
      setShowBubble(false)
    }, 7000)

    return () => {
      if (bubbleTimerRef.current) clearTimeout(bubbleTimerRef.current)
    }
  }, [config.enabled, config.isRecalled, character])

  // 点击触发台词轮播
  const handleCharacterClick = () => {
    const nextIdx = (currentQuoteIndex + 1) % character.quotes.length
    setCurrentQuoteIndex(nextIdx)
    setBubbleText(character.quotes[nextIdx])
    setShowBubble(true)

    if (bubbleTimerRef.current) clearTimeout(bubbleTimerRef.current)
    bubbleTimerRef.current = setTimeout(() => {
      setShowBubble(false)
    }, 6000)
  }

  // 召唤出球
  const handleSummon = () => {
    const next = saveMascotConfig({ isRecalled: false })
    setConfig(next)
    setBubbleText(getTimeGreeting(character))
    setShowBubble(true)
  }

  // 收回进精灵球
  const handleRecall = () => {
    setIsRecalling(true)
    setBubbleText("收回中……")
    setTimeout(() => {
      const next = saveMascotConfig({ isRecalled: true })
      setConfig(next)
      setIsRecalling(false)
    }, 500)
  }

  // 鼠标视线追踪微动效
  const handleMouseMove = useCallback((e: MouseEvent) => {
    if (!config.trackCursor || !stageRef.current) return
    const rect = stageRef.current.getBoundingClientRect()
    const centerX = rect.left + rect.width / 2
    const centerY = rect.top + rect.height / 2
    const deltaX = (e.clientX - centerX) / window.innerWidth
    const deltaY = (e.clientY - centerY) / window.innerHeight
    
    if (stageRef.current) {
      stageRef.current.style.transform = `perspective(600px) rotateY(${deltaX * 18}deg) rotateX(${-deltaY * 12}deg)`
    }
  }, [config.trackCursor])

  useEffect(() => {
    window.addEventListener("mousemove", handleMouseMove)
    return () => window.removeEventListener("mousemove", handleMouseMove)
  }, [handleMouseMove])

  if (!config.enabled || runtimeUnavailable || !live2dModel) return null

  return (
    <aside aria-label="伴读看板娘" className="fixed bottom-[72px] right-6 z-40 select-none flex flex-col items-end">
      {/* 收回状态显示精灵球；Live2D 舞台本身保持挂载，只做隐藏，避免重新召唤时重建实例。 */}
      {config.isRecalled && (
        <PokeBallWidget
          ballType={config.ballType}
          characterName={character.name}
          onSummon={handleSummon}
        />
      )}

      <div
        ref={containerRef}
        aria-hidden={config.isRecalled}
        onMouseEnter={() => setIsHovered(true)}
        onMouseLeave={() => {
          setIsHovered(false)
          setShowQuickMenu(false)
        }}
        className={cn(
          "relative flex flex-col items-end transition-all duration-500",
          config.isRecalled
            ? // 绝对定位脱离文档流，让 aside 收缩为精灵球大小，球正好落在角色原本的右下角位置
              "pointer-events-none invisible absolute right-0 bottom-0 scale-95 opacity-0"
            : isRecalling
              ? "scale-0 opacity-0 blur-sm brightness-200 translate-y-[48px]"
              : "scale-100 opacity-100"
        )}
      >
          {/* 角色对话框气泡 */}
          {(showBubble || isHovered) && (
            <div
              className={cn(
                "relative mb-[8px] max-w-[240px] sm:max-w-[280px] rounded-3xl p-3.5 shadow-2xl border backdrop-blur-2xl transition-all duration-300 animate-in fade-in zoom-in-95",
                "bg-background/95 border-border/80 text-foreground"
              )}
            >
              <div className="flex items-start justify-between gap-[8px]">
                <div className="flex items-center gap-1.5 text-[11px] font-extrabold text-primary">
                  <Sparkles size={13} className="shrink-0" />
                  <span>{character.name}</span>
                </div>
                <button
                  type="button"
                  onClick={() => setShowBubble(false)}
                  className="text-muted-foreground/60 hover:text-foreground cursor-pointer"
                  aria-label="关闭气泡"
                >
                  <X size={12} />
                </button>
              </div>

              <p className="mt-1.5 text-xs font-medium leading-relaxed">
                {bubbleText || character.quotes[0]}
              </p>

              {/* 气泡尖角 */}
              <div className="absolute -bottom-1.5 right-[48px] h-3 w-3 rotate-45 border-r border-b border-border/80 bg-background/95" />
            </div>
          )}

          {/* 角色微型交互 Dock (悬停时浮出) */}
          <div
            className={cn(
              "flex items-center gap-1.5 rounded-full border border-border/80 bg-background/90 px-3 py-1 shadow-lg backdrop-blur-xl mb-1 transition-all duration-300",
              isHovered || showQuickMenu ? "opacity-100 translate-y-0" : "opacity-0 translate-y-[8px] pointer-events-none"
            )}
          >
            {/* 收回进精灵球按键 */}
            <button
              type="button"
              onClick={handleRecall}
              title="收回进宝可梦球"
              className="flex items-center gap-1 text-[11px] font-bold text-red-500 hover:text-red-600 px-[8px] py-0.5 rounded-full hover:bg-red-500/10 transition-colors cursor-pointer"
            >
              <div className="h-3 w-3 rounded-full bg-red-500 border border-white shrink-0" />
              <span>收回</span>
            </button>

            <div className="h-3 w-px bg-border/60" />

            {/* 换一句台词 */}
            <button
              type="button"
              onClick={handleCharacterClick}
              title="换一句台词"
              className="flex items-center gap-1 text-[11px] font-semibold text-muted-foreground hover:text-foreground px-1.5 py-0.5 rounded-full hover:bg-muted transition-colors cursor-pointer"
            >
              <RefreshCw size={11} />
              <span>互动</span>
            </button>

            {/* 打开设置 */}
            <button
              type="button"
              onClick={() => navigate("/settings")}
              title="去设置页更换角色与精灵球"
              className="flex items-center gap-1 text-[11px] font-semibold text-muted-foreground hover:text-foreground px-1.5 py-0.5 rounded-full hover:bg-muted transition-colors cursor-pointer"
            >
              <Settings2 size={11} />
              <span>设置</span>
            </button>
          </div>

          {/* 角色立体视效与 Live2D 舞台 */}
          <div
            onClick={handleCharacterClick}
            className="relative cursor-pointer transition-transform duration-300 hover:scale-105 active:scale-95"
            style={{ width: 220, height: 260 }}
          >
            {/* Live2D 模型挂载点（oh-my-live2d 在此创建 WebGL 舞台） */}
            <div
              ref={stageRef}
              className="haven-live2d-stage relative w-full h-full flex items-end justify-center overflow-hidden bg-transparent"
              style={{
                filter: "drop-shadow(0 10px 25px rgba(0,0,0,0.25))",
              }}
            />
          </div>
      </div>
    </aside>
  )
}
