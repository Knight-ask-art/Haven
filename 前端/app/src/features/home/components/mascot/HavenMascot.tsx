import { useEffect, useRef } from "react"
import { GrokCharacter } from "./character"

interface HavenMascotProps {
  isDark?: boolean
  className?: string
}

export function HavenMascot({ isDark = false, className = "" }: HavenMascotProps) {
  const svgRef = useRef<SVGSVGElement>(null)
  const mascotRef = useRef<GrokCharacter | null>(null)
  const isSleepingRef = useRef<boolean>(false)
  const sleepTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const clickCountRef = useRef<number>(0)
  const clickResetTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const longPressTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const isLongPressRef = useRef<boolean>(false)

  // 鼠标画圈/快速晃动检测
  const mouseTrailRef = useRef<{ x: number; y: number; t: number }[]>([])

  useEffect(() => {
    const svgEl = svgRef.current
    if (!svgEl) return

    // 1. 初始化 GrokCharacter
    const mascot = new GrokCharacter(svgEl, {
      shape: "cloud",
      color: "grad-lavender-ice",
      mode: "hold",
      followPointer: true,
      scheme: "light",
      loginWrap: false,
      pose: { turn: 0, tilt: 0, roll: 0, scale: 1 },
      poseHome: { turn: 0, tilt: 0, roll: 0 },
      faceTune: { size: 1, gap: 1, height: 1, eyeWidth: 1, eyeHeight: 1 },
    })
    mascotRef.current = mascot

    // 首页云朵不要四周的彩色纸屑/星星粒子：屏蔽全部爆发粒子，仅保留动作、表情与呼吸动画
    mascot.particles.burst = () => {}

    // 2. 注入 薰衣草碎冰 (Lavender Ice) 渐变到 SVG defs
    let defs = svgEl.querySelector("defs")
    if (!defs) {
      defs = document.createElementNS("http://www.w3.org/2000/svg", "defs")
      svgEl.insertBefore(defs, svgEl.firstChild)
    }
    defs.insertAdjacentHTML("beforeend", `
      <linearGradient id="grad-lavender-ice" x1="0%" y1="0%" x2="100%" y2="100%">
        <stop offset="0%" stop-color="#e0c3fc" />
        <stop offset="100%" stop-color="#8ec5fc" />
      </linearGradient>
    `)

    // 3. 设定为 cloud
    mascot.setShape("cloud")
    mascot.setState("idle", { resetEyes: true })

    // 5. 🌟 5 分钟无操作自动休眠倒计时
    const SLEEP_IDLE_TIMEOUT = 5 * 60 * 1000 // 5 分钟

    const goToSleep = () => {
      isSleepingRef.current = true
      mascot.setState("sleeping", { resetEyes: true })
    }

    const resetSleepTimer = () => {
      if (sleepTimerRef.current) clearTimeout(sleepTimerRef.current)
      if (!isSleepingRef.current) {
        sleepTimerRef.current = setTimeout(() => {
          goToSleep()
        }, SLEEP_IDLE_TIMEOUT)
      }
    }

    const wakeUp = () => {
      isSleepingRef.current = false
      mascot.setState("waking")
      // 3.2s 伸懒腰眨眼动画完整播完后切回待机
      setTimeout(() => {
        if (!isSleepingRef.current) {
          mascot.setState("idle", { resetEyes: true })
        }
      }, 3200)
      resetSleepTimer()
    }

    // 全局活动重置休眠倒计时
    const handleGlobalActivity = () => {
      if (!isSleepingRef.current) resetSleepTimer()
    }
    window.addEventListener("pointermove", handleGlobalActivity, { passive: true })
    window.addEventListener("keydown", handleGlobalActivity, { passive: true })
    resetSleepTimer()

    // 6. 🌟 鼠标交互系统 (单击 50/50 跳/转、双击大笑、长按害羞、狂点生气、画圈逗玩)
    const handlePointerDown = () => {
      isLongPressRef.current = false
      longPressTimerRef.current = setTimeout(() => {
        isLongPressRef.current = true
        if (!isSleepingRef.current) {
          mascot.setState("shy")
          setTimeout(() => {
            if (!isSleepingRef.current) mascot.setState("idle")
          }, 2400)
        }
      }, 600)
    }

    const handlePointerUp = () => {
      if (longPressTimerRef.current) {
        clearTimeout(longPressTimerRef.current)
        longPressTimerRef.current = null
      }
    }

    const handleClick = (e: MouseEvent) => {
      e.stopPropagation()
      if (isLongPressRef.current) {
        isLongPressRef.current = false
        return
      }

      // 如果正在睡觉，点击唤醒
      if (isSleepingRef.current) {
        wakeUp()
        return
      }

      // 连击计数 (狂点生气)
      clickCountRef.current += 1
      if (clickResetTimerRef.current) clearTimeout(clickResetTimerRef.current)
      clickResetTimerRef.current = setTimeout(() => {
        clickCountRef.current = 0
      }, 900)

      if (clickCountRef.current >= 6) {
        mascot.setState("angry")
        clickCountRef.current = 0
        setTimeout(() => {
          if (!isSleepingRef.current) mascot.setState("idle")
        }, 2200)
        return
      }

      // 单击：50% 弹跳，50% 旋转
      if (Math.random() < 0.5) {
        mascot.bounceOnce()
      } else {
        mascot.spinOnce(Math.random() < 0.5 ? 1 : -1)
      }
      resetSleepTimer()
    }

    const handleDoubleClick = (e: MouseEvent) => {
      e.stopPropagation()
      if (isSleepingRef.current) {
        wakeUp()
        return
      }
      mascot.setState("laughing")
      setTimeout(() => {
        if (!isSleepingRef.current) mascot.setState("idle")
      }, 2500)
    }

    // 鼠标在吉祥物区域剧烈画圈时触发 playful (顽皮)
    const handleMascotPointerMove = (e: PointerEvent) => {
      const now = performance.now()
      mouseTrailRef.current.push({ x: e.clientX, y: e.clientY, t: now })
      mouseTrailRef.current = mouseTrailRef.current.filter((p) => now - p.t < 600)
      if (mouseTrailRef.current.length > 18 && !isSleepingRef.current) {
        let totalDist = 0
        for (let i = 1; i < mouseTrailRef.current.length; i++) {
          const dx = mouseTrailRef.current[i].x - mouseTrailRef.current[i - 1].x
          const dy = mouseTrailRef.current[i].y - mouseTrailRef.current[i - 1].y
          totalDist += Math.hypot(dx, dy)
        }
        if (totalDist > 400 && mascot.state === "idle") {
          mascot.setState("playful")
          mascot.spinOnce(1)
          setTimeout(() => {
            if (!isSleepingRef.current) mascot.setState("idle")
          }, 2600)
          mouseTrailRef.current = []
        }
      }
    }

    svgEl.addEventListener("pointerdown", handlePointerDown)
    svgEl.addEventListener("pointermove", handleMascotPointerMove as unknown as EventListener)
    svgEl.addEventListener("click", handleClick)
    svgEl.addEventListener("dblclick", handleDoubleClick)
    window.addEventListener("pointerup", handlePointerUp)

    return () => {
      if (sleepTimerRef.current) clearTimeout(sleepTimerRef.current)
      if (clickResetTimerRef.current) clearTimeout(clickResetTimerRef.current)
      if (longPressTimerRef.current) clearTimeout(longPressTimerRef.current)
      window.removeEventListener("pointermove", handleGlobalActivity)
      window.removeEventListener("keydown", handleGlobalActivity)
      window.removeEventListener("pointerup", handlePointerUp)
      svgEl.removeEventListener("pointerdown", handlePointerDown)
      svgEl.removeEventListener("pointermove", handleMascotPointerMove as unknown as EventListener)
      svgEl.removeEventListener("click", handleClick)
      svgEl.removeEventListener("dblclick", handleDoubleClick)
      mascot.destroy()
    }
  }, [])

  // 响应主题切换
  useEffect(() => {
    const svgEl = svgRef.current
    if (!svgEl) return
    const eyeBgColor = isDark ? "#222222" : "#fdfbf7"
    svgEl.style.setProperty("--fg", "url(#grad-lavender-ice)")
    svgEl.style.setProperty("--bg", eyeBgColor)
    svgEl.style.setProperty("--sand-bg-base", isDark ? "#171717" : "#f7f7f5")
  }, [isDark])

  return (
    <div
      className={`relative inline-flex items-center justify-center select-none pointer-events-auto cursor-pointer ${className}`}
      title="栖阅云朵伴读 · 单击跳跃/旋转 · 双击大笑 · 长按害羞 · 5分钟休眠"
    >
      <svg
        ref={svgRef}
        className="w-full h-full block filter drop-shadow-[0_16px_36px_rgba(161,140,209,0.18)] dark:drop-shadow-[0_16px_36px_rgba(161,140,209,0.12)] transition-transform duration-300 hover:scale-[1.03]"
        viewBox="-15 -15 259 259"
        role="img"
        aria-label="栖阅云朵伴读吉祥物"
      />
    </div>
  )
}
