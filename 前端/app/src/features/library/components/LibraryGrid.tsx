
import { MediaItem } from "./MediaItem"
import type { LibraryMediaItemData } from "./MediaItem"
import { useNavigate } from "react-router"
import { ArtworkImage } from "@/components/ui/haven/ArtworkImage"
import { defaultCoverCategoryForMediaType } from "@/lib/default-cover"
import { getHavenClientMode } from "@/lib/ipc/runtime"

export type SortOption = "date" | "name" | "year"
export type ViewMode = "grid" | "list"

interface LibraryGridProps {
  category: string
  sortBy?: SortOption
  viewMode?: ViewMode
  searchQuery?: string
  density?: "regular" | "compact"
  onHoverItem?: (item: LibraryMediaItemData) => void
  /** 数据源注入（IPC-MOCK-001）：缺省回落共享 Mock（搜索/下载页迁移前保持原行为）。 */
  items?: LibraryMediaItemData[]
}

// 丰富的影视、图书、漫画、报刊、资料综合演示数据库
export const REPRESENTATIVE_ITEMS: LibraryMediaItemData[] = [
  // 影视
  { 
    id: "2", 
    title: "沙丘2 Dune: Part Two", 
    originalTitle: "Dune: Part Two (2024)",
    type: "movie", 
    year: 2024, 
    rating: "8.9",
    badge: "4K HDR",
    imageUrl: "https://picsum.photos/seed/dune/400/600",
    backdropUrl: "https://images.unsplash.com/photo-1509198397868-475647b2a1e5?q=80&w=2560&auto=format&fit=crop",
    description: "保罗·厄崔迪公爵与契妮以及弗雷曼人会合后展开的复仇旅程。面对关于终极命运的抉择，他必须在深爱的挚爱与已知的宇宙命运之间做出艰难取舍。"
  },
  { 
    id: "10", 
    title: "星际穿越 Interstellar", 
    originalTitle: "Interstellar (2014)",
    type: "movie", 
    year: 2014, 
    rating: "9.4",
    badge: "4K Remux",
    imageUrl: "https://picsum.photos/seed/interstellar/400/600",
    backdropUrl: "https://images.unsplash.com/photo-1462331940025-496dfbfc7564?q=80&w=2560&auto=format&fit=crop",
    description: "前 NASA 宇航员库珀受命带领一组探险家利用最新发现的虫洞穿越星际，寻找适合人类生存的全新栖息地。"
  },
  { 
    id: "6", 
    title: "奥本海默 Oppenheimer", 
    originalTitle: "Oppenheimer (2023)",
    type: "movie", 
    year: 2023, 
    rating: "8.8",
    badge: "1080P",
    imageUrl: "https://picsum.photos/seed/oppenheimer/400/600",
    backdropUrl: "https://images.unsplash.com/photo-1446776811953-b23d57bd21aa?q=80&w=2560&auto=format&fit=crop",
    description: "讲述美国理论物理学家罗伯特·奥本海默主导制造人类第一颗原子弹的曼哈顿计划幕后故事。"
  },
  { 
    id: "4", 
    title: "怪奇物语：1985故事集 第一季", 
    originalTitle: "Stranger Things Season 1",
    type: "tv", 
    year: 2024, 
    rating: "9.2",
    badge: "1080P",
    imageUrl: "https://picsum.photos/seed/spider/400/600",
    backdropUrl: "https://images.unsplash.com/photo-1578632767115-351597cf2477?q=80&w=2560&auto=format&fit=crop",
    description: "在一个小镇上，一个男孩神秘失踪，他的朋友、家人和当地警察开始寻找答案，卷入政府秘密实验与超自然谜团。"
  },

  // 图书
  { 
    id: "1", 
    title: "Apple Design Resources (iOS 18)", 
    originalTitle: "Official Human Interface Guidelines",
    type: "book", 
    year: 2024, 
    rating: "9.8",
    badge: "EPUB",
    imageUrl: "https://picsum.photos/seed/apple/400/600",
    backdropUrl: "https://images.unsplash.com/photo-1518770660439-4636190af475?q=80&w=2560&auto=format&fit=crop",
    description: "系统展示 Apple 官方 iOS 18 UI 交互设计规范与空间计算设计指导原则，包含 Swift UI 组件库与视觉动效指南。"
  },
  { 
    id: "3", 
    title: "史蒂夫·乔布斯传", 
    originalTitle: "Steve Jobs - Official Biography",
    type: "book", 
    year: 2011, 
    rating: "9.5",
    badge: "EPUB",
    imageUrl: "https://picsum.photos/seed/steve/400/600",
    backdropUrl: "https://images.unsplash.com/photo-1518770660439-4636190af475?q=80&w=2560&auto=format&fit=crop",
    description: "乔布斯唯一授权官方传记，记录了他颠覆个人电脑、动画电影、音乐与数字出版六大产业的传奇人生。"
  },
  { 
    id: "5", 
    title: "程序员修炼之道 (第2版)", 
    originalTitle: "The Pragmatic Programmer",
    type: "book", 
    year: 2019, 
    rating: "9.6",
    badge: "PDF",
    imageUrl: "https://picsum.photos/seed/pragmatic/400/600",
    backdropUrl: "https://images.unsplash.com/photo-1518770660439-4636190af475?q=80&w=2560&auto=format&fit=crop",
    description: "软件开发领域的传世经典，涵盖代码重构、测试驱动开发、敏捷实践与个人职业成长哲学。"
  },

  // 漫画 (Comic / Manga)
  { 
    id: "comic-aot", 
    title: "进击的巨人 漫画全集 (Vol.1-34)", 
    originalTitle: "Attack on Titan",
    type: "comic", 
    year: 2021, 
    rating: "9.6",
    badge: "CBZ",
    imageUrl: "https://images.unsplash.com/photo-1542451313056-b7c8e626645f?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1542451313056-b7c8e626645f?q=80&w=2560&auto=format&fit=crop",
    description: "百年之前，巨人的突然出现将人类逼入三道巨大城墙之中。艾伦·耶格尔立誓要将巨人从这个世界上彻底驱逐。"
  },
  { 
    id: "c2", 
    title: "迷宫饭 (单行本 1-14 卷全集)", 
    originalTitle: "Delicious in Dungeon",
    type: "comic", 
    year: 2024, 
    rating: "9.4",
    badge: "CBZ",
    imageUrl: "https://images.unsplash.com/photo-1607604276583-eef5d076aa5f?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1607604276583-eef5d076aa5f?q=80&w=2560&auto=format&fit=crop",
    description: "冒险者莱欧斯为了解救被红龙吞食的妹妹，率领小队深入迷宫，决定：吃掉迷宫里的魔物！"
  },
  { 
    id: "c3", 
    title: "葬送的芙莉莲 (第 1-13 卷)", 
    originalTitle: "Frieren: Beyond Journey's End",
    type: "comic", 
    year: 2024, 
    rating: "9.7",
    badge: "Manga",
    imageUrl: "https://images.unsplash.com/photo-1578632767115-351597cf2477?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1578632767115-351597cf2477?q=80&w=2560&auto=format&fit=crop",
    description: "打倒魔王后的长寿精灵芙莉莲，踏上了解人类感情与追忆昔日勇者伙伴的全新旅程。"
  },

  // 报刊 (Periodical)
  { 
    id: "p1", 
    title: "National Geographic 国家地理杂志 (2024合订本)", 
    originalTitle: "National Geographic Magazine",
    type: "periodical", 
    year: 2024, 
    rating: "9.8",
    badge: "PDF",
    imageUrl: "https://images.unsplash.com/photo-1544716278-ca5e3f4abd8c?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1544716278-ca5e3f4abd8c?q=80&w=2560&auto=format&fit=crop",
    description: "探索自然界的奇观、深海未解之谜与人类文明的史诗篇章，随顶尖摄影师镜头穿梭极地与热带雨林。"
  },
  { 
    id: "p2", 
    title: "The Economist 经济学人 (2024年8月刊)", 
    originalTitle: "The Economist Weekly",
    type: "periodical", 
    year: 2024, 
    rating: "9.7",
    badge: "PDF",
    imageUrl: "https://images.unsplash.com/photo-1504711434969-e33886168f5c?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1504711434969-e33886168f5c?q=80&w=2560&auto=format&fit=crop",
    description: "汇聚全球政治、商业、科技与金融领域的深度分析与前瞻洞察，厘清复杂多变的全球局势。"
  },
  { 
    id: "p3", 
    title: "WIRED 连线杂志：Agentic AI 封面特刊", 
    originalTitle: "WIRED Magazine 2024",
    type: "periodical", 
    year: 2024, 
    rating: "9.5",
    badge: "Magazine",
    imageUrl: "https://images.unsplash.com/photo-1585829365295-ab7cd400c167?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1585829365295-ab7cd400c167?q=80&w=2560&auto=format&fit=crop",
    description: "聚焦前沿人工智能、硅谷创业风向与数字文化变迁的全球权威科技文化期刊。"
  },
  {
    id: "p4",
    title: "Monocle 城市与设计特刊",
    originalTitle: "Monocle: The Urbanist Issue",
    type: "periodical",
    year: 2024,
    rating: "9.4",
    badge: "PDF",
    imageUrl: "https://images.unsplash.com/photo-1497366811353-6870744d04b2?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1497366811353-6870744d04b2?q=80&w=2560&auto=format&fit=crop",
    description: "关于城市生活、建筑、设计与新商业的独立观察，记录全球城市正在发生的细微变化。"
  },
  {
    id: "p5",
    title: "Science 科学杂志 2024 合订本",
    originalTitle: "Science Magazine 2024 Archive",
    type: "periodical",
    year: 2024,
    rating: "9.6",
    badge: "ARCHIVE",
    imageUrl: "https://images.unsplash.com/photo-1532094349884-543bc11b234d?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1532094349884-543bc11b234d?q=80&w=2560&auto=format&fit=crop",
    description: "汇集生命科学、物理、气候与空间探索领域的年度重要研究与前沿观点。"
  },
  {
    id: "p6",
    title: "The New Yorker 文化观察特刊",
    originalTitle: "The New Yorker: Culture Issue",
    type: "periodical",
    year: 2024,
    rating: "9.3",
    badge: "MAGAZINE",
    imageUrl: "https://images.unsplash.com/photo-1504711434969-e33886168f5c?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1504711434969-e33886168f5c?q=80&w=2560&auto=format&fit=crop",
    description: "以长篇报道、文化评论和插画记录当代社会，适合慢读与长期收藏。"
  },

  // 资料 (Document)
  { 
    id: "d1", 
    title: "Apple Human Interface Guidelines (iOS 18 规范)", 
    originalTitle: "iOS 18 Design Specifications",
    type: "document", 
    year: 2024, 
    rating: "9.9",
    badge: "Doc Specs",
    imageUrl: "https://images.unsplash.com/photo-1518770660439-4636190af475?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1518770660439-4636190af475?q=80&w=2560&auto=format&fit=crop",
    description: "Apple 官方最新发布的 iOS 18 与 visionOS 2 交互设计指南，涵盖动效曲线、组件边界约束与视觉规制。"
  },
  { 
    id: "d2", 
    title: "DeepMind AlphaFold 3 论文与算法白皮书", 
    originalTitle: "AlphaFold 3 Architecture Review",
    type: "document", 
    year: 2024, 
    rating: "9.9",
    badge: "Paper",
    imageUrl: "https://images.unsplash.com/photo-1451187580459-43490279c0fa?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1451187580459-43490279c0fa?q=80&w=2560&auto=format&fit=crop",
    description: "阐述 AlphaFold 3 针对蛋白质、DNA、RNA 与小分子配体互作预测的扩散模型架构与白皮书。"
  },
  {
    id: "d3",
    title: "Web Platform Baseline 2024",
    originalTitle: "Web Platform Baseline Technical Report",
    type: "document",
    year: 2024,
    rating: "9.6",
    badge: "REPORT",
    imageUrl: "https://images.unsplash.com/photo-1558494949-ef010cbdcc31?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1558494949-ef010cbdcc31?q=80&w=2560&auto=format&fit=crop",
    description: "整理现代 Web 平台的能力基线、浏览器支持状态与面向产品团队的渐进增强策略。"
  },
  {
    id: "d4",
    title: "Kubernetes Patterns 实践手册",
    originalTitle: "Kubernetes Patterns: Design and Deployment",
    type: "document",
    year: 2023,
    rating: "9.5",
    badge: "PDF",
    imageUrl: "https://images.unsplash.com/photo-1667372393119-3d4c48d07fc9?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1667372393119-3d4c48d07fc9?q=80&w=2560&auto=format&fit=crop",
    description: "从容器编排、服务发现到可观测性，提炼云原生系统中可复用的架构模式。"
  },
  {
    id: "d5",
    title: "React 19 设计与迁移指南",
    originalTitle: "React 19 Architecture Notes",
    type: "document",
    year: 2024,
    rating: "9.4",
    badge: "DOC SPECS",
    imageUrl: "https://images.unsplash.com/photo-1555066931-4365d14bab8c?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1555066931-4365d14bab8c?q=80&w=2560&auto=format&fit=crop",
    description: "面向工程团队的 React 19 新能力、并发渲染和组件迁移规范，强调可维护的边界设计。"
  },
  {
    id: "d6",
    title: "本地优先应用架构白皮书",
    originalTitle: "Local-First Software Architecture",
    type: "document",
    year: 2025,
    rating: "9.7",
    badge: "WHITEPAPER",
    imageUrl: "https://images.unsplash.com/photo-1516321318423-f06f85e504b3?q=80&w=400&auto=format&fit=crop",
    backdropUrl: "https://images.unsplash.com/photo-1516321318423-f06f85e504b3?q=80&w=2560&auto=format&fit=crop",
    description: "讨论离线优先、端侧数据所有权、同步冲突与可恢复状态，为个人软件提供清晰的技术路线。"
  }
]

