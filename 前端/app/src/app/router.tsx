import React from "react"
import { createBrowserRouter, Navigate, useParams } from "react-router"
import type { RouteObject } from "react-router"
import { AppShell } from "./layouts/AppShell"
import { LibraryPage } from "@/features/library/pages/LibraryPage"
import { LibraryBrowsePage } from "@/features/library/pages/LibraryBrowsePage"
import { HomePage } from "@/features/home/pages/HomePage"
import { FootprintsPage } from "@/features/footprints/pages/FootprintsPage"
import { SearchPage } from "@/features/search/pages/SearchPage"
import { DownloadsPage } from "@/features/downloads/pages/DownloadsPage"
import { MediaDetailPage } from "@/features/media/pages/MediaDetailPage"
import { PlayerPage } from "@/features/player/pages/PlayerPage"
import { SettingsPage } from "@/features/settings/pages/SettingsPage"
import { BookReaderPage } from "@/features/reader/pages/BookReaderPage"
import { ComicReaderPage } from "@/features/comic/pages/ComicReaderPage"
import { ArticleReaderPage } from "@/features/reader/pages/ArticleReaderPage"
import { HistoryPage } from "@/features/footprints/pages/HistoryPage"
import { EditionDetailPage } from "@/features/media/pages/EditionDetailPage"

export class AppErrorBoundary extends React.Component<{ children: React.ReactNode }, { hasError: boolean; error: unknown }> {
  state = { hasError: false, error: null as unknown }
  static getDerivedStateFromError(error: unknown) { return { hasError: true, error } }
  componentDidCatch(error: unknown, info: unknown) { console.error("[AppErrorBoundary]", error, info) }
  render() {
    if (this.state.hasError) {
      return (
        <div className="flex h-[60vh] flex-col items-center justify-center gap-4 p-8 text-center">
          <h1 className="text-xl font-bold">页面遇到了问题</h1>
          <p className="max-w-md text-sm text-muted-foreground">错误：{String((this.state.error as Error)?.message ?? this.state.error).slice(0,200)}</p>
          <button type="button" onClick={() => this.setState({ hasError: false, error: null })} className="rounded-full bg-primary px-4 py-2 text-sm font-semibold text-primary-foreground">重试</button>
          <button type="button" onClick={() => location.reload()} className="text-xs text-muted-foreground underline">刷新页面</button>
        </div>
      )
    }
    return this.props.children
  }
}

// SPIKE-FOLIATE-001：诊断路由仅在 dev 模式或显式 VITE_SPIKE_ENABLED=1 构建中存在。
// 常规生产构建两个条件均为假，整块消除（含对 spike 页面的动态导入），产物零影响。
let spikeRoute: RouteObject | null = null
if (import.meta.env.DEV || import.meta.env.VITE_SPIKE_ENABLED === "1") {
  const SpikePage = React.lazy(() => import("@/spike/foliate/FoliateSpikePage"))
  spikeRoute = {
    path: "dev/foliate-spike",
    element: <React.Suspense fallback={null}><SpikePage /></React.Suspense>,
  }
}

export const router = createBrowserRouter([
  ...spikeRoute ? [spikeRoute] : [],
  {
    path: "player/:mediaItemId",
    element: <PlayerPage />
  },
  {
    path: "reader/:mediaItemId",
    element: <BookReaderPage />
  },
  {
    path: "comic/:mediaItemId",
    element: <ComicReaderPage />
  },
  {
    path: "article/:mediaItemId",
    element: <ArticleReaderPage />
  },
  {
    path: "/",
    element: <AppShell />,
    children: [
      {
        index: true,
        element: <HomePage />
      },
      {
        path: "library",
        element: <LibraryPage />
      },
      {
        path: "library/movies",
        element: <Navigate replace to="/library/browse/video" />
      },
      {
        path: "library/browse/:category",
        element: <LibraryBrowsePage />
      },
      {
        path: "work/:workId",
        element: <MediaDetailPage />
      },
      {
        path: "edition/:editionId",
        element: <EditionDetailPage />
      },
      {
        path: "media/:id",
        element: <LegacyMediaRedirect />
      },
      {
        path: "footprints",
        element: <FootprintsPage />
      },
      {
        path: "footprints/history",
        element: <HistoryPage />
      },
      {
        path: "search",
        element: <SearchPage />
      },
      {
        path: "downloads",
        element: <DownloadsPage />
      },
      {
        path: "settings/:section?",
        element: <SettingsPage />
      },
      {
        path: "*",
        element: (
          <div className="p-[32px] max-w-6xl mx-auto flex flex-col items-center justify-center h-[60vh] space-y-[16px]">
            <div className="w-[64px] h-[64px] rounded-2xl bg-muted flex items-center justify-center mb-[16px]">
              <span className="text-2xl">🚧</span>
            </div>
            <h1 className="text-2xl font-bold">页面不存在 (404)</h1>
            <p className="text-muted-foreground">您访问的页面（如旧版的收藏/标记）已被移除或重构。</p>
          </div>
        )
      }
    ]
  }
])

function LegacyMediaRedirect() {
  const { id } = useParams<{ id?: string }>()
  return <Navigate replace to={id ? `/work/${id}` : "/library"} />
}
