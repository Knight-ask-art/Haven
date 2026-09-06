import { useEffect, useRef, useState } from "react"
import { useParams, useNavigate } from "react-router"
import { cn } from "@/lib/utils"
import { 
  ChevronLeft, ChevronRight, Play, BookOpen, Heart, Share2,
  Ellipsis, Star, Folder, CheckCircle, RotateCcw, Trash2,
  Tv, Film, Book, FileText, Download, Link2,
  ArrowUp, ArrowDown, ArrowUpDown
} from "lucide-react"
import { ShareCardModal } from "@/components/ui/haven/ShareCardModal"
import { ArtworkImage } from "@/components/ui/haven/ArtworkImage"
import { defaultCoverCategoryForMediaType } from "@/lib/default-cover"
import { REPRESENTATIVE_ITEMS } from "@/features/library/components/LibraryGrid"
import type { LibraryMediaItemData } from "@/features/library/components/MediaItem"
import { HavenError } from "@/lib/ipc/errors"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import { onFavoriteChanged, setFavorite } from "../ipc/favorite-gateway"
import { settleFavoriteMutation } from "../lib/favorite-mutation"
import { resolveMediaDetailRuntimeState } from "../lib/media-detail-runtime-state"
import { getWorkDetail } from "../ipc/work-gateway"
import { mapWorkDetailHeaderToMediaDetail } from "../lib/work-detail-mapper"
import { canConsumeDetail, type WorkDetailState } from "../lib/work-detail-state"
import { loadAllEditionsByWork, normalizeEditionError } from "../ipc/edition-gateway"
import { mapEditionListToDetailItems, partitionEditionItems, toMediaDetailEpisodes, type EditionListItem, type EditionGroup } from "../lib/edition-mapper"
import { canConsumeEdition, getEditionListState, type EditionListState } from "../lib/edition-state"
import { primaryActionRoute } from "../lib/primary-action-route"
import type { PrimaryActionDto } from "@/lib/ipc/generated/wire"
import {
  createDownloadForMediaItem,
  getMediaItemDownloadInfo,
  revealOfflineDownload,
  type DownloadStatus,
} from "@/features/downloads/ipc/download-gateway"
import { resetProgress } from "@/features/progress/ipc/progress-gateway"
import { clearArtworkCache } from "@/features/settings/ipc/privacy-gateway"

export interface MediaDetailData {
  id: string
  title: string
  originalTitle?: string
  type: "movie" | "tv" | "book" | "comic" | "periodical" | "document" | "article"
  year: number
  rating?: string
  quality?: string
  format?: string
  authorOrDirector?: string
  publisherOrStudio?: string
  backdropUrl: string
  posterUrl: string
  description: string
  favorite?: boolean
  progress?: number
  progressText?: string
  size?: string
  seasons?: Array<{ id: string; name: string }>
  episodesOrChapters?: Array<{
    id: string
    number: string | number
    title: string
    durationOrPages?: string
    progress?: number
    isDownloaded?: boolean
    thumbnail?: string
    primaryAction?: PrimaryActionDto | null
  }>
  specs?: {
    resolution?: string
    codec?: string
    audio?: string
    language?: string
    wordCount?: string
    orientation?: string
    frequency?: string
    issn?: string
    pageCount?: string
    docVersion?: string
  }
  relations?: WorkRelation[]
  primaryAction?: PrimaryActionDto
}

export interface WorkRelation {
  role: string
  workId: string
  title: string
  meta: string
}

// 按媒介类型绑定到对应的播放器 / 阅读器路由
function getConsumeRoute(mediaType: MediaDetailData["type"], mediaItemId: string): string {
  if (mediaType === "movie" || mediaType === "tv") return `/player/${mediaItemId}`
  if (mediaType === "comic") return `/comic/${mediaItemId}`
  if (mediaType === "article") return `/article/${mediaItemId}`
  return `/reader/${mediaItemId}`
}