const getCategoryLabel = (type: string) => {
  switch (type) {
    case "movie": return "影视"
    case "tv": return "剧集"
    case "book": return "图书"
    case "comic": return "漫画"
    case "periodical": return "报刊"
    case "document": return "资料"
    default: return "媒体"
  }
}

export function LibraryGrid({
  category,
  sortBy = "date",
  viewMode = "grid",
  searchQuery = "",
  density = "regular",
  onHoverItem,
  items: itemsProp
}: LibraryGridProps) {
  const navigate = useNavigate()
  const allowExternal = getHavenClientMode() !== "tauri"

  const matchesCategory = (item: LibraryMediaItemData) => {
    if (category === "all") return true
    if (category === "video") return item.type === "movie" || item.type === "tv"
    if (category === "periodical") return item.type === "periodical"
    return item.type === category
  }

  const sourceItems = itemsProp ?? REPRESENTATIVE_ITEMS
  let items = sourceItems.filter(matchesCategory)

  if (searchQuery.trim() !== "") {
    const q = searchQuery.toLowerCase()
    items = items.filter((item) => 
      item.title.toLowerCase().includes(q) || 
      item.originalTitle?.toLowerCase().includes(q) ||
      item.type.toLowerCase().includes(q)
    )
  }

  // Sort logic: date 保持 REPRESENTATIVE_ITEMS 原始顺序（即真实添加顺序），name/year 才重排
  if (sortBy === "name") {
    items = [...items].sort((a, b) => a.title.localeCompare(b.title))
  } else if (sortBy === "year") {
    items = [...items].sort((a, b) => b.year - a.year)
  }

  if (items.length === 0) {
    return (
      <div className="py-[64px] text-center text-muted-foreground flex flex-col items-center justify-center">
        <p className="text-base font-bold text-foreground mb-1">未找到符合条件的内容</p>
        <p className="text-xs">您可以尝试更换搜索词或选择其他分类。</p>
      </div>
    )
  }

  if (viewMode === "list") {
    return (
      <div className="flex flex-col gap-3 w-full max-w-5xl">
        {items.map((item) => (
          <div 
            key={item.id} 
            onClick={() => navigate(`/work/${item.id}`, { state: { favorite: item.favorite } })}
            onMouseEnter={() => onHoverItem?.(item)}
            className="flex items-center gap-6 p-3.5 rounded-2xl bg-black/5 dark:bg-white/5 hover:bg-black/10 dark:hover:bg-white/10 transition-all cursor-pointer group border border-black/5 dark:border-white/5"
          >
            {/* Thumbnail */}
            <div className="w-[72px] sm:w-[90px] aspect-[2/3] rounded-xl overflow-hidden bg-muted shrink-0 shadow-sm border border-black/10 dark:border-white/10 group-hover:scale-105 transition-transform">
              <ArtworkImage
                src={item.imageUrl}
                alt={item.title}
                allowExternal={allowExternal}
                fallbackCategory={defaultCoverCategoryForMediaType(item.type)}
                fallbackSeed={item.id}
                className="w-full h-full object-cover"
                loading="lazy"
              />
            </div>
            
            {/* Details */}
            <div className="flex flex-col flex-1 min-w-0 gap-1">
              <h3 className="text-base sm:text-lg font-bold text-foreground truncate group-hover:text-primary transition-colors">
                {item.title}
              </h3>
              {item.description && (
                <p className="text-xs text-muted-foreground line-clamp-1 font-medium">
                  {item.description}
                </p>
              )}
              <div className="flex items-center gap-[8px] text-xs font-semibold text-muted-foreground/80 mt-0.5">
                <span>{item.year}</span>
                <span>•</span>
                <span>{getCategoryLabel(item.type)}</span>
                {item.rating && (
                  <>
                    <span>•</span>
                    <span className="text-amber-500">★ {item.rating}</span>
                  </>
                )}
              </div>
            </div>

            {/* Action button */}
            <div className="hidden sm:flex items-center pr-[16px]">
              <button className="inline-flex h-[40px] min-w-[112px] items-center justify-center rounded-full bg-foreground px-5 text-xs font-bold text-background transition-transform hover:scale-105">
                查看详情
              </button>
            </div>
          </div>
        ))}
      </div>
    )
  }

  return (
    <div className={density === "compact"
      ? "grid grid-cols-2 sm:grid-cols-4 md:grid-cols-5 lg:grid-cols-6 xl:grid-cols-7 2xl:grid-cols-7 gap-16 md:gap-20"
      : "grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 gap-5 md:gap-7"
    }>
      {items.map((item) => (
        <MediaItem key={item.id} item={item} onHover={onHoverItem} density={density} />
      ))}
    </div>
  )
}
