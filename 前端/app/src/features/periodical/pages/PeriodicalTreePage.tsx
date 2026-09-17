// 期刊层级浏览页（React UI → Feature Gateway → HavenClient）。
//
// 事实来源只有一个：`getPeriodicalTree(workId)`。页面不直接调用 Tauri、SQLite、
// Provider 或任何 URL，也不从标题、DOI、来源 key 推断归属——后端返回的
// 卷 / 期 / 文章顺序就是事实顺序，这里只按数组顺序渲染。
//
// 层级事实是「为某个 workId 读到的一棵树」：新一轮请求开始的同一刻，上一棵树与由它
// 派生的阅读能力一起作废（见 `periodical-tree-load-state`），因此 workId 已经变了、
// 响应还没回来的那段窗口里，页面上不会残留上一个作品的层级，也不会残留它的
// readable 结论——哪怕新作品复用了同一个 mediaItemId。
//
// 正文入口只使用既有 `article.mediaItemId` 导航到 `/article/:mediaItemId`。
// 可读性来自既有 download Feature 的能力投影（批量读取一次，单篇重查复用同一条投影
// 规则），状态区分 loading / readable / preparing / downloading / download_failed /
// download_only / metadata_only / unavailable / unknown；只有 readable 会导航，
// `unknown` 表示「这次探测失败」而不是「不可用」——投影里缺条目同样归入 unknown，
// 缺失的事实不会被写成「明确不可用」的结论。
//
// download_only 不是死状态：后端给了可下载来源，页面就必须提供一个真实可执行的动作。
// 点击只调用既有 `createDownloadForMediaItem(mediaItemId)`，绝不直接发 IPC、拼 URL 或
// 查 SQLite，也绝不因此导航到阅读器——**创建成功不等于可读**。只有重新读到的真实能力
// （离线资源或在线读取能力）才把文章升级成 readable；仅有离线资源时也绝不称其为在线阅读。
//
// 下载生命周期：创建请求在途时按钮禁用并说明正在创建；任务排队/下载中时保持「下载中」
// 并禁用重复创建；失败时显示可读原因并允许重试，不把失败伪装成「不可用」或「已读」。
// 完成/失败/取消的下载事件到达时按 (taskId → mediaItemId, 代) 归属重新读取该篇的真实
// 能力；组件卸载、换作品与旧代响应都会让事件与响应失效，旧下载事件不能改变新树的结论。
// 归属还要随当前投影对账：只有投影仍说这一篇在下载（queued 且带 taskId）时才保留，
// 任务一旦进入终态而被重新投影成 idle / downloaded，它的 taskId 就立即注销——同一个
// taskId 的迟到事件不再被当成当前任务，也不能在用户重试之后改写这一行的状态。
//
// `article.availability` 是 Provider 的正文观察，只在没有真实可读路径时用来把结论说
// 清楚（`metadata_only` → 仅有元数据 · 正文未开放）；它**不参与**能否打开的判断，
// 用户本地已有的在线或离线内容始终按真实资源路径打开。它同时是下载动作边界的一部分：
// 这一篇的观察随动作一起交给 `handleArticleDownload`，`metadata_only` 在那里被再次拒绝，
// 迟到的点击或直接调用都不会让来源明确没有正文的一篇建出下载任务。
//
// 失败即失败：错误由 Gateway / HavenError 承担，页面不回退到空树或演示数据。

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import type { ReactNode } from "react"
import { ArrowLeft, CircleAlert, FileText, LoaderCircle } from "lucide-react"
import { useNavigate, useParams } from "react-router"
import type {
  PeriodicalArticleDto,
  PeriodicalIssueDto,
  PeriodicalVolumeDto,
} from "@/lib/ipc/generated/wire"
import { HavenError, toHavenError } from "@/lib/ipc/errors"
import { getHavenClientMode } from "@/lib/ipc/runtime"
import {
  createDownloadForMediaItem,
  getMediaItemsDownloadInfo,
  subscribeDownloadEvents,
} from "@/features/downloads/ipc/download-gateway"
import { downloadErrorMessage } from "@/features/downloads/lib/download-error"
import { getPeriodicalTree } from "../ipc/periodical-gateway"
import { createArticleCapabilityRequestRegistry } from "../lib/periodical-capability-requests"
import {
  resolvePeriodicalTreeLoadView,
  treeLoadFailed,
  treeLoadPending,
  treeLoadReady,
  type PeriodicalTreeLoad,
} from "../lib/periodical-tree-load-state"
import {
  NO_DOWNLOAD_ATTEMPT,
  activeDownloadTaskId,
  canStartArticleDownload,
  failedArticleCapabilities,
  pendingArticleCapabilities,
  periodicalArticleAccessibleName,
  projectArticleCapabilities,
  resolvePeriodicalArticleReadState,
  type ArticleCapability,
  type ArticleDownloadAttempt,
  type PeriodicalArticleReadPresentation,
  type PeriodicalArticleReadState,
  type ProviderContentAvailability,
} from "../lib/periodical-article-read-state"
import {
  articleMetaText,
  collectArticleMediaItemIds,
  issnEntries,
  issueHeading,
  issueMeta,
  volumeHeading,
  volumeMeta,
} from "../lib/periodical-tree-view"