// 预定义多种类型的示范数据库
const PREDEFINED_MEDIA_DB: Record<string, MediaDetailData> = {
  // 电影
  "2": {
    id: "2",
    title: "沙丘2 Dune: Part Two",
    originalTitle: "Dune: Part Two (2024)",
    type: "movie",
    year: 2024,
    rating: "8.9",
    quality: "4K HDR",
    format: "MKV / Dolby Vision",
    authorOrDirector: "丹尼斯·维伦纽瓦 (Denis Villeneuve)",
    publisherOrStudio: "传奇影业 / 华纳兄弟",
    backdropUrl: "https://images.unsplash.com/photo-1509198397868-475647b2a1e5?q=80&w=2560&auto=format&fit=crop",
    posterUrl: "https://picsum.photos/seed/dune/400/600",
    description: "保罗·厄崔迪公爵与契妮以及弗雷曼人会合后展开的复仇旅程。面对关于终极命运的抉择，他必须在深爱的挚爱与已知的宇宙命运之间做出艰难取舍。",
    progress: 45,
    progressText: "已看 1h 14m / 2h 46m (45%)",
    size: "28.6 GB",
    specs: {
      resolution: "3840x2160 (4K UHD)",
      codec: "HEVC / H.265 (Main 10@L5.1)",
      audio: "Dolby TrueHD Atmos 7.1 / DTS-HD MA 5.1",
      language: "英语原声 (杜比全景声) / 简体中文字幕"
    },
    episodesOrChapters: [
      { id: "m1", number: "正片", title: "沙丘2 (4K IMAX 杜比视界高码率原盘)", durationOrPages: "166 分钟", progress: 45, isDownloaded: true }
    ],
    relations: [
      { role: "原作", workId: "dune-novel", title: "沙丘 Dune", meta: "弗兰克·赫伯特 · 1965 · 科幻小说" }
    ]
  },
  // 剧集
  "4": {
    id: "4",
    title: "怪奇物语：1985故事集 第一季",
    originalTitle: "Stranger Things Season 1",
    type: "tv",
    year: 2024,
    rating: "9.2",
    quality: "1080P",
    format: "MP4",
    authorOrDirector: "杜夫兄弟 (Duffer Brothers)",
    publisherOrStudio: "Netflix",
    backdropUrl: "https://images.unsplash.com/photo-1578632767115-351597cf2477?q=80&w=2560&auto=format&fit=crop",
    posterUrl: "https://picsum.photos/seed/spider/400/600",
    description: "在一个小镇上，一个男孩神秘失踪，他的朋友、家人和当地警察开始寻找答案，结果被卷入一个涉及秘密政府实验、恐怖超自然力量和奇怪小女孩的谜团中。",
    progress: 45,
    progressText: "S1:E4 · 24:12",
    size: "14.2 GB",
    seasons: [
      { id: "s1", name: "第一季 (2016)" },
      { id: "s2", name: "第二季 (2017)" },
      { id: "s3", name: "第三季 (2019)" }
    ],
    specs: {
      resolution: "1920x1080 (FHD)",
      codec: "H.264 / AVC",
      audio: "5.1 Surround Sound",
      language: "英语 / 多国字幕"
    },
    episodesOrChapters: [
      { id: "e1", number: "EP 01", title: "威尔·拜尔斯的失踪", durationOrPages: "48 分钟", progress: 100, isDownloaded: true, thumbnail: "https://picsum.photos/seed/ep1/300/180" },
      { id: "e2", number: "EP 02", title: "枫树街上的怪人", durationOrPages: "55 分钟", progress: 100, isDownloaded: true, thumbnail: "https://picsum.photos/seed/ep2/300/180" },
      { id: "e3", number: "EP 03", title: "节日灯光与密码", durationOrPages: "51 分钟", progress: 100, isDownloaded: true, thumbnail: "https://picsum.photos/seed/ep3/300/180" },
      { id: "e4", number: "EP 04", title: "湖底的秘密尸体", durationOrPages: "50 分钟", progress: 45, isDownloaded: true, thumbnail: "https://picsum.photos/seed/ep4/300/180" },
      { id: "e5", number: "EP 05", title: "跳蚤与手风琴理论", durationOrPages: "53 分钟", progress: 0, isDownloaded: false, thumbnail: "https://picsum.photos/seed/ep5/300/180" }
    ],
    relations: [
      { role: "原作小说", workId: "st-novel", title: "怪奇物语：黑暗与迷雾", meta: "格温达·邦德 · 2017 · 官方小说" }
    ]
  },
  // 漫画
  "comic-aot": {
    id: "comic-aot",
    title: "进击的巨人 漫画全集",
    originalTitle: "Attack on Titan (Shingeki no Kyojin)",
    type: "comic",
    year: 2021,
    rating: "9.6",
    format: "CBZ / High-Res PNG",
    authorOrDirector: "谏山创 (Hajime Isayama)",
    publisherOrStudio: "讲谈社 (Kodansha)",
    backdropUrl: "https://images.unsplash.com/photo-1542451313056-b7c8e626645f?q=80&w=2560&auto=format&fit=crop",
    posterUrl: "https://images.unsplash.com/photo-1542451313056-b7c8e626645f?q=80&w=400&auto=format&fit=crop",
    description: "百年之前，巨人的突然出现将人类逼入三道巨大城墙之中。艾伦·耶格尔在亲眼目睹母亲被吞食后，立誓要将巨人从这个世界上彻底驱逐干净。",
    progress: 88,
    progressText: "第 34 卷 (最终卷) · 88%",
    size: "8.4 GB",
    specs: {
      resolution: "2400x3400 PNG",
      wordCount: "全 34 卷 / 139 话",
      orientation: "右开翻页 (Right-to-Left)"
    },
    episodesOrChapters: [
      { id: "v1", number: "Vol.01", title: "单行本 第 1 卷 (第1-4话)", durationOrPages: "192 页", progress: 100, isDownloaded: true },
      { id: "v2", number: "Vol.02", title: "单行本 第 2 卷 (第5-8话)", durationOrPages: "192 页", progress: 100, isDownloaded: true },
      { id: "v34", number: "Vol.34", title: "单行本 第 34 卷 (最终卷：朝向那丘陵上的树)", durationOrPages: "220 页", progress: 88, isDownloaded: true }
    ],
    relations: [
      { role: "改编动画", workId: "aot-anime", title: "进击的巨人 动画系列", meta: "WIT Studio / MAPPA · 2013 起 · 剧集" }
    ]
  },
  "c1": {
    id: "c1",
    title: "进击的巨人 漫画全集 (Vol.1-34)",
    originalTitle: "Shingeki no Kyojin",
    type: "comic",
    year: 2021,
    rating: "9.6",
    format: "CBZ",
    authorOrDirector: "谏山创",
    publisherOrStudio: "讲谈社",
    backdropUrl: "https://images.unsplash.com/photo-1542451313056-b7c8e626645f?q=80&w=2560&auto=format&fit=crop",
    posterUrl: "https://images.unsplash.com/photo-1542451313056-b7c8e626645f?q=80&w=400&auto=format&fit=crop",
    description: "百年之前，巨人的突然出现将人类逼入三道巨大城墙之中。艾伦·耶格尔立誓要将巨人从这个世界上彻底驱逐。",
    progress: 100,
    progressText: "已读完 34 卷",
    size: "8.4 GB",
    specs: { resolution: "2400x3400", orientation: "右开双页 (R-to-L)" },
    episodesOrChapters: [
      { id: "v1", number: "Vol.1", title: "单行本 第 1 卷 (第1-4话)", durationOrPages: "192 页", progress: 100, isDownloaded: true },
      { id: "v34", number: "Vol.34", title: "单行本 第 34 卷 (最终卷)", durationOrPages: "220 页", progress: 100, isDownloaded: true }
    ],
    relations: [
      { role: "改编动画", workId: "aot-anime", title: "进击的巨人 动画系列", meta: "WIT Studio / MAPPA · 2013 起 · 剧集" }
    ]
  },
  // 报刊 (Periodical)
  "p1": {
    id: "p1",
    title: "National Geographic 国家地理杂志 (2024合订本)",
    originalTitle: "National Geographic Magazine 2024",
    type: "periodical",
    year: 2024,
    rating: "9.8",
    format: "High-Res PDF",
    authorOrDirector: "National Geographic Society",
    publisherOrStudio: "国家地理学会",
    backdropUrl: "https://images.unsplash.com/photo-1544716278-ca5e3f4abd8c?q=80&w=2560&auto=format&fit=crop",
    posterUrl: "https://images.unsplash.com/photo-1544716278-ca5e3f4abd8c?q=80&w=400&auto=format&fit=crop",
    description: "探索自然界的奇观、深海未解之谜与人类文明的史诗篇章。《国家地理》带您随顶尖摄影师的镜头穿梭极地与热带雨林。",
    progress: 40,
    progressText: "8 月号 · 40%",
    size: "1.2 GB",
    specs: {
      frequency: "月刊 (Monthly)",
      issn: "ISSN 0027-9358",
      language: "英文原文 / 权威摄影图集",
      orientation: "左开双页 (L-to-R)"
    },
    episodesOrChapters: [
      { id: "p1-1", number: "2024.08", title: "8 月号：深海热泉与极地雪鸮专题", durationOrPages: "148 页", progress: 40, isDownloaded: true },
      { id: "p1-2", number: "2024.07", title: "7 月号：亚马逊雨林失落部落探索", durationOrPages: "152 页", progress: 100, isDownloaded: true },
      { id: "p1-3", number: "2024.06", title: "6 月号：韦伯太空望远镜新视角", durationOrPages: "140 页", progress: 100, isDownloaded: true }
    ]
  },
  // 资料 (Document)
  "d1": {
    id: "d1",
    title: "Apple Human Interface Guidelines (iOS 18 官方规范)",
    originalTitle: "iOS 18 HIG & Spatial Computing Design Specs",
    type: "document",
    year: 2024,
    rating: "9.9",
    format: "Interactive PDF / ePub",
    authorOrDirector: "Apple Human Interface Team",
    publisherOrStudio: "Apple Inc.",
    backdropUrl: "https://images.unsplash.com/photo-1518770660439-4636190af475?q=80&w=2560&auto=format&fit=crop",
    posterUrl: "https://images.unsplash.com/photo-1518770660439-4636190af475?q=80&w=400&auto=format&fit=crop",
    description: "Apple 官方最新发布的 iOS 18 与 visionOS 2 交互设计指南，涵盖动效曲线、组件边界约束、SF Symbols 视觉规制与无障碍访问交互标准。",
    progress: 75,
    progressText: "第 3 部分：TabBar 与全局浮动导航 (75%)",
    size: "128 MB",
    specs: {
      docVersion: "v18.2 Final Release",
      pageCount: "380 页",
      wordCount: "165,000 字",
      language: "英文原版 / 中文全译"
    },
    episodesOrChapters: [
      { id: "d1-1", number: "Part 1", title: "设计总则：空间计算与毛玻璃光影", durationOrPages: "45 页", progress: 100, isDownloaded: true },
      { id: "d1-2", number: "Part 2", title: "动态字阶 (Dynamic Type) 与色彩层级", durationOrPages: "60 页", progress: 100, isDownloaded: true },
      { id: "d1-3", number: "Part 3", title: "TabBar, Floating Nav & 悬浮菜单规范", durationOrPages: "55 页", progress: 75, isDownloaded: true }
    ]
  },
  // 原作书籍：沙丘 (由《沙丘2》电影的作品关系跳转而来)
  "dune-novel": {
    id: "dune-novel",
    title: "沙丘 Dune",
    originalTitle: "Dune (1965)",
    type: "book",
    year: 1965,
    rating: "9.7",
    format: "EPUB",
    authorOrDirector: "弗兰克·赫伯特 (Frank Herbert)",
    publisherOrStudio: "Chilton Books / 读客文化",
    backdropUrl: "https://images.unsplash.com/photo-1509198397868-475647b2a1e5?q=80&w=2560&auto=format&fit=crop",
    posterUrl: "https://picsum.photos/seed/dune-novel/400/600",
    description: "厄拉科斯星球上，沙漠与香料统治着宇宙的命运。少年保罗·厄崔迪被卷入帝国家族斗争，最终成为领导弗雷曼人反抗暴政的传奇救世主。",
    progress: 100,
    progressText: "已读完",
    size: "2.1 MB",
    specs: { wordCount: "412,000 字", language: "中文译版 / 英文原版" },
    episodesOrChapters: [
      { id: "dn1", number: "Book 1", title: "第一卷：沙丘", durationOrPages: "224 页", progress: 100, isDownloaded: true },
      { id: "dn2", number: "Book 2", title: "第二卷：穆阿迪布", durationOrPages: "196 页", progress: 100, isDownloaded: true }
    ],
    relations: [
      { role: "改编电影", workId: "2", title: "沙丘2 Dune: Part Two", meta: "丹尼斯·维伦纽瓦 · 2024 · 电影" }
    ]
  },
  // 原作小说：怪奇物语 (由《怪奇物语》剧集的作品关系跳转而来)
  "st-novel": {
    id: "st-novel",
    title: "怪奇物语：黑暗与迷雾",
    originalTitle: "Stranger Things: Darkness on the Edge of Town",
    type: "book",
    year: 2017,
    rating: "8.8",
    format: "EPUB",
    authorOrDirector: "格温达·邦德 (Gwenda Bond)",
    publisherOrStudio: "Netflix / 青马文化",
    backdropUrl: "https://images.unsplash.com/photo-1578632767115-351597cf2477?q=80&w=2560&auto=format&fit=crop",
    posterUrl: "https://picsum.photos/seed/st-novel/400/600",
    description: "霍金斯小镇的怪谈并未结束。以霍珀警长为主视角的官方前传小说，揭开他来到霍金斯之前的秘密过往。",
    progress: 35,
    progressText: "第 6 章 · 35%",
    size: "1.4 MB",
    specs: { wordCount: "98,000 字", language: "简体中文 / 英文原版" },
    episodesOrChapters: [
      { id: "sn1", number: "CH 01", title: "霍珀警长的来处", durationOrPages: "28 页", progress: 100, isDownloaded: true },
      { id: "sn6", number: "CH 06", title: "1977 年的那个夏天", durationOrPages: "32 页", progress: 35, isDownloaded: true }
    ],
    relations: [
      { role: "改编剧集", workId: "4", title: "怪奇物语：1985故事集 第一季", meta: "Netflix · 2024 · 剧集" }
    ]
  },
  // 改编动画：进击的巨人 (由《进击的巨人》漫画的作品关系跳转而来)
  "aot-anime": {
    id: "aot-anime",
    title: "进击的巨人 动画系列",
    originalTitle: "Attack on Titan (Shingeki no Kyojin)",
    type: "tv",
    year: 2013,
    rating: "9.3",
    quality: "1080P",
    format: "MKV",
    authorOrDirector: "荒木哲郎 / 林祐一郎",
    publisherOrStudio: "WIT Studio / MAPPA",
    backdropUrl: "https://images.unsplash.com/photo-1542451313056-b7c8e626645f?q=80&w=2560&auto=format&fit=crop",
    posterUrl: "https://picsum.photos/seed/aot-anime/400/600",
    description: "改编自谏山创同名漫画。被巨人支配的世界里，少年艾伦·耶格尔立誓复仇，却一步步揭开墙内世界的惊人真相。",
    progress: 76,
    progressText: "S4:E28 · 自由之翼 (76%)",
    size: "32.8 GB",
    seasons: [
      { id: "s1", name: "第一季 (2013)" },
      { id: "s2", name: "第二季 (2017)" },
      { id: "s3", name: "第三季 (2018)" },
      { id: "s4", name: "最终季 (2020)" }
    ],
    specs: { resolution: "1920x1080 (FHD)", codec: "H.264 / AVC", audio: "日语原声 / 简繁字幕" },
    episodesOrChapters: [
      { id: "ae1", number: "S1:E1", title: "致两千年后的你", durationOrPages: "24 分钟", progress: 100, isDownloaded: true, thumbnail: "https://picsum.photos/seed/aot1/300/180" },
      { id: "ae2", number: "S4:E28", title: "自由之翼", durationOrPages: "24 分钟", progress: 76, isDownloaded: true, thumbnail: "https://picsum.photos/seed/aot2/300/180" }
    ],
    relations: [
      { role: "改编自", workId: "comic-aot", title: "进击的巨人 漫画全集", meta: "谏山创 · 2009 · 漫画" }
    ]
  }
}

// 根据媒体库 canonical 数据兜底构建详情页数据，保证任意库内 ID 都显示正确作品
function buildCatalogFallback(item: LibraryMediaItemData): MediaDetailData {
  const type = item.type as MediaDetailData["type"]
  const base = {
    id: item.id,
    title: item.title,
    originalTitle: item.originalTitle,
    type,
    year: item.year,
    rating: item.rating,
    description: item.description || "",
    backdropUrl: item.backdropUrl || item.imageUrl,
    posterUrl: item.imageUrl,
  }
  switch (type) {
    case "tv":
      return {
        ...base,
        quality: item.badge,
        format: "MKV",
        authorOrDirector: "剧集制作团队",
        specs: { resolution: "1920x1080", codec: "H.264 / AVC", audio: "5.1 Surround", language: "多语言" },
        episodesOrChapters: [
          { id: `${item.id}-e1`, number: "EP 01", title: "第一集", durationOrPages: "45 分钟", progress: 0 },
          { id: `${item.id}-e2`, number: "EP 02", title: "第二集", durationOrPages: "45 分钟", progress: 0 }
        ]
      }
    case "movie":
      return {
        ...base,
        quality: item.badge,
        format: "4K",
        authorOrDirector: "导演",
        specs: { resolution: "3840x2160", codec: "HEVC / H.265", audio: "Dolby Atmos", language: "原声 / 中文字幕" },
        episodesOrChapters: [
          { id: `${item.id}-main`, number: "正片", title: "正片资源", durationOrPages: "约 2 小时", progress: 0 }
        ]
      }
    case "comic":
      return {
        ...base,
        format: "CBZ",
        authorOrDirector: "作者",
        specs: { resolution: "1600x2400", orientation: "右开翻页 (Right-to-Left)" },
        episodesOrChapters: [
          { id: `${item.id}-v1`, number: "Vol.01", title: "单行本 第 1 卷", durationOrPages: "192 页", progress: 0 },
          { id: `${item.id}-v2`, number: "Vol.02", title: "单行本 第 2 卷", durationOrPages: "192 页", progress: 0 }
        ]
      }
    case "periodical":
      return {
        ...base,
        format: "PDF",
        authorOrDirector: "编辑部",
        specs: { frequency: "定期发行", language: "多语言" },
        episodesOrChapters: [
          { id: `${item.id}-i1`, number: "最新刊", title: "最新一期", durationOrPages: "120 页", progress: 0 },
          { id: `${item.id}-i2`, number: "往期", title: "往期刊物", durationOrPages: "120 页", progress: 0 }
        ]
      }
    case "document":
      return {
        ...base,
        format: "PDF",
        authorOrDirector: "文档作者",
        specs: { pageCount: "约 100 页", language: "多语言" },
        episodesOrChapters: [
          { id: `${item.id}-p1`, number: "Part 1", title: "第一部分", durationOrPages: "50 页", progress: 0 },
          { id: `${item.id}-p2`, number: "Part 2", title: "第二部分", durationOrPages: "50 页", progress: 0 }
        ]
      }
    case "article":
      return {
        ...base,
        format: "HTML",
        authorOrDirector: "作者",
        specs: { wordCount: "约 8,000 字", language: "中文" },
        episodesOrChapters: [
          { id: `${item.id}-main`, number: "正文", title: "文章正文", durationOrPages: "约 15 分钟阅读", progress: 0 }
        ]
      }
    default:
      return {
        ...base,
        format: "EPUB",
        authorOrDirector: "作者",
        specs: { wordCount: "约 150,000 字", language: "简体中文" },
        episodesOrChapters: [
          { id: `${item.id}-c1`, number: "CH 01", title: "第一章", durationOrPages: "30 页", progress: 0 },
          { id: `${item.id}-c2`, number: "CH 02", title: "第二章", durationOrPages: "30 页", progress: 0 }
        ]
      }
  }
}

