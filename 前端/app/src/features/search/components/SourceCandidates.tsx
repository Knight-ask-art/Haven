// 来源候选区（V2-B 实战批次）：渐进式搜索的来源结果 + 一键导入。
// 六态遵循契约 §27：loading / success_empty / success_data / partial（warning 计数）
// / cancelled / error。本地结果不受来源失败影响。

import { useEffect, useRef, useState } from "react"
import { useNavigate } from "react-router"
import { ChevronDown, ChevronRight, Loader2, SearchX, Server } from "lucide-react"

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
          const mapped = event.data.works.map((work) => ({ index: runningIndex++, work }))
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
      .then(() => {
        // started 同步先发；promise 只确认登记成功。
        if (!cancelled && status === "idle") setStatus("searching")
      })
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

  if (status === "failed") {
    // 局部失败：不破坏本地结果，仅本区块呈现可重试错误。
    return (
      <section className="flex flex-col gap-4">
        <h2 className="text-xl font-bold tracking-tight">来源结果</h2>
        <div className="flex items-center justify-between rounded-3xl border border-black/[0.08] bg-white/70 px-5 py-4 text-sm">
          <span className="text-[#d97706]">来源搜索暂时不可用（{errorCode ?? "UNKNOWN"}）</span>
          <button type="button" onClick={() => { setStatus("idle") }} className="font-semibold text-[#007aff]">知道了</button>
        </div>
      </section>
    )
  }

  return (
    <section className="flex flex-col gap-4">
      <h2 className="text-xl font-bold tracking-tight">来源结果</h2>
      {status === "searching" && (
        <div className="flex items-center gap-3 rounded-3xl border border-black/[0.08] bg-white/70 px-5 py-4 text-sm text-[#86868b]">
          <Loader2 className="h-4 w-4 animate-spin" />
          正在向已启用来源分发查询…
        </div>
      )}
      {warnings.length > 0 && (
        <div className="rounded-3xl border border-black/[0.08] bg-white/70 px-5 py-4 text-sm">
          <button
            type="button"
            aria-expanded={warningsOpen}
            onClick={() => { setWarningsOpen((open) => !open) }}
            className="flex w-full items-center justify-between text-left"
          >
            <span className="text-xs text-[#d97706]">
              有 {warnings.length} 个来源未返回结果，已展示其余来源。
            </span>
            <span className="flex items-center gap-1 text-xs font-semibold text-[#007aff]">
              {warningsOpen ? "收起明细" : "查看明细"}
              {warningsOpen
                ? <ChevronDown className="h-4 w-4" />
                : <ChevronRight className="h-4 w-4" />}
            </span>
          </button>
          {warningsOpen && (
            <ul className="mt-3 flex flex-col gap-2 border-t border-black/[0.06] pt-3">
              {warnings.map((warning, i) => (
                <li key={`${warning.sourceId}-${i}`} className="text-xs text-[#86868b]">
                  <span className="font-semibold">{sourceDisplayName(warning.sourceId)}</span>
                  {" · "}
                  {warningLineText(warning)}
                  <span className="ml-2 rounded-full bg-black/[0.05] px-2 py-0.5 text-[10px] text-[#6e6e73]">
                    {warning.code}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
      {candidates.length === 0 && status === "done" ? (
        <div className="flex items-center gap-3 rounded-3xl border border-dashed border-black/[0.12] bg-white/60 px-5 py-4 text-sm text-[#86868b]">
          <SearchX className="h-4 w-4" />
          已启用来源没有返回匹配条目。
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
          {candidates.map(({ index, work }) => (
            <article
              key={`${work.workId}-${index}`}
              className="flex items-center gap-4 rounded-3xl border border-black/[0.08] bg-white/80 px-5 py-4"
            >
              <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-[#f2f2f4] text-[#6e6e73]">
                <Server className="h-5 w-5" />
              </span>
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-semibold">{work.title}</p>
                <p className="mt-1 truncate text-xs text-[#86868b]">
                  来源候选{work.releaseYear !== null ? ` · ${work.releaseYear}` : ""}
                  {work.availableMediaTypes.length > 0 ? ` · ${work.availableMediaTypes.join("/")}` : ""}
                </p>
              </div>
              {canImportCandidate(work.workId) ? (
                <button
                  type="button"
                  disabled={importingIndex !== null}
                  onClick={() => { void importCandidate({ index, work }) }}
                  className="shrink-0 rounded-full bg-[#007aff] px-[16px] py-[8px] text-xs font-semibold text-white disabled:opacity-50"
                >
                  {importingIndex === index ? "导入中…" : "导入媒体库"}
                </button>
              ) : (
                <span className="shrink-0 rounded-full bg-black/[0.05] px-3 py-2 text-[11px] font-medium text-[#86868b] dark:bg-white/[0.08] dark:text-[#a1a1a6]">
                  仅搜索
                </span>
              )}
            </article>
          ))}
        </div>
      )}
      {importError && <p className="text-sm font-semibold text-[#d97706]">{importError}</p>}
    </section>
  )
}