const TERMINAL_COPY: Record<string, { title: string; detail: string }> = {
  PERIODICAL_NOT_FOUND: {
    title: "该作品没有期刊层级",
    detail: "该作品没有已登记的期刊或卷期，无法浏览。",
  },
  INVALID_ARGUMENT: {
    title: "期刊作品标识无效",
    detail: "作品标识格式非法，请从作品详情重新进入。",
  },
}

/** 按钮上的可见动作文案：必须说出这个按钮此刻真正会做什么。 */
const ARTICLE_BUTTON_LABEL: Record<PeriodicalArticleReadState, string> = {
  readable: "打开正文",
  preparing: "准备下载",
  downloading: "下载中",
  download_failed: "重试下载",
  download_only: "下载后阅读",
  metadata_only: "仅元数据",
  loading: "读取能力",
  unknown: "重试",
  unavailable: "暂不可阅读",
}

/** 一次下载任务的身份归属：哪一个 mediaItemId、在哪一代树里创建。 */
interface DownloadTaskOwner {
  mediaItemId: string
  generation: number
}

export function PeriodicalTreePage() {
  const { workId } = useParams<{ workId?: string }>()
  const navigate = useNavigate()
  const mode = getHavenClientMode()
  const [treeLoad, setTreeLoad] = useState<PeriodicalTreeLoad>(treeLoadPending)
  const requestSequence = useRef(0)
  const capabilityGeneration = useRef(0)
  // 在途登记表跨代存在，代际由 generation 区分（每次渲染多建的那个实例会被丢弃）。
  const capabilityRequests = useRef(createArticleCapabilityRequestRegistry()).current
  const capabilitiesRef = useRef<Record<string, ArticleCapability>>({})
  const [capabilities, setCapabilities] = useState<Record<string, ArticleCapability>>({})
  // 下载动作事实（创建在途 / 最近一次失败）与能力事实分开记账：它只影响按钮此刻
  // 能做什么，永远不参与「能不能读」的判断。
  const downloadAttemptsRef = useRef<Record<string, ArticleDownloadAttempt>>({})
  const [downloadAttempts, setDownloadAttempts] = useState<Record<string, ArticleDownloadAttempt>>({})
  // 下载任务身份归属：事件只带 taskId，这里是唯一能把事件映射回文章的地方。
  const taskOwners = useRef(new Map<string, DownloadTaskOwner>())

  // ref 与 state 同步写入：点击时的复核读的是 ref，不能落后于最后一次渲染。
  const replaceCapabilities = useCallback((next: Record<string, ArticleCapability>) => {
    capabilitiesRef.current = next
    setCapabilities(next)
  }, [])

  const replaceDownloadAttempts = useCallback((next: Record<string, ArticleDownloadAttempt>) => {
    downloadAttemptsRef.current = next
    setDownloadAttempts(next)
  }, [])

  const updateCapability = useCallback((mediaItemId: string, next: ArticleCapability) => {
    replaceCapabilities({ ...capabilitiesRef.current, [mediaItemId]: next })
  }, [replaceCapabilities])

  const setDownloadAttempt = useCallback((mediaItemId: string, next: ArticleDownloadAttempt) => {
    replaceDownloadAttempts({ ...downloadAttemptsRef.current, [mediaItemId]: next })
  }, [replaceDownloadAttempts])

  /**
   * 记下「这个 taskId 属于这一代的哪一篇」，同时把不再成立的关系注销。
   *
   * 下载事件只携带 taskId，归属连同代一起记账：旧代留下的归属在新一代里不能匹配到
   * 任何东西。归属还必须跟着当前投影走——任务到达终态（完成 / 失败 / 取消）之后，
   * 投影会重新落成 `idle` 或 `downloaded`，此时那个 taskId 已经没有任何可监听的东西，
   * 留着它就会让同一个 taskId 的迟到事件被当成当前任务，重新刷新甚至改写这一行。
   * 因此这里按 (代, mediaItemId) 对账：只保留投影仍说「在下载」的那个 taskId。
   */
  const noteTaskOwners = useCallback((landed: Record<string, ArticleCapability>, generation: number) => {
    for (const [mediaItemId, capability] of Object.entries(landed)) {
      // 只有真正读到投影的那一篇才对账：这次没拿到事实（探测失败、投影缺条目）时
      // 归属不动——缺失的事实既不能登记一个新任务，也不能注销一个仍可能有效的归属。
      if (capability.info === null || capability.failed) continue
      const activeTaskId = activeDownloadTaskId(capability.info)
      for (const [knownTaskId, owner] of taskOwners.current) {
        if (owner.generation !== generation || owner.mediaItemId !== mediaItemId) continue
        // 投影仍指向的这一个保留；同一篇在这一代里其余的旧 taskId 一律注销。
        if (knownTaskId === activeTaskId) continue
        taskOwners.current.delete(knownTaskId)
      }
      // 投影仍说这一篇在下载（包括新树重新读到同一个活动任务）：归属继续有效。
      if (activeTaskId !== null) taskOwners.current.set(activeTaskId, { mediaItemId, generation })
    }
  }, [])

  const load = useCallback(async () => {
    if (!workId || mode === "unavailable") {
      // 没有可承载事实的 Work：页面本来就不会渲染层级，旧树与旧能力一并作废。
      setTreeLoad(treeLoadPending)
      replaceCapabilities({})
      replaceDownloadAttempts({})
      return
    }
    const requestId = ++requestSequence.current
    // 请求开始的这一刻旧事实就作废：旧树与由它派生的阅读能力都不再属于当前作品，
    // 不能留在页面上等响应回来才换。这里落成「正在读取」，而不是空树、也不是成功——
    // 清空旧事实与「后端确认没有卷期」必须彼此可辨。
    setTreeLoad(treeLoadPending)
    replaceCapabilities({})
    replaceDownloadAttempts({})
    try {
      const next = await getPeriodicalTree(workId)
      if (requestSequence.current === requestId) setTreeLoad(treeLoadReady(workId, next))
    } catch (cause) {
      if (requestSequence.current === requestId) {
        setTreeLoad(treeLoadFailed(workId, cause instanceof HavenError ? cause : toHavenError(cause)))
      }
    }
  }, [workId, mode, replaceCapabilities, replaceDownloadAttempts])

  useEffect(() => {
    void load()
    return () => { requestSequence.current += 1 }
  }, [load])

  // 只有「为当前 workId 读到的」树才算当前事实：workId 已经变了而新请求还没开始的
  // 那一帧里，上一棵树连同它的阅读能力都不再可见。
  const view = resolvePeriodicalTreeLoadView(treeLoad, workId)
  const currentTree = view.state === "ready" ? view.tree : null

  const articleMediaItemIds = useMemo(
    () => (currentTree ? collectArticleMediaItemIds(currentTree) : []),
    [currentTree],
  )

  useEffect(() => {
    // 新一代：上一代的在途重查由登记表按代作废，这里不需要（也不能）清空它——
    // 旧代请求收尾时若顺手抹掉新代的登记，同一篇的去重就失效了。
    const generation = ++capabilityGeneration.current
    // 下载归属同属这一代，随代一并作废：旧代的任务事件不能落到新树的行上。
    taskOwners.current.clear()
    replaceDownloadAttempts({})
    if (articleMediaItemIds.length === 0) {
      replaceCapabilities({})
      return
    }
    replaceCapabilities(pendingArticleCapabilities(articleMediaItemIds))

    void getMediaItemsDownloadInfo(articleMediaItemIds).then((projected) => {
      if (capabilityGeneration.current !== generation) return
      // 投影缺条目时保持 unknown：缺失的事实不能当成「明确不可用」的结论。
      const landed = projectArticleCapabilities(articleMediaItemIds, projected)
      replaceCapabilities(landed)
      noteTaskOwners(landed, generation)
    }).catch(() => {
      if (capabilityGeneration.current !== generation) return
      replaceCapabilities(failedArticleCapabilities(articleMediaItemIds))
    })
  }, [articleMediaItemIds, replaceCapabilities, replaceDownloadAttempts, noteTaskOwners])

  /**
   * 单篇重查：同一篇在**同一代**里已有在途请求时不再重复发起，避免一次点击打出一串
   * 相同请求。登记按代记账，上一代的在途请求收尾时只会解除它自己那一代的登记，
   * 不会把这一代仍在途的标记清掉。
   *
   * `generation` 由调用方传入：下载事件带来的是它自己那一代的归属，旧代的刷新既不该
   * 写进新代的能力记录，也不该把一个已经作废的结论贴到新树上。
   */
  const refreshCapability = useCallback(async (
    mediaItemId: string,
    generation = capabilityGeneration.current,
  ) => {
    if (capabilityGeneration.current !== generation) return
    const release = capabilityRequests.begin(mediaItemId, generation)
    if (release === null) return
    updateCapability(mediaItemId, { info: null, failed: false })
    try {
      // 与批量查询共用同一条投影规则：投影里没有这一篇时保持 unknown。
      // 不能退回单篇助手的空投影兜底——那会把「这次没拿到事实」写成「明确不可用」。
      const projected = await getMediaItemsDownloadInfo([mediaItemId])
      if (capabilityGeneration.current !== generation) return
      const landed = projectArticleCapabilities([mediaItemId], projected)[mediaItemId]
      updateCapability(mediaItemId, landed)
      noteTaskOwners({ [mediaItemId]: landed }, generation)
    } catch {
      if (capabilityGeneration.current !== generation) return
      updateCapability(mediaItemId, failedArticleCapabilities([mediaItemId])[mediaItemId])
    } finally {
      release()
    }
  }, [updateCapability, capabilityRequests, noteTaskOwners])

  // 下载生命周期：任务到达终态时重新读取真实能力。
  //
  // 订阅在页面实例上建一次，归属与代际在事件到达时逐条复核，因此卸载、换作品、换树
  // 都不会让旧下载事件改变当前的结论；进度事件不触发重读，避免一次下载打出一串查询。
  useEffect(() => {
    if (mode !== "tauri") return
    let mounted = true
    let dispose: (() => Promise<void>) | null = null
    void subscribeDownloadEvents((event) => {
      if (!mounted) return
      const owner = taskOwners.current.get(event.data.taskId)
      // 认不出的任务、或属于上一代树的任务：一律丢弃，绝不据此改动当前页面。
      if (!owner || owner.generation !== capabilityGeneration.current) return
      if (!capabilitiesRef.current[owner.mediaItemId]) return
      if (event.data.state === "failed" || event.data.state === "interrupted") {
        // 终态失败只带稳定错误码，交给受控码表翻译成可读原因。
        setDownloadAttempt(owner.mediaItemId, {
          creating: false,
          error: downloadErrorMessage(event.data.errorCode ?? ""),
        })
      } else if (event.data.state === "cancelled") {
        // 取消不是失败：撤销本地的失败/在途标记，回到「可下载」这个真实状态。
        setDownloadAttempt(owner.mediaItemId, NO_DOWNLOAD_ATTEMPT)
      } else if (event.data.state !== "completed") {
        return
      }
      void refreshCapability(owner.mediaItemId, owner.generation)
    }).then((cleanup) => {
      if (mounted) dispose = cleanup
      else void cleanup().catch(() => undefined)
    }).catch(() => undefined)
    return () => {
      mounted = false
      if (dispose) void dispose().catch(() => undefined)
    }
  }, [mode, refreshCapability, setDownloadAttempt])

  const handleArticleOpen = useCallback((
    mediaItemId: string,
    providerAvailability: ProviderContentAvailability,
  ) => {
    const current = capabilitiesRef.current[mediaItemId]
    // 点击时重新核对缓存里的能力，而不是沿用渲染时的旧值。下载动作事实同样按点击
    // 那一刻取值，但它只可能让这一篇更打不开——它能决定的是「下载/重试下载」。
    const presentation = resolvePeriodicalArticleReadState(
      current?.info,
      current?.failed ?? false,
      providerAvailability,
      downloadAttemptsRef.current[mediaItemId] ?? NO_DOWNLOAD_ATTEMPT,
    )
    if (presentation.canOpen) {
      navigate(`/article/${mediaItemId}`)
      return
    }
    // 只有「探测失败」值得再查一次；其余都是明确结论，不重试。
    if (presentation.state === "unknown") void refreshCapability(mediaItemId)
  }, [navigate, refreshCapability])

  const handleRefreshCapability = useCallback((mediaItemId: string) => {
    void refreshCapability(mediaItemId)
  }, [refreshCapability])

  /**
   * 「下载后阅读」是这一页唯一会创建下载的地方。
   *
   * 它只调用既有 `createDownloadForMediaItem(mediaItemId)`：页面不碰 IPC、URL、SQLite，
   * 也不因为创建成功就导航或改口说可读——创建成功只说明任务建好了，能不能读要由重新
   * 读到的真实能力决定。
   *
   * 动作边界自己复核，不依赖按钮的禁用状态：`metadata_only` 是关于来源的结论，迟到的
   * 点击与直接调用都不该为一个来源明确没有正文的 MediaItem 建出任务（判据见
   * `canStartArticleDownload`）。能力事实同样按点击那一刻的 ref 取值，而不是渲染时的旧值。
   */
  const handleArticleDownload = useCallback(async (
    mediaItemId: string,
    providerAvailability: ProviderContentAvailability,
  ) => {
    const generation = capabilityGeneration.current
    const current = capabilitiesRef.current[mediaItemId]
    // 没有真实可下载来源就不发起：canDownload 是后端能力投影，不是本地推断。
    // 探测失败同样不发起——那说明这次没拿到事实。
    if (!canStartArticleDownload(current?.info, current?.failed ?? false, providerAvailability)) return
    // 点击时复核在途标记：同一篇已有创建请求时不重复提交（不依赖渲染时的旧值）。
    if (downloadAttemptsRef.current[mediaItemId]?.creating === true) return
    setDownloadAttempt(mediaItemId, { creating: true, error: null })
    try {
      const task = await createDownloadForMediaItem(mediaItemId)
      if (capabilityGeneration.current !== generation) return
      taskOwners.current.set(task.taskId, { mediaItemId, generation })
      // 创建成功不等于可读：只有重新读到的真实能力才决定这一篇此刻是什么状态。
      await refreshCapability(mediaItemId, generation)
      if (capabilityGeneration.current === generation) setDownloadAttempt(mediaItemId, NO_DOWNLOAD_ATTEMPT)
    } catch (cause) {
      if (capabilityGeneration.current !== generation) return
      setDownloadAttempt(mediaItemId, {
        creating: false,
        error: cause instanceof HavenError ? cause.message : "创建下载任务失败",
      })
    }
  }, [refreshCapability, setDownloadAttempt])

  if (mode === "unavailable") {
    return (
      <StatePanel
        title="当前环境无法读取本地期刊数据"
        detail="浏览器预览不会伪造本地期刊层级。请在桌面应用中打开。"
        onBack={() => navigate(-1)}
      />
    )
  }
  if (!workId) {
    return (
      <StatePanel
        title="期刊作品标识无效"
        detail="缺少作品标识，请从作品详情重新进入。"
        onBack={() => navigate("/library")}
      />
    )
  }
  if (view.state === "pending") {
    return (
      <StatePanel
        title="正在加载期刊层级"
        detail="正在读取真实的卷、期与文章。"
        icon={<LoaderCircle className="h-6 w-6 animate-spin" />}
        onBack={() => navigate(-1)}
      />
    )
  }
  if (view.state === "failed") {
    const { error } = view
    const copy = TERMINAL_COPY[error.code]
    return (
      <StatePanel
        title={copy?.title ?? "期刊层级加载失败"}
        detail={copy?.detail ?? error.message}
        retry={error.retryable ? load : undefined}
        onBack={() => navigate(-1)}
      />
    )
  }

  // 到这里只剩 ready：渲染用的树与能力查询用的 `currentTree` 是同一个对象。
  const tree = view.tree
  const issns = issnEntries(tree.periodical.issnPrint, tree.periodical.issnElectronic)

  return (
    <main data-slice-state="ready" className="min-h-full bg-background px-6 py-8 text-foreground md:px-12">
      <button
        type="button"
        className="mb-8 inline-flex items-center gap-2 text-sm font-semibold text-muted-foreground hover:text-foreground"
        onClick={() => navigate(`/work/${tree.workId}`)}
      >
        <ArrowLeft className="h-4 w-4" /> 返回作品
      </button>

      <header className="mb-8 max-w-3xl">
        <p className="text-xs font-bold uppercase tracking-[0.18em] text-muted-foreground">报刊层级</p>
        <h1 className="mt-2 text-3xl font-bold tracking-tight">{tree.periodical.title}</h1>
        <div className="mt-4 flex flex-wrap items-baseline gap-x-6 gap-y-2 text-sm text-muted-foreground">
          {issns.map((entry) => (
            <span key={entry.key} className="inline-flex items-baseline gap-1.5">
              <span className="font-semibold">{entry.label}</span>
              <span>{entry.value}</span>
            </span>
          ))}
          {tree.periodical.publisher !== null && <span>{tree.periodical.publisher}</span>}
        </div>
      </header>

      <section aria-labelledby="periodical-volumes-heading" className="max-w-4xl">
        <div className="mb-3 flex items-center justify-between">
          <h2 id="periodical-volumes-heading" className="text-lg font-bold">卷期与文章</h2>
          <span className="text-sm text-muted-foreground">{tree.volumes.length} 卷</span>
        </div>
        {tree.volumes.length === 0 ? (
          <div className="rounded-xl border border-dashed border-border px-5 py-10 text-center text-sm text-muted-foreground">
            该期刊暂无可浏览的卷期
          </div>
        ) : (
          <div className="flex flex-col gap-6">
            {tree.volumes.map((volume) => (
              <VolumeSection
                key={volume.id}
                volume={volume}
                capabilities={capabilities}
                downloadAttempts={downloadAttempts}
                onOpenArticle={handleArticleOpen}
                onDownloadArticle={handleArticleDownload}
                onRetryCapability={handleRefreshCapability}
              />
            ))}
          </div>
        )}
      </section>
    </main>
  )
}