// 动态合成/推断函数，保证点击任意未知 ID 都能展示匹配类型的丰富排版
function getMediaDetailData(id: string): MediaDetailData {
  if (PREDEFINED_MEDIA_DB[id]) {
    return PREDEFINED_MEDIA_DB[id]
  }

  // 优先命中媒体库 canonical 数据，保证 /work/:id 与库内作品一致
  const catalogItem = REPRESENTATIVE_ITEMS.find((item) => item.id === id)
  if (catalogItem) {
    return buildCatalogFallback(catalogItem)
  }

  // 根据 ID 关键字推断媒介类型
  let inferredType: "movie" | "tv" | "book" | "comic" | "periodical" | "document" | "article" = "book"
  if (id.includes("show") || id.includes("tv") || id.includes("severance")) inferredType = "tv"
  else if (id.includes("movie") || id.includes("dune") || id.includes("interstellar") || id.includes("oppenheimer")) inferredType = "movie"
  else if (id.includes("comic") || id.includes("manga") || id.includes("aot") || id.startsWith("c")) inferredType = "comic"
  else if (id.includes("periodical") || id.includes("magazine") || id.includes("natgeo") || id.includes("econ") || id.startsWith("p")) inferredType = "periodical"
  else if (id.includes("document") || id.includes("paper") || id.includes("whitepaper") || id.includes("spec") || id.startsWith("d")) inferredType = "document"
  else if (id.includes("article") || id.includes("deepmind") || id.includes("html")) inferredType = "article"

  // 根据推断的类型动态构建定制数据
  if (inferredType === "periodical") {
    return {
      id,
      title: "The Economist 经济学人",
      originalTitle: "The Economist Magazine 2024",
      type: "periodical",
      year: 2024,
      rating: "9.7",
      format: "PDF",
      authorOrDirector: "The Economist Newspaper Ltd.",
      publisherOrStudio: "经济学人集团",
      backdropUrl: "https://images.unsplash.com/photo-1504711434969-e33886168f5c?q=80&w=2560&auto=format&fit=crop",
      posterUrl: `https://picsum.photos/seed/${id}/400/600`,
      description: "汇聚全球政治、商业、科技与金融领域的深度分析与前瞻洞察。每周发布最新特刊，助您厘清复杂多变的全球局势。",
      progress: 30,
      progressText: "最新期 · 30%",
      size: "45 MB",
      specs: { frequency: "周刊 (Weekly)", issn: "ISSN 0013-0613", language: "英文" },
      episodesOrChapters: [
        { id: "p-e1", number: "Issue 9401", title: "封面特刊：AI 智能体时代商业重构", durationOrPages: "96 页", progress: 100, isDownloaded: true },
        { id: "p-e2", number: "Issue 9402", title: "全球宏观经济与美联储降息政策前瞻", durationOrPages: "92 页", progress: 30, isDownloaded: true }
      ]
    }
  } else if (inferredType === "document") {
    return {
      id,
      title: "DeepMind AlphaFold 3 技术白皮书",
      originalTitle: "Accurate Structure Prediction of Biomolecular Interactions",
      type: "document",
      year: 2024,
      rating: "9.9",
      format: "PDF / Technical Paper",
      authorOrDirector: "Google DeepMind Team",
      publisherOrStudio: "Nature / DeepMind Research",
      backdropUrl: "https://images.unsplash.com/photo-1451187580459-43490279c0fa?q=80&w=2560&auto=format&fit=crop",
      posterUrl: `https://picsum.photos/seed/${id}/400/600`,
      description: "全面阐述 AlphaFold 3 针对蛋白质、DNA、RNA 与小分子配体互作预测的最新扩散模型架构，开创结构生物学全新的计算范式。",
      progress: 50,
      progressText: "第 2 节 · 架构解析 (50%)",
      size: "18.5 MB",
      specs: { docVersion: "Nature 2024 Release", pageCount: "42 页", language: "英文学术原版" },
      episodesOrChapters: [
        { id: "d-e1", number: "Sec 01", title: "引言与生物大分子预测概述", durationOrPages: "8 页", progress: 100, isDownloaded: true },
        { id: "d-e2", number: "Sec 02", title: "Diffusion Network 扩散网络与 Pairformer 架构", durationOrPages: "16 页", progress: 50, isDownloaded: true }
      ]
    }
  } else if (inferredType === "tv") {
    return {
      id,
      title: id.includes("severance") ? "人生切割术 第一季" : `热门剧集 (${id})`,
      originalTitle: "Severance Season 1",
      type: "tv",
      year: 2024,
      rating: "9.1",
      quality: "1080P HDR",
      format: "MKV",
      authorOrDirector: "本·斯蒂勒 (Ben Stiller)",
      publisherOrStudio: "Apple TV+",
      backdropUrl: "https://images.unsplash.com/photo-1497366216548-37526070297c?q=80&w=2560&auto=format&fit=crop",
      posterUrl: `https://picsum.photos/seed/${id}/400/600`,
      description: "在卢蒙工业公司，一项名为“工作与生活平衡”的手术将员工的记忆彻底切割为工作与私人两部分。当马克逐渐发现背后的阴谋，真相开始撕裂规则。",
      progress: 89,
      progressText: "S1:E4 · 你真正的自己 (89%)",
      size: "18.4 GB",
      seasons: [{ id: "s1", name: "第一季" }, { id: "s2", name: "第二季" }],
      specs: { resolution: "1920x1080", codec: "H.265 / HEVC", audio: "Dolby Digital 5.1" },
      episodesOrChapters: [
        { id: "e1", number: "S1:E1", title: "好消息是……", durationOrPages: "57 分钟", progress: 100, isDownloaded: true, thumbnail: "https://picsum.photos/seed/sev1/300/180" },
        { id: "e2", number: "S1:E2", title: "半头半脑", durationOrPages: "53 分钟", progress: 100, isDownloaded: true, thumbnail: "https://picsum.photos/seed/sev2/300/180" },
        { id: "e3", number: "S1:E3", title: "永恒的清理间", durationOrPages: "51 分钟", progress: 100, isDownloaded: true, thumbnail: "https://picsum.photos/seed/sev3/300/180" },
        { id: "e4", number: "S1:E4", title: "你真正的自己", durationOrPages: "54 分钟", progress: 89, isDownloaded: true, thumbnail: "https://picsum.photos/seed/sev4/300/180" }
      ]
    }
  } else if (inferredType === "movie") {
    return {
      id,
      title: id.includes("interstellar") ? "星际穿越 Interstellar" : "电影作品",
      originalTitle: "Interstellar (2014)",
      type: "movie",
      year: 2014,
      rating: "9.4",
      quality: "4K HDR",
      format: "4K Remux",
      authorOrDirector: "克里斯托弗·诺兰 (Christopher Nolan)",
      publisherOrStudio: "派拉蒙影业",
      backdropUrl: "https://images.unsplash.com/photo-1462331940025-496dfbfc7564?q=80&w=2560&auto=format&fit=crop",
      posterUrl: `https://picsum.photos/seed/${id}/400/600`,
      description: "在近未来的地球，前 NASA 宇航员库珀受命带领一组探险家利用最新发现的虫洞穿越星际，寻找适合人类生存的全新栖息地。",
      progress: 55,
      progressText: "已看 1h 23m / 2h 49m",
      size: "42.1 GB",
      specs: { resolution: "3840x2160 (4K)", codec: "HEVC / H.265", audio: "DTS-HD MA 7.1" },
      episodesOrChapters: [
        { id: "m1", number: "正片", title: "星际穿越 (4K 蓝光高清正片)", durationOrPages: "169 分钟", progress: 55, isDownloaded: true }
      ],
      relations: id.includes("dune")
        ? [{ role: "原作", workId: "dune-novel", title: "沙丘 Dune", meta: "弗兰克·赫伯特 · 1965 · 科幻小说" }]
        : id.includes("interstellar")
          ? [{ role: "原作", workId: "interstellar-novel", title: "星际穿越：官方小说", meta: "格雷格·凯斯 · 2014 · 电影小说化" }]
          : undefined
    }
  } else if (inferredType === "comic") {
    return {
      id,
      title: "迷宫饭 漫画全集",
      originalTitle: "Delicious in Dungeon",
      type: "comic",
      year: 2024,
      rating: "9.4",
      format: "CBZ",
      authorOrDirector: "九井谅子 (Ryoko Kui)",
      publisherOrStudio: "KADOKAWA",
      backdropUrl: "https://images.unsplash.com/photo-1607604276583-eef5d076aa5f?q=80&w=2560&auto=format&fit=crop",
      posterUrl: `https://picsum.photos/seed/${id}/400/600`,
      description: "冒险者莱欧斯为了解救被红龙吞食的妹妹，率领小队再度深入迷宫。在缺乏资金与补给的绝境下，他们决定：吃掉迷宫里的魔物！",
      progress: 40,
      progressText: "第 8 卷 · 40%",
      size: "3.6 GB",
      specs: { orientation: "右开双页 (R-to-L)", resolution: "1600x2400" },
      episodesOrChapters: [
        { id: "v1", number: "Vol.1", title: "单行本 第 1 卷 (大蝎子与大水蛇)", durationOrPages: "198 页", progress: 100, isDownloaded: true },
        { id: "v2", number: "Vol.2", title: "单行本 第 2 卷 (曼德拉草与巴西利斯克)", durationOrPages: "192 页", progress: 100, isDownloaded: true }
      ],
      relations: id.includes("aot")
        ? [{ role: "改编动画", workId: "aot-anime", title: "进击的巨人 动画系列", meta: "WIT Studio / MAPPA · 2013 起 · 剧集" }]
        : undefined
    }
  } else if (inferredType === "article") {
    return {
      id,
      title: "The Agentic AI Era: 智能体时代演进",
      originalTitle: "Autonomous Systems Architecture Review",
      type: "article",
      year: 2024,
      rating: "9.5",
      format: "HTML / RSS",
      authorOrDirector: "Tech Insights & DeepMind",
      publisherOrStudio: "Substack / arXiv",
      backdropUrl: "https://images.unsplash.com/photo-1451187580459-43490279c0fa?q=80&w=2560&auto=format&fit=crop",
      posterUrl: `https://picsum.photos/seed/${id}/400/600`,
      description: "深度剖析 Agentic AI 智能体时代的核心设计范式，探讨大模型在自主规划、工具调用、长期记忆与多智能体协作领域的最新实践与架构挑战。",
      progress: 30,
      progressText: "阅读进度 30%",
      size: "1.2 MB",
      specs: { wordCount: "18,400 字", language: "英文原文 / 沉浸翻译" },
      episodesOrChapters: [
        { id: "a1", number: "Part 1", title: "从 Prompt 到 Autonomy：Agent 的演进史", durationOrPages: "6 分钟阅读", progress: 100, isDownloaded: true },
        { id: "a2", number: "Part 2", title: "Tool Use & Environment Loops 架构解析", durationOrPages: "10 分钟阅读", progress: 30, isDownloaded: true }
      ]
    }
  } else {
    // 默认图书
    return {
      id,
      title: id.includes("interstellar-novel") ? "星际穿越：官方小说" : id.includes("jobs") ? "史蒂夫·乔布斯传" : "经典图书作品",
      originalTitle: id.includes("interstellar-novel") ? "Interstellar (Novelization)" : "Steve Jobs - Official Biography",
      type: "book",
      year: id.includes("interstellar-novel") ? 2014 : 2011,
      rating: "9.5",
      format: "EPUB",
      authorOrDirector: id.includes("interstellar-novel") ? "格雷格·凯斯 (Greg Keyes)" : "沃尔特·艾萨克森 (Walter Isaacson)",
      publisherOrStudio: id.includes("interstellar-novel") ? "Titan Books" : "中信出版社",
      backdropUrl: id.includes("interstellar-novel")
        ? "https://images.unsplash.com/photo-1462331940025-496dfbfc7564?q=80&w=2560&auto=format&fit=crop"
        : "https://images.unsplash.com/photo-1518770660439-4636190af475?q=80&w=2560&auto=format&fit=crop",
      posterUrl: `https://picsum.photos/seed/${id}/400/600`,
      description: id.includes("interstellar-novel")
        ? "根据电影《星际穿越》改编的官方小说，将库珀穿越虫洞、探索新世界的星际旅程还原为一场关于时间、亲情与人类存续的史诗冒险。"
        : "乔布斯唯一授权的官方传记，记录了他颠覆个人电脑、动画电影、音乐、手机、平板电脑和数字出版六大产业的传奇一生。",
      progress: 68,
      progressText: "第 14 章 · 68%",
      size: "18.5 MB",
      specs: { wordCount: "420,000 字" },
      episodesOrChapters: [
        { id: "c1", number: "CH 01", title: "童年：被遗弃与被选择", durationOrPages: "22 页", progress: 100, isDownloaded: true },
        { id: "c14", number: "CH 14", title: "硅谷的王者归来与 NeXT 时代", durationOrPages: "35 页", progress: 68, isDownloaded: true }
      ],
      relations: id.includes("interstellar-novel")
        ? [{ role: "改编电影", workId: "10", title: "星际穿越 Interstellar", meta: "克里斯托弗·诺兰 · 2014 · 电影" }]
        : undefined
    }
  }
}

