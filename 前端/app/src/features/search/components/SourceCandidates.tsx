// 来源候选区（V2-B 实战批次）：渐进式搜索的来源结果 + 一键导入。
// 六态遵循契约 §27：loading / success_empty / success_data / partial（warning 计数）
// / cancelled / error。本地结果不受来源失败影响。

import { useEffect, useRef, useState } from "react"
import { useNavigate } from "react-router"
import { ChevronDown, ChevronRight, Download, Globe, Loader2, SearchX } from "lucide-react"

import { ArtworkImage } from "@/components/ui/haven/ArtworkImage"
import { defaultCoverCategoryForMediaType } from "@/lib/default-cover"
import type { QueryCategory, WorkCardDto } from "@/lib/ipc/generated/wire"
import { toHavenError } from "@/lib/ipc/errors"
import {
  cancelSourceSearch,
  importSourceWork,
  startSourceSearch,
} from "../ipc/source-search-gateway"
import {
  accumulateWarning,
  sourceDisplayName,
  warningLineText,
  type SourceWarning,
} from "../lib/source-warnings"

/** 与 gateway 的 SOURCE_SEARCH_BUSY_CODE 同名：仅用于选择用户文案，不展示内部状态名。 */
const SOURCE_SEARCH_BUSY = "SOURCE_SEARCH_BUSY"

interface Candidate {
  index: number
  work: WorkCardDto
  sourceId: string | null
}

function canImportCandidate(workId: string): boolean {
  // CMS10、Gutenberg OPDS 以及四个固定正文来源具备完整的受控导入链路。
  // 自定义 OPDS 目前只有真实搜索能力，后端也会拒绝正文导入，因此不能
  // 在这里显示一个点击后必然失败的“导入媒体库”按钮。
  return workId.startsWith("cms10-candidate-")
    || workId.startsWith("opds-candidate-opds_gutenberg")
    || workId.startsWith("content-candidate-")
}

type SectionStatus = "idle" | "searching" | "done" | "failed"