function VolumeSection({
  volume,
  capabilities,
  downloadAttempts,
  onOpenArticle,
  onDownloadArticle,
  onRetryCapability,
}: {
  volume: PeriodicalVolumeDto
  capabilities: Record<string, ArticleCapability>
  downloadAttempts: Record<string, ArticleDownloadAttempt>
  onOpenArticle: (mediaItemId: string, providerAvailability: ProviderContentAvailability) => void
  onDownloadArticle: (mediaItemId: string, providerAvailability: ProviderContentAvailability) => void
  onRetryCapability: (mediaItemId: string) => void
}) {
  const meta = volumeMeta(volume)
  return (
    <section data-testid={`periodical-volume-${volume.id}`} className="rounded-xl border border-border">
      <header className="flex flex-wrap items-baseline justify-between gap-2 border-b border-border px-5 py-4">
        <h3 className="text-base font-bold">{volumeHeading(volume)}</h3>
        <span className="text-sm text-muted-foreground">{meta ?? `${volume.issues.length} 期`}</span>
      </header>
      {volume.issues.length === 0 ? (
        <p className="px-5 py-6 text-sm text-muted-foreground">本卷暂无可浏览的期号</p>
      ) : (
        <div className="divide-y divide-border">
          {volume.issues.map((issue) => (
            <IssueSection
              key={issue.id}
              issue={issue}
              capabilities={capabilities}
              downloadAttempts={downloadAttempts}
              onOpenArticle={onOpenArticle}
              onDownloadArticle={onDownloadArticle}
              onRetryCapability={onRetryCapability}
            />
          ))}
        </div>
      )}
    </section>
  )
}

