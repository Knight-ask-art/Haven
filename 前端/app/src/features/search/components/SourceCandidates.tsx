// 来源候选区（V2-B 实战批次）：渐进式搜索的来源结果 + 一键导入。
// 六态遵循契约 §27：loading / success_empty / success_data / partial（warning 计数）
// / cancelled / error。本地结果不受来源失败影响。

import { useEffect, useRef, useState } from "react"
import { useNavigate } from "react-router"
import { ChevronDown, ChevronRight, Download, Globe, Loader2, SearchX, Server } from "lucide-react"

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
}: {
  query: string
  /** 页面分段筛选；all 时来源搜索不限定分类。 */
  category: QueryCategory | "all"
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
  const operationIdRef = useRef<string | null>(null)

  useEffect(() => {
    if (!query) {
      operationIdRef.current = null
      setCandidates([])
      setStatus("idle")
      return undefined
    }
    let cancelled = false
    let runningIndex = 0
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
      .catch((error: unknown) => {
        if (cancelled) return
        setErrorCode(toHavenError(error).code)
        setStatus("failed")
      })

    return () => {
      cancelled = true
      const operationId = operationIdRef.current
      if (operationId !== null) void cancelSourceSearch(operationId).catch(() => undefined)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, sourceCategory])

  const importCandidate = async (candidate: Candidate) => {
    const operationId = operationIdRef.current
    if (operationId === null || importingIndex !== null) return
    setImportingIndex(candidate.index)
    setImportError(null)
    try {
      const result = await importSourceWork({ operationId, index: candidate.index })
      navigate(`/work/${result.workId}`)
    } catch (error) {
      setImportError(toHavenError(error).dto.userMessage)
      setImportingIndex(null)
    }
  }

  if (status === "idle") return null

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
            : warnings.length > 0 ? "部分来源已返回"
            : "已启用的外部来源"}
        </span>
      </header>
      {status === "failed" && errorCode !== null && (
        <div className="search-results__notice" role="alert">
          <div className="search-results__notice-toggle">
            <span>来源搜索暂时不可用（{errorCode ?? "UNKNOWN"}），本地结果不受影响。</span>
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
              有 {warnings.length} 个来源未返回结果，已展示其余来源。
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
      {candidates.length === 0 && status === "done" && (
        <div className="search-results__empty search-results__empty--source" role="status">
          <SearchX size={24} aria-hidden="true" />
          <div>
            <p className="font-medium text-foreground">来源中暂无匹配结果</p>
            <p className="mt-1">已启用来源没有返回匹配条目，可尝试其他关键词。</p>
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
              <span className="search-result-card__source-icon">
                <Server size={17} aria-hidden="true" />
              </span>
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