export function SourceCandidates({
  query,
  category,
  retryRevision = 0,
}: {
  query: string
  /** 页面分段筛选；all 时来源搜索不限定分类。 */
  category: QueryCategory | "all"
  /** 父页面在同关键词显式提交时递增；搜索进行中到达的信号会被忽略。 */
  retryRevision?: number
}) {
  const navigate = useNavigate()
  const sourceCategory: QueryCategory | undefined =
    category === "all" ? undefined : category
  const [candidates, setCandidates] = useState<Candidate[]>([])
  const [status, setStatus] = useState<SectionStatus>("idle")
  const [errorCode, setErrorCode] = useState<string | null>(null)
  const [warnings, setWarnings] = useState<SourceWarning[]>([])
  const [warningsOpen, setWarningsOpen] = useState(false)
  const [importingIndex, setImportingIndex] = useState<number | null>(null)
  const [importError, setImportError] = useState<string | null>(null)
  // 重试版本号由本组件持有：父页面只下发"同关键词显式提交"的信号，
  // 是否真的重查由这里在终态决定。
  const [retryNonce, setRetryNonce] = useState(0)
  const operationIdRef = useRef<string | null>(null)
  const searchEpochRef = useRef(0)
  const statusRef = useRef<SectionStatus>("idle")
  const observedRetryRevisionRef = useRef(retryRevision)

  useEffect(() => {
    statusRef.current = status
  }, [status])

  // 外部重试信号只消费一次：先记下已观察到的版本号，再判断是否忙碌。
  // 搜索进行中被忽略的信号不会在终态到达后被重放；进行中的操作也不能被
  // 取消重启——后端对同一 query 去重，可能对取消中的操作返回 alreadyRunning，
  // 从而丢失候选与导入索引身份。
  useEffect(() => {
    if (observedRetryRevisionRef.current === retryRevision) return
    observedRetryRevisionRef.current = retryRevision
    if (statusRef.current === "searching") return
    setRetryNonce((current) => current + 1)
  }, [retryRevision])

  const retrySourceSearch = () => {
    if (status === "searching") return
    setRetryNonce((current) => current + 1)
  }

  useEffect(() => {
    searchEpochRef.current += 1
    setImportingIndex(null)
    if (!query) {
      operationIdRef.current = null
      setCandidates([])
      setStatus("idle")
      return undefined
    }
    let cancelled = false
    let runningIndex = 0
    // 启动返回值和事件都可能先到；清理时用已拿到的身份取消，
    // 若清理早于启动返回，则用返回的 operationId 补一次取消。
    let cancelWhenStartReturns = false
    operationIdRef.current = null
    setCandidates([])
    setErrorCode(null)
    setWarnings([])
    setWarningsOpen(false)
    setImportError(null)
    setStatus("searching")

    void startSourceSearch(query, (event) => {
      if (cancelled) return
      if (operationIdRef.current === null) operationIdRef.current = event.operationId
      else if (operationIdRef.current !== event.operationId) return
      switch (event.kind) {
        case "source_result": {
          const mapped = event.data.works.map((work) => ({ index: runningIndex++, work, sourceId: event.data.sourceId }))
          setCandidates((prev) => [...prev, ...mapped])
          break
        }
        case "completed":
          setStatus("done")
          break
        case "cancelled":
          setStatus("idle")
          break
        case "failed":
          setErrorCode(event.data.code ?? "INTERNAL_ERROR")
          setStatus("failed")
          break
        case "warning":
          setWarnings((prev) => accumulateWarning(prev, event))
          break
        default:
          break
      }
    }, sourceCategory)
      .then((result) => {
        if (cancelled) {
          // 清理发生在启动返回之前：用返回的身份补一次取消，避免遗留运行中的操作。
          if (cancelWhenStartReturns) void cancelSourceSearch(result.operationId).catch(() => undefined)
          return
        }
        if (operationIdRef.current === null) operationIdRef.current = result.operationId
      })
      .catch((error: unknown) => {
        if (cancelled) return
        setErrorCode(toHavenError(error).code)
        setStatus("failed")
      })

    return () => {
      cancelled = true
      searchEpochRef.current += 1
      const operationId = operationIdRef.current
      operationIdRef.current = null
      if (operationId === null) {
        cancelWhenStartReturns = true
        return
      }
      void cancelSourceSearch(operationId).catch(() => undefined)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, sourceCategory, retryNonce])

  const importCandidate = async (candidate: Candidate) => {
    const operationId = operationIdRef.current
    if (operationId === null || importingIndex !== null) return
    const epoch = searchEpochRef.current
    const isCurrent = () => searchEpochRef.current === epoch && operationIdRef.current === operationId
    setImportingIndex(candidate.index)
    setImportError(null)
    try {
      const result = await importSourceWork({ operationId, index: candidate.index })
      if (isCurrent()) navigate(`/work/${result.workId}`)
    } catch (error) {
      if (isCurrent()) setImportError(toHavenError(error).dto.userMessage)
    } finally {
      if (isCurrent()) setImportingIndex(null)
    }
  }

  if (status === "idle") return null

  // 相同关键词的来源搜索尚未结束时，本次订阅拿不到完整历史，来源候选与
  // 导入索引都不可信；提示原因并把恢复方式留给显式重试。
  const busyFailure = status === "failed" && errorCode === SOURCE_SEARCH_BUSY

  return (
    <section className="search-results__section" aria-labelledby="source-results-heading" aria-busy={status === "searching"}>
      <header className="search-results__section-header">
        <div className="search-results__section-title">
          <Globe size={18} aria-hidden="true" />
          <h2 id="source-results-heading">来源结果</h2>
          <span className="search-results__count">{candidates.length}</span>
        </div>
        <span className="search-results__section-hint" role="status">
          {status === "searching" ? <><Loader2 size={14} className="animate-spin" aria-hidden="true" />正在查询已启用来源…</>
            : status === "failed" ? "查询未完成"
            : warnings.length > 0 ? candidates.length > 0 ? "部分来源已返回" : "来源未返回结果"
            : "已启用的外部来源"}
        </span>
      </header>
      {status === "failed" && errorCode !== null && (
        <div className="search-results__notice" role="alert">
          <div className="search-results__notice-toggle">
            <span>
              {busyFailure
                ? "相同关键词的来源搜索尚未结束，暂时不能重新查询；本地结果不受影响。"
                : `来源搜索暂时不可用（${errorCode ?? "UNKNOWN"}），本地结果不受影响。`}
            </span>
            <button type="button" onClick={() => { setErrorCode(null) }} className="search-results__action">知道了</button>
          </div>
        </div>
      )}
      {warnings.length > 0 && (
        <div className="search-results__notice">
          <button
            type="button"
            aria-expanded={warningsOpen}
            onClick={() => { setWarningsOpen((open) => !open) }}
            className="search-results__notice-toggle"
          >
            <span>
              {status === "searching"
                ? `有 ${warnings.length} 个来源暂未返回结果，仍在查询其他来源。`
                : candidates.length > 0
                  ? `有 ${warnings.length} 个来源未返回结果，已展示其余来源。`
                  : `有 ${warnings.length} 个来源未返回结果，当前没有可展示的来源结果。`}
            </span>
            <span className="search-results__notice-action">
              {warningsOpen ? "收起明细" : "查看明细"}
              {warningsOpen
                ? <ChevronDown size={14} aria-hidden="true" />
                : <ChevronRight size={14} aria-hidden="true" />}
            </span>
          </button>
          {warningsOpen && (
            <ul className="search-results__warning-list">
              {warnings.map((warning, i) => (
                <li key={`${warning.sourceId}-${i}`}>
                  <span className="font-semibold">{sourceDisplayName(warning.sourceId)}</span>
                  {" · "}
                  {warningLineText(warning)}
                  <span className="search-results__warning-code">
                    {warning.code}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
      {status !== "searching" && (status === "failed" || warnings.length > 0) && (
        <div className="search-results__notice">
          <div className="search-results__notice-toggle">
            <span>
              {status === "failed"
                ? busyFailure
                  ? "来源搜索尚未结束，可以稍后重新查询已启用来源。"
                  : "来源搜索未完成，可重新查询已启用来源。"
                : "部分来源未返回结果，可重新查询已启用来源。"}
            </span>
            <button
              type="button"
              onClick={retrySourceSearch}
              className="search-results__action"
            >
              重试
            </button>
          </div>
        </div>
      )}
      {candidates.length === 0 && status === "done" && (
        <div className="search-results__empty search-results__empty--source" role="status">
          <SearchX size={24} aria-hidden="true" />
          <div>
            <p className="font-medium text-foreground">
              {warnings.length > 0 ? "当前没有可展示的来源条目" : "来源中暂无匹配结果"}
            </p>
            <p className="mt-1">
              {warnings.length > 0
                ? "请展开上方明细查看未返回来源的原因。"
                : "已启用来源没有返回匹配条目，可尝试其他关键词。"}
            </p>
          </div>
        </div>
      )}
      {candidates.length > 0 && (
        <div className="search-results__grid">
          {candidates.map(({ index, work, sourceId }) => (
            <article
              key={`${work.workId}-${index}`}
              className="search-result-card search-result-card--source"
            >
              <div className="search-result-card__cover">
                <ArtworkImage
                  src={work.posterUri ?? work.backdropUri}
                  alt={work.title}
                  fallbackCategory={defaultCoverCategoryForMediaType(work.categories[0] ?? work.availableMediaTypes[0])}
                  fallbackSeed={work.workId}
                  loading="lazy"
                />
              </div>
              <div className="search-result-card__body">
                <h3 className="search-result-card__title">{work.title}</h3>
                {work.description && <p className="search-result-card__description">{work.description}</p>}
                <div className="search-result-card__metadata">
                  <span className="search-result-card__source-name">{sourceId ? sourceDisplayName(sourceId) : "来源候选"}</span>
                  {work.releaseYear !== null && <span>{work.releaseYear}</span>}
                  {work.availableMediaTypes.length > 0 && <span>{work.availableMediaTypes.join(" / ")}</span>}
                </div>
              </div>
              {canImportCandidate(work.workId) ? (
                <button
                  type="button"
                  disabled={importingIndex !== null}
                  onClick={() => { void importCandidate({ index, work, sourceId }) }}
                  aria-label={`${importingIndex === index ? "导入中…" : "导入媒体库"}：${work.title}`}
                  className="search-results__action"
                >
                  {importingIndex === index ? <Loader2 size={13} className="animate-spin" aria-hidden="true" /> : <Download size={13} aria-hidden="true" />}
                  {importingIndex === index ? "导入中…" : "导入媒体库"}
                </button>
              ) : (
                <span className="search-result-card__read-only">
                  仅搜索
                </span>
              )}
            </article>
          ))}
        </div>
      )}
      {importError && <p className="search-results__notice" role="alert">{importError}</p>}
    </section>
  )
}