function IssueSection({
  issue,
  capabilities,
  downloadAttempts,
  onOpenArticle,
  onDownloadArticle,
  onRetryCapability,
}: {
  issue: PeriodicalIssueDto
  capabilities: Record<string, ArticleCapability>
  downloadAttempts: Record<string, ArticleDownloadAttempt>
  onOpenArticle: (mediaItemId: string, providerAvailability: ProviderContentAvailability) => void
  onDownloadArticle: (mediaItemId: string, providerAvailability: ProviderContentAvailability) => void
  onRetryCapability: (mediaItemId: string) => void
}) {
  const meta = issueMeta(issue)
  return (
    <section data-testid={`periodical-issue-${issue.id}`} className="px-5 py-4">
      <header className="mb-3 flex flex-wrap items-baseline justify-between gap-2">
        <h4 className="text-sm font-bold">{issueHeading(issue)}</h4>
        <span className="text-sm text-muted-foreground">{meta ?? `${issue.articles.length} 篇`}</span>
      </header>
      {issue.articles.length === 0 ? (
        <p className="text-sm text-muted-foreground">本期暂无可展示的文章</p>
      ) : (
        <ul className="divide-y divide-border rounded-lg border border-border">
          {issue.articles.map((article) => (
            <ArticleRow
              key={article.id}
              article={article}
              capability={capabilities[article.mediaItemId]}
              downloadAttempt={downloadAttempts[article.mediaItemId]}
              onOpen={onOpenArticle}
              onDownload={onDownloadArticle}
              onRetry={onRetryCapability}
            />
          ))}
        </ul>
      )}
    </section>
  )
}

