export type BallType = "poke" | "great" | "ultra" | "master"

export interface Live2DCharacter {
  id: string
  name: string
  source: string
  tag: string
  color: string
  avatarUrl: string
  yOffset: number
  quotes: string[]
  greetings: {
    morning: string
    afternoon: string
    night: string
  }
}

export interface MascotConfig {
  enabled: boolean
  isRecalled: boolean // 是否已收回进精灵球
  selectedCharacterId: string
  ballType: BallType
  trackCursor: boolean
  enableVoice: boolean
  showBubble: boolean
  showHistoryHint: boolean
  scale: number
}

// ==========================================
// 开源 Live2D 角色库 (Cubism 2, live2d-widget 系列模型)
// ==========================================
export const CHARACTER_REGISTRY: Live2DCharacter[] = [
  {
    id: "miku",
    name: "Hatsune Miku",
    source: "VOCALOID · Crypton Future Media",
    tag: "虚拟歌姬 · 元气伴读",
    color: "#38bdf8",
    avatarUrl: "/avatars/miku.jpg",
    yOffset: 20,
    quotes: [
      "世界第一的虚拟歌姬登场！今天想听哪一首歌？",
      "啦啦啦~ 栖阅的媒体库里藏着好多宝藏呢！",
      "累的时候，让我的歌声为你充电吧！",
      "刚才偷偷练习了新曲，要不要听一遍？",
      "无论多少次，歌声都能传递心情哦！",
    ],
    greetings: {
      morning: "早安！元气满满的一天，从一首轻快的歌开始吧！",
      afternoon: "午后犯困的话，听首歌放松一下如何？",
      night: "夜深了，愿轻柔的旋律伴你入眠~",
    },
  },
  {
    id: "shizuku",
    name: "Shizuku",
    source: "Live2D 官方示例模型",
    tag: "官方示例 · 恬静伴读",
    color: "#fbbf24",
    avatarUrl: "/avatars/shizuku.jpg",
    yOffset: 30,
    quotes: [
      "你好，我是静。今天想一起安静地看点什么吗？",
      "阅读的时光总是过得很快呢。",
      "要不要试试边听轻音乐边看书？",
      "你的媒体库品味，我一直很欣赏哦。",
      "累了就休息一下，我会在这里陪着你。",
    ],
    greetings: {
      morning: "早安。清晨适合翻开一本新书。",
      afternoon: "午后阳光正好，适合小憩或阅读。",
      night: "夜深了，请早点休息，明天继续。",
    },
  },
  {
    id: "koharu",
    name: "Koharu",
    source: "Live2D 示例模型",
    tag: "双马尾 · 可爱伴读",
    color: "#f472b6",
    avatarUrl: "/avatars/koharu.jpg",
    yOffset: 25,
    quotes: [
      "喵~ 今天也元气满满地陪你看书！",
      "这本书看起来好有趣，小春也想听！",
      "摸摸头会让我更有干劲哦！",
      "嘿嘿，猜猜小春今天偷偷准备了什么？",
      "就算天塌下来，小春也会陪着你！",
    ],
    greetings: {
      morning: "早上好喵！快起来和小春一起探索吧！",
      afternoon: "下午茶时间到，小春给你泡了红茶~",
      night: "夜深了喵，盖好被子，小春守着你。",
    },
  },
  {
    id: "unitychan",
    name: "Unity-chan",
    source: "Unity 官方吉祥物",
    tag: "引擎娘 · 元气守护",
    color: "#f59e0b",
    avatarUrl: "/avatars/unitychan.jpg",
    yOffset: 20,
    quotes: [
      "Unity 酱报道！今天的学习计划准备好了吗？",
      "无论是游戏还是书，都要认真对待哦！",
      "遇到问题不要慌，先喝口水再想想~",
      "你的专注力，就是最强的引擎！",
      "偶尔放空也很重要，我陪你休息一下。",
    ],
    greetings: {
      morning: "早安！新的一天，从元气满满开始！",
      afternoon: "下午啦，注意劳逸结合哦！",
      night: "夜深了，今天也辛苦啦，好好休息。",
    },
  },
  {
    id: "z16",
    name: "Z16",
    source: "Live2D 官方示例模型",
    tag: "官方高精旗舰 · 灵动少女",
    color: "#a78bfa",
    avatarUrl: "/avatars/z16.jpg",
    yOffset: 20,
    quotes: [
      "嗨，很高兴在这里遇见你~",
      "今天有什么想看的电影或书吗？",
      "屏幕里的世界很大，我陪你去探索。",
      "盯着屏幕太久会累，记得眨眨眼。",
      "我的视线一直跟着你哦，动动鼠标试试~",
    ],
    greetings: {
      morning: "晨光正好，开启美好的一天吧！",
      afternoon: "午后时光，适合慢慢享受。",
      night: "星夜入梦，愿你好眠。",
    },
  },
]

export const POKEBALL_SKINS = [
  { id: "poke", name: "精灵球 (Poké Ball)", desc: "最经典的红白配色彩球", color: "#ee1515" },
  { id: "great", name: "超级球 (Great Ball)", desc: "红蓝相间的强化捕捉球", color: "#007aff" },
  { id: "ultra", name: "高级球 (Ultra Ball)", desc: "黑金搭配的高性能球", color: "#ffcc00" },
  { id: "master", name: "大师球 (Master Ball)", desc: "必定捕捉的至尊紫球", color: "#a855f7" },
] as const

const MASCOT_CONFIG_KEY = "haven:mascot-config"

export const DEFAULT_MASCOT_CONFIG: MascotConfig = {
  enabled: true,
  isRecalled: false, // 默认展开召唤状态
  selectedCharacterId: "miku",
  ballType: "poke",
  trackCursor: true,
  enableVoice: true,
  showBubble: true,
  showHistoryHint: true,
  scale: 1,
}

export function readMascotConfig(): MascotConfig {
  try {
    const raw = localStorage.getItem(MASCOT_CONFIG_KEY)
    if (!raw) return DEFAULT_MASCOT_CONFIG
    return { ...DEFAULT_MASCOT_CONFIG, ...JSON.parse(raw) }
  } catch {
    return DEFAULT_MASCOT_CONFIG
  }
}

export function saveMascotConfig(config: Partial<MascotConfig>): MascotConfig {
  const current = readMascotConfig()
  const next = { ...current, ...config }
  try {
    localStorage.setItem(MASCOT_CONFIG_KEY, JSON.stringify(next))
    // 派发自定义全局事件以便跨组件即时响应
    window.dispatchEvent(new CustomEvent("haven:mascot-changed", { detail: next }))
  } catch (e) {
    console.error("Failed to save mascot config:", e)
  }
  return next
}

export function getCharacterById(id: string): Live2DCharacter {
  return CHARACTER_REGISTRY.find((c) => c.id === id) || CHARACTER_REGISTRY[0]
}

export function getTimeGreeting(char: Live2DCharacter): string {
  const hour = new Date().getHours()
  if (hour >= 5 && hour < 12) return char.greetings.morning
  if (hour >= 12 && hour < 18) return char.greetings.afternoon
  return char.greetings.night
}