function buildUnavailableDetail(id: string): MediaDetailData {
  return {
    id,
    title: "作品信息暂不可用",
    type: "document",
    year: 0,
    backdropUrl: "",
    posterUrl: "",
    description: "暂时无法从媒体库读取这项作品的权威信息。",
    episodesOrChapters: [],
  }
}

export function MediaDetailPage() {
  const runtimeState = resolveMediaDetailRuntimeState(getHavenClientMode())

  if (runtimeState === "unavailable") {
    return <MediaDetailUnavailable />
  }

  return <MediaDetailExperience production={runtimeState === "production"} />
}

function MediaDetailUnavailable() {
  const navigate = useNavigate()

  return (
    <div className="flex min-h-full w-full flex-col bg-background text-foreground">
      <header className="flex h-[68px] items-center gap-3 border-b border-border/50 px-5 sm:px-8">
        <button
          type="button"
          onClick={() => navigate(-1)}
          aria-label="返回"
          className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full transition-colors hover:bg-muted"
        >
          <ChevronLeft className="h-5 w-5" />
        </button>
        <p className="text-sm font-semibold">作品详情</p>
      </header>
      <main className="flex flex-1 items-center justify-center px-6 py-16 text-center">
        <div className="max-w-sm space-y-2">
          <h1 className="text-lg font-semibold">作品信息暂不可用</h1>
          <p className="text-sm text-muted-foreground">
            当前环境未连接栖阅本地媒体库，请在桌面应用中重新打开作品。
          </p>
        </div>
      </main>
    </div>
  )
}