function ArticleRow({
  article,
  capability,
  downloadAttempt,
  onOpen,
  onDownload,
  onRetry,
}: {
  article: PeriodicalArticleDto
  capability: ArticleCapability | undefined
  downloadAttempt: ArticleDownloadAttempt | undefined
  onOpen: (mediaItemId: string, providerAvailability: ProviderContentAvailability) => void
  onDownload: (mediaItemId: string, providerAvailability: ProviderContentAvailability) => void
  onRetry: (mediaItemId: string) => void
}) {
  // Provider 的正文观察只影响「打不开时怎么说」，不影响能否打开。
  const presentation: PeriodicalArticleReadPresentation = resolvePeriodicalArticleReadState(
    capability?.info,
    capability?.failed ?? false,
    article.availability,
    downloadAttempt ?? NO_DOWNLOAD_ATTEMPT,
  )
  const meta = articleMetaText(article)
  // 三个真实动作各归各的状态：只有 readable 是打开正文，unknown 是重试读取能力，
  // 下载生命周期里仍有下载来源时才给出可执行的下载/重试下载。
  const opens = presentation.state === "readable"
  const retries = presentation.state === "unknown"
  const downloads = presentation.state === "download_only" || presentation.state === "download_failed"
  const buttonLabel = ARTICLE_BUTTON_LABEL[presentation.state]

  return (
    <li data-testid={`periodical-article-${article.id}`} className="flex items-start gap-4 px-4 py-4">
      <span className="mt-0.5 flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground">
        <FileText className="h-4 w-4" />
      </span>
      <div className="min-w-0 flex-1">
        <p className="font-semibold">{article.title}</p>
        {meta !== "" && <p className="mt-1 text-sm text-muted-foreground">{meta}</p>}
        <p className="mt-1 text-xs font-semibold text-muted-foreground">{presentation.label}</p>
      </div>
      <button
        type="button"
        aria-label={periodicalArticleAccessibleName(presentation, article.title)}
        title={presentation.label}
        disabled={!opens && !retries && !downloads}
        onClick={() => {
          if (opens) onOpen(article.mediaItemId, article.availability)
          else if (retries) onRetry(article.mediaItemId)
          // 下载动作连同这一篇的 Provider 观察一起交出去：动作边界要自己复核它，
          // 不能只靠这个按钮此刻是否可点。
          else if (downloads) onDownload(article.mediaItemId, article.availability)
        }}
        className="shrink-0 self-center rounded-lg border border-border px-3 py-2 text-sm font-semibold transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50"
      >
        {buttonLabel}
      </button>
    </li>
  )
}

function StatePanel({
  title,
  detail,
  icon = <CircleAlert className="h-6 w-6" />,
  retry,
  onBack,
}: {
  title: string
  detail: string
  icon?: ReactNode
  retry?: () => Promise<void>
  onBack: () => void
}) {
  return (
    <main className="flex min-h-full items-center justify-center bg-background px-6 py-12 text-foreground">
      <section className="w-full max-w-md rounded-xl border border-border p-8 text-center">
        <div className="mx-auto flex h-12 w-12 items-center justify-center rounded-full bg-muted text-muted-foreground">{icon}</div>
        <h1 className="mt-5 text-xl font-bold">{title}</h1>
        <p className="mt-2 text-sm leading-6 text-muted-foreground">{detail}</p>
        <div className="mt-6 flex justify-center gap-3">
          <button type="button" className="rounded-lg border border-border px-4 py-2 text-sm font-semibold hover:bg-muted" onClick={onBack}>返回</button>
          {retry && <button type="button" className="rounded-lg bg-foreground px-4 py-2 text-sm font-semibold text-background hover:opacity-90" onClick={() => void retry()}>重试</button>}
        </div>
      </section>
    </main>
  )
}