function MediaDetailExperience({ production }: { production: boolean }) {
  const { workId, id } = useParams<{ workId?: string; id?: string }>()
  const navigate = useNavigate()
  const requestedId = workId || id || "1"
  const [authoritativeItem, setAuthoritativeItem] = useState<MediaDetailData | null>(null)
  const [detailLoading, setDetailLoading] = useState(production)
  const [detailError, setDetailError] = useState<HavenError | null>(null)
  const [detailRetryNonce, setDetailRetryNonce] = useState(0)
  const [editionItems, setEditionItems] = useState<EditionListItem[] | null>(production ? null : [])
  const [editionLoading, setEditionLoading] = useState(production)
  const [editionError, setEditionError] = useState<HavenError | null>(null)
  const [editionRetryNonce, setEditionRetryNonce] = useState(0)
  const [activeEditionType, setActiveEditionType] = useState<string | null>(null)

  useEffect(() => {
    if (!production) return
    let cancelled = false
    setDetailLoading(true)
    setAuthoritativeItem(null)
    setDetailError(null)
    getWorkDetail(requestedId)
      .then((header) => {
        if (cancelled) return
        setAuthoritativeItem(mapWorkDetailHeaderToMediaDetail(header))
        setDetailLoading(false)
      })
      .catch((error: unknown) => {
        if (cancelled) return
        setDetailError(error instanceof HavenError ? error : null)
        setDetailLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [production, requestedId, detailRetryNonce])

  useEffect(() => {
    if (!production) return
    let cancelled = false
    setEditionLoading(true)
    setEditionItems(null)
    setEditionError(null)
    loadAllEditionsByWork(requestedId)
      .then((result) => {
        if (cancelled) return
        setEditionItems(mapEditionListToDetailItems(result))
        setEditionLoading(false)
      })
      .catch((error: unknown) => {
        if (cancelled) return
        setEditionError(normalizeEditionError(error))
        setEditionLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [editionRetryNonce, production, requestedId])

  // Explicit browser demo keeps its curated catalog; Tauri uses only the server projection.
  const media = production
    ? (authoritativeItem
      ? {
        ...authoritativeItem,
        episodesOrChapters: editionItems ? toMediaDetailEpisodes(editionItems) : [],
      }
      : buildUnavailableDetail(requestedId))
    : getMediaDetailData(requestedId)
  const downloadableMediaItemId = authoritativeItem?.primaryAction?.mediaItemId
    ?? editionItems?.find((item) => item.primaryAction?.mediaItemId)?.primaryAction?.mediaItemId

  const [activeTab, setActiveTab] = useState<"contents" | "overview" | "specs">("contents")
  const [isDescriptionExpanded, setIsDescriptionExpanded] = useState(false)
  const [isAscending, setIsAscending] = useState<boolean>(() => {
    try {
      const raw = localStorage.getItem(`haven:ui:sort:${requestedId}`)
      if (raw === "asc") return true
      if (raw === "desc") return false
    } catch { /* ignore */ }
    return true
  })
  const [isFavorite, setIsFavorite] = useState(false)
  const [isFavoriteLoading, setIsFavoriteLoading] = useState(production)
  const [isFavoriteSaving, setIsFavoriteSaving] = useState(false)
  const editionOpenRequestRef = useRef(0)
  const [editionOpeningId, setEditionOpeningId] = useState<string | null>(null)
  const [isDownloadSaving, setIsDownloadSaving] = useState(false)
  const favoriteRequestRef = useRef(0)
  const [downloadStatus, setDownloadStatus] = useState<DownloadStatus>("idle")
  const [downloadCapability, setDownloadCapability] = useState<"loading" | "available" | "unavailable" | "error">(
    production ? "loading" : "unavailable",
  )
  const [onlineReadCapability, setOnlineReadCapability] = useState<"loading" | "available" | "unavailable" | "error">(
    production ? "loading" : "available",
  )
  const initialSeasonId = media.seasons?.[0]?.id || "s1"
  const [toastMessage, setToastMessage] = useState<string | null>(null)
  const [isShareModalOpen, setIsShareModalOpen] = useState(false)
  const [isMoreMenuOpen, setIsMoreMenuOpen] = useState(false)
  const [selectedSeason, setSelectedSeason] = useState(media.seasons?.[0]?.id || "s1")
  const sortedEpisodes = media.episodesOrChapters
    ? isAscending
      ? media.episodesOrChapters
      : [...media.episodesOrChapters].reverse()
    : media.episodesOrChapters

  // “分页后分组”：仅对已加载的一页版本按媒体类型分区（O(n)），多类型时用 Tabs 切换。
  const editionGroups: EditionGroup[] = partitionEditionItems(editionItems ?? [])
  const activeGroup = editionGroups.find((group) => group.mediaType === activeEditionType) ?? editionGroups[0]
  const episodesForActiveGroup = activeGroup ? toMediaDetailEpisodes(activeGroup.items) : sortedEpisodes
  const sortedActiveEpisodes = episodesForActiveGroup
    ? isAscending
      ? episodesForActiveGroup
      : [...episodesForActiveGroup].reverse()
    : episodesForActiveGroup

  // 详情页直接进入/刷新也从服务端 WorkCard 投影读取收藏；localStorage 不再作为权威。
  useEffect(() => {
    if (production) {
      if (detailLoading) {
        setIsFavoriteLoading(true)
      } else if (authoritativeItem?.id === media.id) {
        setIsFavorite(authoritativeItem.favorite ?? false)
        setIsFavoriteLoading(false)
      } else {
        // A missing or failed Work is terminal/retryable, not an endless
        // loading state. It must remain read-only because no authoritative ID
        // was returned by the service.
        setIsFavorite(false)
        setIsFavoriteLoading(false)
      }
    } else {
      setIsFavorite(REPRESENTATIVE_ITEMS.find((item) => item.id === media.id)?.favorite ?? false)
      setIsFavoriteLoading(false)
    }
  }, [authoritativeItem, detailLoading, media.id, production])

  // 同一组件实例切换 /work/:id 时，所有与作品绑定的局部状态都必须重置。
  useEffect(() => {
    favoriteRequestRef.current += 1
    editionOpenRequestRef.current += 1
    setEditionOpeningId(null)
    setIsFavoriteSaving(false)
    setDownloadStatus("idle")
    setDownloadCapability(production ? "loading" : "unavailable")
    setOnlineReadCapability(production ? "loading" : "available")
    setActiveTab("contents")
    setSelectedSeason(initialSeasonId)
    setIsMoreMenuOpen(false)
    setActiveEditionType(null)
  }, [initialSeasonId, media.id, production])

  // per-作品正倒序持久化（localStorage，仅 UI 偏好）
  useEffect(() => {
    try {
      const raw = localStorage.getItem(`haven:ui:sort:${requestedId}`)
      if (raw === "asc") setIsAscending(true)
      else if (raw === "desc") setIsAscending(false)
      else setIsAscending(true)
    } catch { setIsAscending(true) }
  }, [requestedId])
  useEffect(() => {
    try {
      localStorage.setItem(`haven:ui:sort:${requestedId}`, isAscending ? "asc" : "desc")
    } catch { /* ignore */ }
  }, [isAscending, requestedId])

  // favorite-changed 订阅（SLICE-FAVORITE-001：跨入口收藏投影一致；仅 Tauri 环境）。
  useEffect(() => {
    if (!production) return
    let unlisten: (() => void) | null = null
    let disposed = false
    onFavoriteChanged((payload) => {
      if (payload.workId === media.id) setIsFavorite(payload.favorite)
    })
      .then((fn) => {
        if (disposed) fn()
        else unlisten = fn
      })
      .catch(() => {
        // 事件订阅失败不阻塞页面（收藏仍可经 favoriteSet 直接操作）
      })
    return () => {
      disposed = true
      unlisten?.()
    }
  }, [media.id, production])

  // 收藏切换：乐观更新 + 单飞请求，避免连点时旧请求回滚覆盖新状态。
  const handleFavoriteToggle = () => {
    if (isFavoriteLoading || isFavoriteSaving) return
    const next = !isFavorite
    const requestId = ++favoriteRequestRef.current
    setIsFavoriteSaving(true)
    setIsFavorite(next)
    settleFavoriteMutation(isFavorite, () => setFavorite({ workId: media.id, favorite: next }))
      .then(({ favorite, error }) => {
        if (favoriteRequestRef.current !== requestId) return
        setIsFavorite(favorite)
        if (error !== null) {
          const message = error instanceof HavenError
            ? error.dto.userMessage
            : "收藏操作失败，请稍后重试"
          setToastMessage(message)
          setTimeout(() => setToastMessage(null), 2500)
        }
      })
      .finally(() => {
        if (favoriteRequestRef.current === requestId) setIsFavoriteSaving(false)
      })
  }

  const handleDownloadAction = async () => {
    if (!production) {
      setToastMessage("下载功能仅在桌面应用中可用")
      setTimeout(() => setToastMessage(null), 2500)
      return
    }
    if (downloadStatus === "queued") {
      navigate("/downloads")
      return
    }
    if (downloadStatus === "downloaded") {
      setToastMessage("该作品已有离线内容")
      setTimeout(() => setToastMessage(null), 2500)
      return
    }
    if (downloadCapability === "loading") {
      setToastMessage("正在读取下载能力，请稍候")
      setTimeout(() => setToastMessage(null), 2500)
      return
    }
    if (downloadCapability !== "available" || !downloadableMediaItemId) {
      setToastMessage(downloadCapability === "error" ? "下载能力读取失败，请重试" : "当前内容没有可保存到本地的资源")
      setTimeout(() => setToastMessage(null), 2500)
      return
    }
    setIsDownloadSaving(true)
    try {
      const task = await createDownloadForMediaItem(downloadableMediaItemId)
      setDownloadStatus(task.state === "completed" ? "downloaded" : "queued")
      setToastMessage(task.state === "completed" ? "已保存到离线库" : "已加入下载列表")
    } catch (error) {
      setToastMessage(error instanceof HavenError ? error.dto.userMessage : "创建下载任务失败")
    } finally {
      setIsDownloadSaving(false)
      setTimeout(() => setToastMessage(null), 2500)
    }
  }

  const handleMoreAction = async (action: "folder" | "reset" | "cache") => {
    setIsMoreMenuOpen(false)
    if (!production) {
      setToastMessage("该操作仅在桌面应用中可用")
      return
    }
    try {
      if (action === "cache") {
        await clearArtworkCache()
        setToastMessage("已清理 Artwork 缓存")
        return
      }
      if (!downloadableMediaItemId) throw new Error("当前作品没有可用的媒体版本")
      if (action === "reset") {
        await resetProgress(downloadableMediaItemId)
        setToastMessage("已重置进度")
        return
      }
      const info = await getMediaItemDownloadInfo(downloadableMediaItemId)
      if (!info.taskId || !info.hasOfflineResource) throw new Error("当前作品没有可定位的离线文件")
      await revealOfflineDownload(info.taskId)
      setToastMessage("已打开离线文件夹")
    } catch (error) {
      setToastMessage(error instanceof HavenError ? error.dto.userMessage : "操作失败，请重试")
    }
  }

  /**
   * Edition rows carry the server-selected action so they can still identify
   * a download-only remote resource.  They must not navigate directly to a
   * reader route until the backend capability projection confirms that this
   * specific media item is online-readable (or has a usable Offline Resource).
   * This keeps an OPDS/Gutenberg EPUB, for example, from landing on a dead
   * reader route while preserving the existing visual list structure.
   */
  const handleEditionOpen = async (action: PrimaryActionDto | null | undefined, fallbackId: string) => {
    if (!production) {
      const target = getConsumeRoute(media.type, fallbackId)
      navigate(target)
      return
    }
    if (!action) {
      setToastMessage("当前版本暂不可打开")
      setTimeout(() => setToastMessage(null), 2500)
      return
    }
    const target = primaryActionRoute(action)
    if (!target) {
      setToastMessage("当前版本暂不可打开")
      setTimeout(() => setToastMessage(null), 2500)
      return
    }
    // Opening an edition detail page is a safe metadata navigation.  Only
    // content-consuming actions need the per-media capability check below.
    if (action.kind === "open_edition") {
      navigate(target)
      return
    }
    const mediaItemId = action.mediaItemId
    if (!mediaItemId) {
      setToastMessage("当前内容暂不可用")
      setTimeout(() => setToastMessage(null), 2500)
      return
    }
    const requestId = ++editionOpenRequestRef.current
    setEditionOpeningId(fallbackId)
    try {
      const info = await getMediaItemDownloadInfo(mediaItemId)
      if (editionOpenRequestRef.current !== requestId) return
      if (info.canOnlineRead || info.hasOfflineResource) {
        navigate(target)
      } else {
        setToastMessage(info.canDownload ? "该内容需要下载后阅读" : "当前内容暂不可用")
        setTimeout(() => setToastMessage(null), 2500)
      }
    } catch {
      if (editionOpenRequestRef.current === requestId) {
        setToastMessage("在线阅读能力读取失败，请重试")
        setTimeout(() => setToastMessage(null), 2500)
      }
    } finally {
      if (editionOpenRequestRef.current === requestId) setEditionOpeningId(null)
    }
  }

  // 媒介图标/按钮文案适配
  const primaryAction = production ? authoritativeItem?.primaryAction : undefined

  const getPrimaryActionLabel = () => {
    if (production) {
      if (!primaryAction) return "打开"
      if (primaryAction.labelHint === "continue") return "继续"
      if (primaryAction.labelHint === "start") return "开始"
      return "打开"
    }
    if (media.progress && media.progress > 0) {
      if (media.type === "movie" || media.type === "tv") return "继续播放"
      if (media.type === "periodical") return "继续翻阅"
      if (media.type === "document") return "继续查阅"
      return "继续阅读"
    }
    if (media.type === "movie" || media.type === "tv") return "播放"
    if (media.type === "periodical") return "翻阅"
    if (media.type === "document") return "查阅"
    return "阅读"
  }

  const getTypeLabel = () => {
    switch (media.type) {
      case "movie": return "电影"
      case "tv": return "剧集"
      case "book": return "图书"
      case "comic": return "漫画"
      case "periodical": return "报刊"
      case "document": return "资料"
      case "article": return "文章"
      default: return "媒体"
    }
  }

  const getTabContentsLabel = () => {
    switch (media.type) {
      case "tv": return "剧集与选集"
      case "comic": return "单行本与话数"
      case "movie": return "影片资源"
      case "periodical": return "往期刊物与分册"
      case "document": return "文档目录与附录"
      default: return "章节与目录"
    }
  }

  const detailState: WorkDetailState = !production
    ? "data"
    : detailLoading
      ? "loading"
      : authoritativeItem?.id === media.id
        ? "data"
        : detailError?.dto.retryable
          ? "retryable_error"
          : "terminal_error"
  const favoriteCanWrite = !production || (!detailLoading && authoritativeItem?.id === media.id)
  const editionState: EditionListState = getEditionListState(production, editionLoading, editionItems, editionError)
  // The Work/Edition DTO exposes the selected media item, while the resource
  // summary owns the actual online-read capability.  Keep the visual action
  // in place but fail closed until that capability has been resolved; this is
  // what turns download-only providers (for example OPDS/Gutenberg EPUBs)
  // into an explicit "download first" state instead of a dead reader route.
  const primaryActionTarget = production && onlineReadCapability === "available"
    ? primaryActionRoute(primaryAction)
    : !production
      ? getConsumeRoute(media.type, media.id)
      : null
  const canConsume = canConsumeDetail(production, detailState) && canConsumeEdition(production, editionState, primaryActionTarget !== null)

  // Download state is a projection of server resources/tasks. It is deliberately
  // loaded only after authoritative detail data is ready, so stale responses
  // from a previous Work cannot change the newly selected page.
  useEffect(() => {
    if (!production || detailState !== "data" || !downloadableMediaItemId) {
      setDownloadStatus("idle")
      setDownloadCapability("unavailable")
      setOnlineReadCapability(production && detailState === "data" ? "unavailable" : production ? "loading" : "available")
      return
    }
    let cancelled = false
    setDownloadCapability("loading")
    setOnlineReadCapability("loading")
    getMediaItemDownloadInfo(downloadableMediaItemId)
      .then((info) => {
        if (cancelled) return
        setDownloadStatus(info.status)
        setDownloadCapability(
          info.canDownload || info.hasOfflineResource || info.status === "queued"
            ? "available"
            : "unavailable",
        )
        setOnlineReadCapability(info.canOnlineRead ? "available" : "unavailable")
      })
      .catch(() => {
        if (!cancelled) {
          setDownloadStatus("idle")
          setDownloadCapability("error")
          setOnlineReadCapability("error")
        }
      })
    return () => {
      cancelled = true
    }
  }, [detailState, downloadableMediaItemId, production])

  return (
    <div data-slice-state={detailState} className="relative min-h-full w-full bg-background text-foreground flex flex-col overflow-x-hidden select-none">
      {/* Toast 提示弹窗 */}
      {toastMessage && (
        <div className="fixed top-[32px] left-1/2 -translate-x-1/2 z-[100] flex items-center gap-3 px-6 py-3.5 rounded-full bg-zinc-950/90 dark:bg-white/95 text-white dark:text-zinc-950 text-base font-bold shadow-2xl backdrop-blur-2xl border border-white/10 dark:border-black/10 animate-in fade-in slide-in-from-top-[16px] duration-300">
          <Download className="w-6 h-6 text-emerald-400 dark:text-emerald-600 shrink-0" />
          <span>{toastMessage}</span>
        </div>
      )}

      {/* 
        ========================================
        1. 顶栏浮动返回控件 (Back Button Header)
        ========================================
      */}
      <header className="fixed top-6 left-6 md:left-[48px] z-50 flex items-center">
        <button
          onClick={() => navigate(-1)}
          className={cn(
            "flex items-center gap-1.5 px-3 py-[8px] rounded-full",
            "text-foreground/80 hover:text-foreground hover:bg-black/5 dark:hover:bg-white/10 transition-all duration-200 cursor-pointer backdrop-blur-sm"
          )}
        >
          <ChevronLeft className="w-6 h-6" />
          <span className="text-sm font-semibold pr-1">返回</span>
        </button>
      </header>

      {/* 
        ========================================
        2. Hero 沉浸式头部区域
        ========================================
      */}
      <section className="relative w-full min-h-[500px] lg:min-h-[560px] flex items-end pt-28 pb-[48px] px-6 md:px-[64px] lg:px-[96px]">
        {/* 背景大图遮罩 — 完全对齐 HavenStage：brightness-[0.95] + via-background/50 统一背景 */}
        <div className="absolute inset-0 z-0 select-none overflow-hidden">
          <img
            src={media.backdropUrl || media.posterUrl}
            alt={media.title}
            className="w-full h-full object-cover object-top filter brightness-[0.95]"
          />
          {/* 深色渐变遮罩，使底部平滑融入页面底色 — 同 HavenStage */}
          <div className="absolute inset-0 bg-gradient-to-t from-background via-background/50 to-transparent" />
          {/* 左侧的额外遮罩，保证文字在复杂背景下的可读性 */}
          <div className="absolute inset-0 bg-gradient-to-r from-background via-background/75 to-transparent/10" />
        </div>

        {/* Hero 主内容 */}
        <div className="relative z-10 w-full max-w-6xl mx-auto flex flex-col md:flex-row items-end md:items-center gap-10 md:gap-14">
          {/* 左侧封面卡片 (去除大阴影，改为极简边框) */}
          <div className="w-[160px] sm:w-[200px] lg:w-[240px] aspect-[3/4] rounded-xl overflow-hidden border border-black/10 dark:border-white/10 shrink-0 transform transition-transform hover:scale-[1.02] duration-300 bg-muted">
            <ArtworkImage
              src={media.posterUrl}
              alt={media.title}
              allowExternal={getHavenClientMode() !== "tauri"}
              fallbackCategory={defaultCoverCategoryForMediaType(media.type)}
              fallbackSeed={media.id}
              className="w-full h-full object-cover"
            />
          </div>

          {/* 右侧详细元数据与操作 */}
          <div className="flex flex-col gap-6 flex-1 w-full">
            
            {/* 标题与元数据区 */}
            <div className="flex flex-col gap-3">
              {/* 顶栏小标签组 */}
              <div className="flex flex-wrap items-center gap-[8px]">
                <span className="text-xs font-bold tracking-widest uppercase text-muted-foreground">
                  {getTypeLabel()}
                </span>
                <span className="w-1 h-1 rounded-full bg-muted-foreground/50" />
                <span className="text-xs font-semibold text-muted-foreground">
                  {media.year}
                </span>
                {media.quality && (
                  <>
                    <span className="w-1 h-1 rounded-full bg-muted-foreground/50" />
                    <span className="px-1.5 py-0.5 rounded-[4px] border border-black/10 dark:border-white/15 text-[10px] font-bold text-foreground/80 uppercase tracking-wider">
                      {media.quality}
                    </span>
                  </>
                )}
                {media.format && (
                  <>
                    <span className="w-1 h-1 rounded-full bg-muted-foreground/50" />
                    <span className="px-1.5 py-0.5 rounded-[4px] border border-black/10 dark:border-white/15 text-[10px] font-bold text-foreground/80 uppercase tracking-wider">
                      {media.format}
                    </span>
                  </>
                )}
              </div>

              {/* 标题 */}
              <div className="flex flex-col gap-1">
                <h1 className="text-4xl sm:text-5xl lg:text-[56px] font-black tracking-tight text-foreground leading-[1.1]">
                  {media.title}
                </h1>
                {media.originalTitle && (
                  <p className="text-sm font-semibold text-muted-foreground tracking-wide mt-1">
                    {media.originalTitle}
                  </p>
                )}
              </div>

              {/* 作者/导演/出版社 */}
              <div className="text-sm font-semibold text-foreground/90 flex items-center gap-[8px] mt-1">
                <span>{media.authorOrDirector}</span>
                {media.publisherOrStudio && (
                  <>
                    <span className="opacity-40">•</span>
                    <span className="text-muted-foreground">{media.publisherOrStudio}</span>
                  </>
                )}
                {media.rating && (
                  <>
                    <span className="opacity-40">•</span>
                    <span className="flex items-center gap-1 text-amber-500">
                      <Star className="w-3.5 h-3.5 fill-current" />
                      {media.rating}
                    </span>
                  </>
                )}
              </div>

              {/* 简介：LineClamp 3 + 渐变 + 展开 */}
              {(() => {
                const needsClamp = media.description.length > 120
                return (
                  <div className="relative max-w-3xl mt-[8px]">
                    <p className={cn("text-sm text-foreground/75 leading-relaxed font-medium", !isDescriptionExpanded && needsClamp && "line-clamp-3")}>
                      {media.description}
                    </p>
                    {!isDescriptionExpanded && needsClamp && (
                      <div className="absolute inset-x-0 bottom-0 h-6 bg-gradient-to-t from-background to-transparent pointer-events-none" />
                    )}
                    {needsClamp && (
                      <button
                        type="button"
                        onClick={() => setIsDescriptionExpanded(!isDescriptionExpanded)}
                        className="mt-1 text-xs font-semibold text-primary hover:underline"
                      >
                        {isDescriptionExpanded ? "收起" : "展开更多"}
                      </button>
                    )}
                  </div>
                )
              })()}
            </div>

            {/* 操作按钮区 */}
            <div className="flex flex-wrap items-center gap-[16px] pt-6 pb-[8px]">
              <div className="flex flex-col gap-[8px]">
                <div className="flex items-center gap-3.5">
                  {/* 主操作：使用黑色背景(暗色模式下为白色)，放大图标与字体 */}
                  <button
                    onClick={() => {
                      if (canConsume && primaryActionTarget) navigate(primaryActionTarget)
                    }}
                    disabled={!canConsume}
                    aria-disabled={!canConsume}
                    className={cn(
                      "flex items-center justify-center gap-3 px-9 h-14 rounded-full font-bold text-base shadow-none",
                      "bg-foreground text-background transition-all duration-300 hover:scale-105 active:scale-95 cursor-pointer",
                      !canConsume && "cursor-not-allowed opacity-60 hover:scale-100 active:scale-100"
                    )}
                  >
                    {media.type === "movie" || media.type === "tv" ? (
                      <Play className="w-6 h-6 fill-current shrink-0" />
                    ) : (
                      <BookOpen className="w-6 h-6 fill-current shrink-0" />
                    )}
                    <span className="tracking-wide">{getPrimaryActionLabel()}</span>
                  </button>

                  {production && detailState !== "data" && (
                    <div className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
                      <span>{detailState === "retryable_error" ? "作品信息加载失败" : detailState === "loading" ? "正在加载作品信息…" : "作品信息暂不可用"}</span>
                      {detailState === "retryable_error" && (
                        <button
                          type="button"
                          onClick={() => setDetailRetryNonce((value) => value + 1)}
                          className="font-semibold underline underline-offset-2 hover:text-foreground"
                        >
                          重试
                        </button>
                      )}
                    </div>
                  )}
                  {production && detailState === "data" && editionState !== "data" && editionState !== "empty" && (
                    <div className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
                      <span>{editionState === "retryable_error" ? "版本信息加载失败" : editionState === "loading" ? "正在加载版本信息…" : "版本信息暂不可用"}</span>
                      {editionState === "retryable_error" && (
                        <button
                          type="button"
                          onClick={() => setEditionRetryNonce((value) => value + 1)}
                          className="font-semibold underline underline-offset-2 hover:text-foreground"
                        >
                          重试
                        </button>
                      )}
                    </div>
                  )}
                  {production && detailState === "data" && onlineReadCapability === "unavailable" && (
                    <div className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
                      <span>{downloadCapability === "available" ? "需要下载后阅读" : "当前内容暂不可在线阅读"}</span>
                    </div>
                  )}
                  {production && detailState === "data" && onlineReadCapability === "error" && (
                    <div className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
                      <span>在线阅读能力读取失败，请重试</span>
                    </div>
                  )}

                  {/* 次级操作组：图标明显放大 (w-14 h-14 容器，w-6 h-6 图标) */}
                  <button
                    onClick={handleFavoriteToggle}
                    disabled={!favoriteCanWrite || isFavoriteLoading || isFavoriteSaving}
                    title="收藏"
                    className={cn(
                      "w-14 h-14 rounded-full flex items-center justify-center transition-all duration-300 cursor-pointer shrink-0",
                      "bg-black/5 dark:bg-white/10 hover:bg-black/10 dark:hover:bg-white/20 hover:scale-105 active:scale-95",
                      isFavorite ? "text-red-500 bg-red-500/10 dark:bg-red-500/20 border-red-500/30" : "text-foreground"
                    )}
                  >
                    <Heart className={cn("w-6 h-6", isFavorite && "fill-current")} />
                  </button>

                  <button
                    onClick={handleDownloadAction}
                    disabled={isDownloadSaving || downloadCapability !== "available" || !production}
                    title={downloadStatus === "downloaded"
                      ? "已下载"
                      : downloadStatus === "queued"
                        ? "已加入下载列表"
                        : !production
                          ? "下载功能仅在桌面应用中可用"
                          : downloadCapability === "loading"
                            ? "正在读取下载能力"
                            : downloadCapability === "error"
                              ? "下载能力读取失败"
                              : downloadCapability === "unavailable"
                                ? "当前内容没有可下载资源"
                                : "下载至本地"}
                    aria-label={downloadStatus === "downloaded" ? "已下载" : "下载至本地"}
                    className={cn(
                      "w-14 h-14 rounded-full flex items-center justify-center transition-all duration-300 cursor-pointer shrink-0",
                      "bg-black/5 dark:bg-white/10 hover:bg-black/10 dark:hover:bg-white/20 hover:scale-105 active:scale-95",
                      (downloadCapability !== "available" || !production) && "cursor-not-allowed opacity-50 hover:scale-100 active:scale-100",
                      downloadStatus === "queued"
                        ? "text-[#007aff] bg-[#007aff]/10 dark:bg-[#007aff]/20"
                        : downloadStatus === "downloaded"
                          ? "text-emerald-500 bg-emerald-500/10 dark:bg-emerald-500/20"
                          : "text-foreground"
                    )}
                  >
                    <Download className="w-6 h-6" />
                  </button>

                  <button
                    onClick={() => setIsShareModalOpen(true)}
                    title="生成分享卡片"
                    className="w-14 h-14 rounded-full flex items-center justify-center bg-black/5 dark:bg-white/10 text-foreground hover:bg-black/10 dark:hover:bg-white/20 hover:scale-105 active:scale-95 transition-all duration-300 cursor-pointer shrink-0"
                  >
                    <Share2 className="w-6 h-6" />
                  </button>

                  {/* 更多 Popover */}
                  <div className="relative shrink-0">
                    <button
                      onClick={() => setIsMoreMenuOpen(!isMoreMenuOpen)}
                      title="更多"
                      className="w-14 h-14 rounded-full flex items-center justify-center bg-black/5 dark:bg-white/10 text-foreground hover:bg-black/10 dark:hover:bg-white/20 hover:scale-105 active:scale-95 transition-all duration-300 cursor-pointer"
                    >
                      <Ellipsis className="w-6 h-6" />
                    </button>

                    {isMoreMenuOpen && (
                      <>
                        <div className="fixed inset-0 z-40" onClick={() => setIsMoreMenuOpen(false)} />
                        <div className={cn(
                          "absolute left-0 bottom-full mb-3 w-56 p-[10px] rounded-2xl z-50 shadow-2xl",
                          "bg-white/90 dark:bg-zinc-900/90 backdrop-blur-2xl border border-black/10 dark:border-white/15",
                          "flex flex-col gap-1.5 animate-in fade-in zoom-in-95 duration-150"
                        )}>
                          <button 
                            onClick={() => void handleMoreAction("folder")}
                            className="flex items-center gap-3.5 px-[16px] py-3 rounded-xl text-base font-medium text-foreground hover:bg-black/5 dark:hover:bg-white/10 transition-colors text-left whitespace-nowrap"
                          >
                            <Folder className="w-5 h-5 text-muted-foreground shrink-0" />
                            在文件夹中定位
                          </button>
                          <button 
                            onClick={() => void handleMoreAction("reset")}
                            className="flex items-center gap-3.5 px-[16px] py-3 rounded-xl text-base font-medium text-foreground hover:bg-black/5 dark:hover:bg-white/10 transition-colors text-left whitespace-nowrap"
                          >
                            <RotateCcw className="w-5 h-5 text-muted-foreground shrink-0" />
                            重置进度
                          </button>
                          <div className="h-px bg-black/5 dark:bg-white/10 my-1.5" />
                          <button 
                            onClick={() => void handleMoreAction("cache")}
                            className="flex items-center gap-3.5 px-[16px] py-3 rounded-xl text-base font-medium text-destructive hover:bg-destructive/10 transition-colors text-left whitespace-nowrap"
                          >
                            <Trash2 className="w-5 h-5 shrink-0" />
                            清理本地缓存
                          </button>
                        </div>
                      </>
                    )}
                  </div>
                </div>

                {/* 上次定位文本 */}
                {(media.progressText || downloadStatus !== "idle" || (production && downloadCapability !== "available")) && (
                  <div className="flex items-center gap-[8px] text-xs font-semibold text-muted-foreground ml-[8px] mt-1">
                    {media.progressText && <span>上次定位：{media.progressText}</span>}
                    {downloadStatus === "queued" && <span className="text-[#007aff]">· 已加入下载列表</span>}
                    {downloadStatus === "downloaded" && <span className="text-emerald-500">· 已下载</span>}
                    {downloadStatus === "idle" && downloadCapability === "loading" && <span>· 正在读取下载能力</span>}
                    {downloadStatus === "idle" && downloadCapability === "unavailable" && <span>· 暂无可下载资源</span>}
                    {downloadStatus === "idle" && downloadCapability === "error" && <span>· 下载能力读取失败</span>}
                  </div>
                )}
              </div>
            </div>
          </div>
        </div>
      </section>

      {/* 
        ========================================
        3. 下半部分：Tab 导航与定制化内容排版
        ========================================
      */}
      <main className="w-full max-w-5xl mx-auto px-6 md:px-[64px] pt-[64px]">
        {/* Tab Header Segmented Control */}
        <div className="flex items-center gap-14 border-b border-border/40 pb-3 mb-[32px]">
          <button
            onClick={() => setActiveTab("contents")}
            className={cn(
              "text-base font-bold transition-all duration-200 cursor-pointer pb-3 -mb-[13px] border-b-2",
              activeTab === "contents"
                ? "text-foreground border-foreground"
                : "text-muted-foreground border-transparent hover:text-foreground"
            )}
          >
            {getTabContentsLabel()}
          </button>
          <button
            onClick={() => setActiveTab("overview")}
            className={cn(
              "text-base font-bold transition-all duration-200 cursor-pointer pb-3 -mb-[13px] border-b-2",
              activeTab === "overview"
                ? "text-foreground border-foreground"
                : "text-muted-foreground border-transparent hover:text-foreground"
            )}
          >
            详细介绍
          </button>
          <button
            onClick={() => setActiveTab("specs")}
            className={cn(
              "text-base font-bold transition-all duration-200 cursor-pointer pb-3 -mb-[13px] border-b-2",
              activeTab === "specs"
                ? "text-foreground border-foreground"
                : "text-muted-foreground border-transparent hover:text-foreground"
            )}
          >
            技术规格与来源
          </button>
        </div>

        {/* TAB 1: 剧集或章节目录 (Contents Tab) */}
        {activeTab === "contents" && (
          <div className="flex flex-col gap-6">
            {production && editionState === "empty" && (
              <div className="py-5 text-sm font-semibold text-muted-foreground" role="status">暂无可用版本</div>
            )}
            {production && editionState !== "data" && editionState !== "empty" && (
              <div className="py-5 text-sm font-semibold text-muted-foreground" role="status">
                {editionState === "retryable_error" ? "版本信息加载失败，请重试" : editionState === "loading" ? "正在加载版本信息…" : "版本信息暂不可用"}
              </div>
            )}
            {/* 如果是 TV 剧集，展示季度选择器 */}
            {media.type === "tv" && media.seasons && (
              <div className="flex flex-wrap items-center gap-[12px]">
                {media.seasons.map((s) => (
                  <button
                    key={s.id}
                    onClick={() => setSelectedSeason(s.id)}
                    className={cn(
                      "inline-flex min-h-[44px] items-center justify-center rounded-full px-5 text-sm font-bold transition-colors cursor-pointer",
                      selectedSeason === s.id
                        ? "bg-foreground text-background"
                        : "bg-black/5 dark:bg-white/5 text-muted-foreground hover:text-foreground"
                    )}
                  >
                    {s.name}
                  </button>
                ))}
              </div>
            )}

            {media.episodesOrChapters && media.episodesOrChapters.length > 1 && (
              <div className="flex items-center justify-between">
                <span className="text-xs font-bold text-muted-foreground uppercase tracking-widest">
                  共 {media.episodesOrChapters.length} 集
                </span>
                <button
                  type="button"
                  aria-label={isAscending ? "正序" : "倒序"}
                  data-testid="work-sort-toggle"
                  onClick={() => setIsAscending((v) => !v)}
                  className="inline-flex items-center gap-1.5 rounded-full border border-black/10 dark:border-white/15 bg-black/5 dark:bg-white/5 px-3 py-1.5 text-xs font-bold text-muted-foreground hover:text-foreground transition-colors cursor-pointer"
                >
                  {isAscending ? <ArrowUp className="w-3.5 h-3.5" /> : <ArrowDown className="w-3.5 h-3.5" />}
                  <ArrowUpDown className="w-3 h-3 opacity-60" />
                  {isAscending ? "正序" : "倒序"}
                </button>
              </div>
            )}

            {/* 多媒介类型版本分区：仅生产分支且当前页含 ≥2 种类型时展示 */}
            {production && editionGroups.length > 1 && (
              <div className="flex w-fit max-w-full gap-[4px] overflow-x-auto rounded-lg bg-muted/60 p-[4px]" role="tablist" aria-label="版本媒介类型">
                {editionGroups.map((group) => (
                  <button
                    key={group.mediaType}
                    type="button"
                    role="tab"
                    aria-selected={activeGroup?.mediaType === group.mediaType}
                    onClick={() => setActiveEditionType(group.mediaType)}
                    className={cn(
                      "inline-flex shrink-0 items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-bold transition-colors cursor-pointer",
                      activeGroup?.mediaType === group.mediaType
                        ? "bg-foreground text-background"
                        : "text-muted-foreground hover:text-foreground"
                    )}
                  >
                    {group.label}
                    <span className="opacity-60 tabular-nums">{group.items.length}</span>
                  </button>
                ))}
              </div>
            )}

            {/* 针对 TV 剧集的图文卡片式单集列表 */}
            {media.type === "tv" && (
              <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-5">
                {sortedActiveEpisodes?.map((ep) => (
                  <div
                    key={ep.id}
                    onClick={() => {
                      void handleEditionOpen(ep.primaryAction, ep.id)
                    }}
                    aria-busy={editionOpeningId === ep.id}
                    className={cn(
                      "flex flex-col gap-3 p-3.5 rounded-2xl bg-black/5 dark:bg-white/5 border border-black/5 dark:border-white/5 hover:border-black/20 dark:hover:border-white/20 transition-all duration-300 group cursor-pointer",
                      editionOpeningId === ep.id && "opacity-70 cursor-wait",
                    )}
                  >
                    <div className="relative w-full aspect-video rounded-xl overflow-hidden bg-muted shrink-0">
                      <ArtworkImage
                        src={ep.thumbnail || media.posterUrl}
                        alt={ep.title}
                        allowExternal={getHavenClientMode() !== "tauri"}
                        fallbackCategory={defaultCoverCategoryForMediaType(media.type)}
                        fallbackSeed={`${media.id}:${ep.id}`}
                        className="w-full h-full object-cover"
                      />
                      <div className="absolute inset-0 bg-black/20 flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity">
                        <div className="w-[48px] h-[48px] rounded-full bg-background/90 flex items-center justify-center shadow-lg">
                          <Play className="w-6 h-6 text-foreground fill-current ml-0.5" />
                        </div>
                      </div>
                      {ep.progress && ep.progress > 0 && (
                        <div className="absolute bottom-0 left-0 right-0 h-1 bg-black/40">
                          <div className="h-full bg-foreground" style={{ width: `${ep.progress}%` }} />
                        </div>
                      )}
                    </div>
                    <div className="flex flex-col px-1">
                      <div className="flex items-center justify-between gap-[8px] mb-1">
                        <span className="text-xs font-bold text-muted-foreground">{ep.number}</span>
                        <span className="text-xs font-semibold text-muted-foreground">{ep.durationOrPages}</span>
                      </div>
                      <h4 className="text-base font-bold text-foreground truncate group-hover:text-primary transition-colors">
                        {ep.title}
                      </h4>
                      {ep.isDownloaded && (
                        <span className="inline-flex items-center gap-1.5 text-xs font-bold text-emerald-500 mt-[8px]">
                          <CheckCircle className="w-3.5 h-3.5" /> 已下载
                        </span>
                      )}
                    </div>
                  </div>
                ))}
              </div>
            )}

            {/* 针对 漫画/图书/电影/文章 的列表：高度与字体调大 (根据用户反馈图片2) */}
            {media.type !== "tv" && (
              <div className="flex flex-col">
                <div className="flex items-center justify-between text-xs md:text-sm font-bold text-muted-foreground uppercase tracking-widest px-[16px] pb-[16px] border-b border-border/40">
                  <span>名称</span>
                  <span>时长/大小</span>
                </div>
                <div className="flex flex-col divide-y divide-border/40">
                  {sortedActiveEpisodes?.map((chap) => (
                    <div
                      key={chap.id}
                      onClick={() => {
                        void handleEditionOpen(chap.primaryAction, chap.id)
                      }}
                      aria-busy={editionOpeningId === chap.id}
                      className={cn(
                        "flex items-center justify-between py-5 px-[16px] -mx-[16px] rounded-2xl hover:bg-black/5 dark:hover:bg-white/5 transition-colors cursor-pointer group gap-[16px]",
                        editionOpeningId === chap.id && "opacity-70 cursor-wait",
                      )}
                    >
                      <div className="flex items-center gap-5 min-w-0">
                        <div className="flex items-center justify-center shrink-0 group-hover:scale-110 transition-transform">
                          {media.type === 'movie' ? <Play className="w-6 h-6 text-foreground fill-current" /> : <BookOpen className="w-6 h-6 text-foreground" />}
                        </div>
                        <div className="flex flex-col min-w-0 gap-0.5">
                          <span className="text-base md:text-lg font-bold text-foreground truncate group-hover:text-primary transition-colors">
                            {chap.title}
                          </span>
                          <span className="text-xs md:text-sm font-semibold text-muted-foreground">
                            {chap.number}
                          </span>
                        </div>
                      </div>
                      <div className="flex items-center gap-[16px] shrink-0 pr-[32px]">
                        <span className="text-sm font-semibold text-muted-foreground">{chap.durationOrPages}</span>
                        {chap.isDownloaded && (
                          <CheckCircle className="w-5 h-5 text-emerald-500" />
                        )}
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        )}

        {/* TAB 2: 详细介绍 (Overview Tab) */}
        {activeTab === "overview" && (
          <div className="flex flex-col gap-[32px] max-w-3xl">
            <div className="flex flex-col gap-3">
              <h3 className="text-sm font-bold text-muted-foreground uppercase tracking-widest">梗概</h3>
              <p className="text-base md:text-lg text-foreground/90 leading-relaxed font-medium">
                {media.description}
              </p>
            </div>
            
            <div className="grid grid-cols-2 sm:grid-cols-3 gap-y-6 gap-x-4 pt-6 border-t border-border/40">
              <div className="flex flex-col gap-1">
                <span className="text-xs font-bold text-muted-foreground uppercase tracking-widest">主创 / 作者</span>
                <span className="text-base font-bold text-foreground">{media.authorOrDirector}</span>
              </div>
              <div className="flex flex-col gap-1">
                <span className="text-xs font-bold text-muted-foreground uppercase tracking-widest">发行 / 出版商</span>
                <span className="text-base font-bold text-foreground">{media.publisherOrStudio || "未知"}</span>
              </div>
              <div className="flex flex-col gap-1">
                <span className="text-xs font-bold text-muted-foreground uppercase tracking-widest">发行年份</span>
                <span className="text-base font-bold text-foreground">{media.year}</span>
              </div>
            </div>

            <div className="flex flex-col gap-[16px] pt-[32px] border-t border-border/40">
              <div>
                <h3 className="text-sm font-bold text-muted-foreground uppercase tracking-widest">作品关系</h3>
                <p className="mt-[8px] text-sm text-muted-foreground">原作、改编与同系列作品。点击关系卡直接跳转到对应作品的详情页。</p>
              </div>

              {/* 当前版本 + 内容单元 双卡 */}
              <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                <button
                  type="button"
                  onClick={() => setActiveTab("contents")}
                  className="flex items-center gap-3 rounded-2xl border border-border/60 bg-background/70 p-[16px] text-left transition-colors hover:border-primary/40 hover:bg-muted/40"
                >
                  <span className="flex h-[40px] w-[40px] shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary"><MediaTypeIcon type={media.type} /></span>
                  <span className="min-w-0"><span className="block text-xs font-semibold text-muted-foreground">当前版本</span><span className="mt-1 block truncate text-sm font-bold">{getTypeLabel()}</span><span className="mt-1 block text-xs text-muted-foreground">{media.year} · 继续打开</span></span>
                </button>
                <button
                  type="button"
                  onClick={() => setActiveTab("contents")}
                  className="flex items-center gap-3 rounded-2xl border border-border/60 bg-background/70 p-[16px] text-left transition-colors hover:border-primary/40 hover:bg-muted/40"
                >
                  <span className="flex h-[40px] w-[40px] shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary"><BookOpen className="h-[18px] w-[18px]" /></span>
                  <span className="min-w-0"><span className="block text-xs font-semibold text-muted-foreground">内容单元</span><span className="mt-1 block truncate text-sm font-bold">{media.episodesOrChapters?.length || 0} 个可打开项</span><span className="mt-1 block text-xs text-muted-foreground">查看季、集或章节</span></span>
                </button>
              </div>

              {/* 关联作品列表 */}
              {media.relations && media.relations.length > 0 ? (
                <div className="flex flex-col divide-y divide-border/40 overflow-hidden rounded-2xl border border-border/60 bg-background/70">
                  {media.relations.map((rel) => (
                    <button
                      key={rel.workId}
                      type="button"
                      onClick={() => navigate(`/work/${rel.workId}`)}
                      className="group flex h-[72px] items-center justify-between gap-[16px] px-[16px] py-3 text-left transition-colors hover:bg-muted/40"
                    >
                      <div className="flex items-center gap-[16px] min-w-0">
                        <span className="flex h-[40px] w-[40px] shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary"><Link2 className="h-[18px] w-[18px]" /></span>
                        <span className="flex flex-col min-w-0 gap-1">
                          <span className="flex min-w-0 items-center gap-[8px]">
                            <span className="shrink-0 rounded-[4px] border border-primary/30 bg-primary/10 px-1.5 py-0.5 text-[10px] font-bold uppercase tracking-wider text-primary">{rel.role}</span>
                            <span className="truncate text-xs font-semibold text-muted-foreground">{rel.meta}</span>
                          </span>
                          <span className="truncate text-sm font-bold text-foreground transition-colors group-hover:text-primary">{rel.title}</span>
                        </span>
                      </div>
                      <ChevronRight className="h-[16px] w-[16px] shrink-0 text-muted-foreground/50 transition-all group-hover:translate-x-0.5 group-hover:text-primary" />
                    </button>
                  ))}
                </div>
              ) : (
                <div className="rounded-2xl border border-dashed border-border/60 px-[16px] py-6 text-center">
                  <span className="text-sm text-muted-foreground">暂无已收录的关联作品</span>
                </div>
              )}
            </div>
          </div>
        )}

        {/* TAB 3: 技术规格与来源 (Specs Tab) - 生产无 size/specs 时收起，避免红框内大留白 */}
        {activeTab === "specs" && (
          <div className="flex flex-col max-w-3xl">
            { !media.size && !media.specs?.resolution && !media.specs?.codec && !media.specs?.audio && !media.specs?.language && !media.specs?.wordCount && !media.specs?.frequency && !media.specs?.issn && !media.specs?.pageCount && !media.specs?.docVersion && !media.specs?.orientation ? (
              <div className="rounded-2xl border border-dashed border-border/60 px-[16px] py-12 text-center">
                <span className="text-sm text-muted-foreground">暂无技术规格</span>
              </div>
            ) : (
            <div className="flex flex-col divide-y divide-border/40">
              
              {media.size && (
              <div className="flex items-center justify-between py-5 gap-6">
                <span className="text-base font-semibold text-muted-foreground whitespace-nowrap shrink-0">文件大小</span>
                <span className="text-base font-bold text-foreground text-right">{media.size}</span>
              </div>
              )}

              {media.specs?.resolution && (
                <div className="flex items-center justify-between py-5 gap-6">
                  <span className="text-base font-semibold text-muted-foreground whitespace-nowrap shrink-0">分辨率 / 画质</span>
                  <span className="text-base font-bold text-foreground text-right">{media.specs.resolution}</span>
                </div>
              )}

              {media.specs?.codec && (
                <div className="flex items-center justify-between py-5 gap-6">
                  <span className="text-base font-semibold text-muted-foreground whitespace-nowrap shrink-0">编码格式</span>
                  <span className="text-base font-bold text-foreground text-right">{media.specs.codec}</span>
                </div>
              )}

              {media.specs?.audio && (
                <div className="flex items-center justify-between py-5 gap-6">
                  <span className="text-base font-semibold text-muted-foreground whitespace-nowrap shrink-0">音轨信息</span>
                  <span className="text-base font-bold text-foreground text-right">{media.specs.audio}</span>
                </div>
              )}

              {media.specs?.language && (
                <div className="flex items-center justify-between py-5 gap-6">
                  <span className="text-base font-semibold text-muted-foreground whitespace-nowrap shrink-0">语言与字幕</span>
                  <span className="text-base font-bold text-foreground text-right">{media.specs.language}</span>
                </div>
              )}

              {media.specs?.wordCount && (
                <div className="flex flex-col sm:flex-row sm:items-center justify-between py-5 gap-6">
                  <span className="text-base font-semibold text-muted-foreground whitespace-nowrap shrink-0">字数统计</span>
                  <span className="text-base font-bold text-foreground text-right">{media.specs.wordCount}</span>
                </div>
              )}

              {media.specs?.frequency && (
                <div className="flex items-center justify-between py-5 gap-6">
                  <span className="text-base font-semibold text-muted-foreground whitespace-nowrap shrink-0">发行频率</span>
                  <span className="text-base font-bold text-foreground text-right">{media.specs.frequency}</span>
                </div>
              )}

              {media.specs?.issn && (
                <div className="flex items-center justify-between py-5 gap-6">
                  <span className="text-base font-semibold text-muted-foreground whitespace-nowrap shrink-0">刊号 (ISSN)</span>
                  <span className="text-base font-bold text-foreground text-right">{media.specs.issn}</span>
                </div>
              )}

              {media.specs?.pageCount && (
                <div className="flex items-center justify-between py-5 gap-6">
                  <span className="text-base font-semibold text-muted-foreground whitespace-nowrap shrink-0">页数统计</span>
                  <span className="text-base font-bold text-foreground text-right">{media.specs.pageCount}</span>
                </div>
              )}

              {media.specs?.docVersion && (
                <div className="flex items-center justify-between py-5 gap-6">
                  <span className="text-base font-semibold text-muted-foreground whitespace-nowrap shrink-0">文档版本号</span>
                  <span className="text-base font-bold text-foreground text-right">{media.specs.docVersion}</span>
                </div>
              )}

              {media.specs?.orientation && (
                <div className="flex items-center justify-between py-5 gap-6">
                  <span className="text-base font-semibold text-muted-foreground whitespace-nowrap shrink-0">阅读翻页方向</span>
                  <span className="text-base font-bold text-foreground text-right">{media.specs.orientation}</span>
                </div>
              )}

              <div className="flex items-center justify-between py-5 gap-6">
                <span className="text-base font-semibold text-muted-foreground whitespace-nowrap shrink-0">来源</span>
                <span className="text-base font-bold text-emerald-500 flex items-center gap-[8px] text-right">
                  <CheckCircle className="w-5 h-5" /> 本地媒体库已同步
                </span>
              </div>
              
            </div>
            )}
          </div>
        )}
      </main>

      {/* 分享卡片弹窗 */}
      <ShareCardModal
        isOpen={isShareModalOpen}
        onClose={() => setIsShareModalOpen(false)}
        data={{
          title: media.title,
          originalTitle: media.originalTitle,
          metadata: `${media.year} · ${getTypeLabel()}`,
          description: media.description,
          backdropUrl: media.backdropUrl
        }}
      />
    </div>
  )
}

function MediaTypeIcon({ type }: { type: MediaDetailData["type"] }) {
  if (type === "tv") return <Tv className="h-[18px] w-[18px]" />
  if (type === "movie") return <Film className="h-[18px] w-[18px]" />
  if (type === "book") return <Book className="h-[18px] w-[18px]" />
  if (type === "comic") return <BookOpen className="h-[18px] w-[18px]" />
  return <FileText className="h-[18px] w-[18px]" />
}
