// @vitest-environment jsdom
//
// 云盘浏览 Hook 的用例：一次性光标、重复提交抑制与"只使用不透明句柄"这三条不变量
// 都只在时序里成立，所以这里用可控 promise 安排顺序，而不是断言渲染文案。

import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type {
  CloudBrowseEntryDto,
  CloudBrowsePageDto,
  CloudObjectDto,
} from "@/lib/ipc/generated/wire"

const gateway = vi.hoisted(() => ({
  root: vi.fn(),
  folder: vi.fn(),
  next: vi.fn(),
  location: vi.fn(),
  register: vi.fn(),
  importPdf: vi.fn(),
}))

vi.mock("../ipc/cloud-storage-gateway", () => ({ cloudStorageGateway: gateway }))

import { useCloudBrowse } from "./useCloudBrowse"

const ACCOUNT_ID = "6a8e0d22-3333-4444-8555-666677778888"
const ROOT_HANDLE = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d"
const FOLDER_HANDLE = "1b2c3d4e-5f6a-4b7c-8d9e-0f1a2b3c4d5e"
const FILE_HANDLE = "2c3d4e5f-6a7b-4c8d-9e0f-1a2b3c4d5e6f"
const CURSOR = "3d4e5f6a-7b8c-4d9e-8f0a-1b2c3d4e5f6a"
const LOCATION_ID = "4e5f6a7b-8c9d-4e0f-9a1b-2c3d4e5f6a7b"
const REGISTERED_LOCATION = "5f6a7b8c-9d0e-4f1a-8b2c-3d4e5f6a7b8c"
const SECOND_ACCOUNT_ID = "9d0e1f2a-3b4c-4d5e-8f6a-7b8c9d0e1f2a"
const ROTATED_ROOT_HANDLE = "0e1f2a3b-4c5d-4e6f-8a7b-8c9d0e1f2a3b"

const FOLDER: CloudBrowseEntryDto = { handle: FOLDER_HANDLE, displayName: "收藏的漫画", isFolder: true, pdfSupported: false, sizeBytes: null }
const PDF: CloudBrowseEntryDto = { handle: FILE_HANDLE, displayName: "第一卷.pdf", isFolder: false, pdfSupported: true, sizeBytes: 2048 }
/** 没有句柄的条目：既不可进入也不可导入——不能拿 displayName 去猜一个身份。 */
const HANDLELESS: CloudBrowseEntryDto = { handle: null, displayName: "未授权目录", isFolder: true, pdfSupported: false, sizeBytes: null }

const ROOT_PAGE: CloudBrowsePageDto = { folderHandle: ROOT_HANDLE, entries: [FOLDER, PDF], nextCursor: CURSOR }
const ENTERED_PAGE: CloudBrowsePageDto = { folderHandle: FOLDER_HANDLE, entries: [PDF], nextCursor: null }
const LATEST_PAGE: CloudBrowsePageDto = { folderHandle: ROOT_HANDLE, entries: [FOLDER], nextCursor: null }
const STALE_PAGE: CloudBrowsePageDto = { folderHandle: FOLDER_HANDLE, entries: [PDF], nextCursor: CURSOR }
const REFRESHED_PAGE: CloudBrowsePageDto = { folderHandle: ROOT_HANDLE, entries: [PDF], nextCursor: null }
const NEXT_PAGE: CloudBrowsePageDto = { folderHandle: ROOT_HANDLE, entries: [PDF], nextCursor: null }
const ROTATED_ROOT_PAGE: CloudBrowsePageDto = { folderHandle: ROTATED_ROOT_HANDLE, entries: [FOLDER], nextCursor: null }
const IMPORTED: CloudObjectDto = {
  objectId: "6a7b8c9d-0e1f-4a2b-8c3d-4e5f6a7b8c9d",
  mediaItemId: "7b8c9d0e-1f2a-4b3c-8d4e-5f6a7b8c9d0e",
  resourceId: "8c9d0e1f-2a3b-4c4d-8e5f-6a7b8c9d0e1f",
  displayName: "第一卷.pdf",
  sizeBytes: 2048,
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((complete) => { resolve = complete })
  return { promise, resolve }
}

beforeEach(() => {
  for (const method of Object.values(gateway)) method.mockReset()
})

afterEach(() => { cleanup() })

describe("useCloudBrowse 的时序与句柄", () => {
  it("同目录翻页与根目录刷新保留名称草稿，句柄轮换不算导航", async () => {
    gateway.root.mockResolvedValueOnce(ROOT_PAGE).mockResolvedValueOnce(ROTATED_ROOT_PAGE)
    gateway.next.mockResolvedValueOnce(NEXT_PAGE)
    const { result } = renderHook(() => useCloudBrowse(ACCOUNT_ID, undefined, vi.fn(), vi.fn()))
    await waitFor(() => expect(result.current.page).toEqual(ROOT_PAGE))
    expect(result.current.displayName).toBe("我的云盘")
    act(() => result.current.setDisplayName("我的漫画收藏"))
    await act(async () => { await result.current.next() })
    expect(result.current.page).toEqual(NEXT_PAGE)
    expect(result.current.displayName).toBe("我的漫画收藏")
    await act(async () => { await result.current.reload() })
    expect(result.current.page).toEqual(ROTATED_ROOT_PAGE)
    expect(result.current.trail).toEqual([{ handle: ROTATED_ROOT_HANDLE, name: "我的云盘" }])
    expect(result.current.displayName).toBe("我的漫画收藏")
  })

  it("子目录刷新保留草稿，返回上级才重设默认名称", async () => {
    gateway.root.mockResolvedValue(ROOT_PAGE)
    gateway.folder.mockResolvedValue(ENTERED_PAGE)
    const { result } = renderHook(() => useCloudBrowse(ACCOUNT_ID, undefined, vi.fn(), vi.fn()))
    await waitFor(() => expect(result.current.page).toEqual(ROOT_PAGE))
    await act(async () => { await result.current.enter(FOLDER) })
    expect(result.current.displayName).toBe("收藏的漫画")
    act(() => result.current.setDisplayName("我的漫画收藏"))
    await act(async () => { await result.current.reload() })
    expect(result.current.displayName).toBe("我的漫画收藏")
    await act(async () => { await result.current.ascend(0) })
    expect(result.current.displayName).toBe("我的云盘")
  })

  it("初次根目录失败后的重试不覆盖草稿，子目录回到根目录是导航", async () => {
    gateway.root.mockRejectedValueOnce({ code: "INTERNAL_ERROR", userMessage: "读取云盘目录失败", retryable: true })
      .mockResolvedValue(ROTATED_ROOT_PAGE)
    gateway.folder.mockResolvedValue(ENTERED_PAGE)
    const { result } = renderHook(() => useCloudBrowse(ACCOUNT_ID, undefined, vi.fn(), vi.fn()))
    await waitFor(() => expect(result.current.error?.code).toBe("INTERNAL_ERROR"))
    act(() => result.current.setDisplayName("我的漫画收藏"))
    await act(async () => { await result.current.root() })
    expect(result.current.displayName).toBe("我的漫画收藏")
    await act(async () => { await result.current.enter(FOLDER) })
    act(() => result.current.setDisplayName("子目录名称"))
    await act(async () => { await result.current.root() })
    expect(result.current.displayName).toBe("我的云盘")
  })

  it("上一账户迟到的目录读回不覆盖当前目录或用户草稿", async () => {
    const first = deferred<CloudBrowsePageDto>()
    gateway.root.mockImplementation((accountId: string) => accountId === ACCOUNT_ID ? first.promise : Promise.resolve(ROTATED_ROOT_PAGE))
    const { result, rerender } = renderHook(
      ({ accountId }: { accountId: string }) => useCloudBrowse(accountId, undefined, vi.fn(), vi.fn()),
      { initialProps: { accountId: ACCOUNT_ID } },
    )
    rerender({ accountId: SECOND_ACCOUNT_ID })
    await waitFor(() => expect(result.current.page).toEqual(ROTATED_ROOT_PAGE))
    act(() => result.current.setDisplayName("我的漫画收藏"))
    await act(async () => { first.resolve(STALE_PAGE); await first.promise })
    expect(result.current.page).toEqual(ROTATED_ROOT_PAGE)
    expect(result.current.displayName).toBe("我的漫画收藏")
  })

  it("StrictMode 重放 effect 后，只有最后发出的那次根目录读取能落地", async () => {
    const first = deferred<CloudBrowsePageDto>()
    const second = deferred<CloudBrowsePageDto>()
    let issued = 0
    gateway.root.mockImplementation(() => (++issued === 1 ? first.promise : second.promise))
    // React 19 在根级 StrictMode 才重放初次 effect，与 main.tsx 的实际挂载条件一致。
    const { result } = renderHook(() => useCloudBrowse(ACCOUNT_ID, undefined, vi.fn(), vi.fn()), { reactStrictMode: true })

    expect(gateway.root, "StrictMode 下 effect 会重放，两次读取都必须真的发出").toHaveBeenCalledTimes(2)

    await act(async () => { second.resolve(LATEST_PAGE); await second.promise })
    expect(result.current.page).toEqual(LATEST_PAGE)

    await act(async () => { first.resolve(STALE_PAGE); await first.promise })
    expect(result.current.page, "更早发出、更晚回来的那一页不得覆盖当前目录").toEqual(LATEST_PAGE)
    expect(result.current.busy).toBe(false)
  })

  it("登记目录：双击只提交一次，句柄取自页面、名字取自用户", async () => {
    gateway.root.mockResolvedValue(ROOT_PAGE)
    gateway.folder.mockResolvedValue(ENTERED_PAGE)
    gateway.register.mockResolvedValue(REGISTERED_LOCATION)
    const onRegistered = vi.fn()
    const { result } = renderHook(() => useCloudBrowse(ACCOUNT_ID, undefined, onRegistered, vi.fn()))
    await waitFor(() => expect(result.current.page).toEqual(ROOT_PAGE))

    await act(async () => { await result.current.enter(FOLDER) })
    expect(gateway.folder, "进入目录只用页面上给的不透明句柄").toHaveBeenCalledWith(FOLDER_HANDLE)
    // 面包屑只把展示名带在句柄旁边，导航始终走句柄。
    expect(result.current.trail).toEqual([
      { handle: ROOT_HANDLE, name: "我的云盘" },
      { handle: FOLDER_HANDLE, name: "收藏的漫画" },
    ])

    await act(async () => { await result.current.enter(HANDLELESS) })
    expect(gateway.folder, "没有句柄的条目不得被当成可进入目录").toHaveBeenCalledTimes(1)

    act(() => result.current.setDisplayName("  我的媒体  "))
    await act(async () => { void result.current.register(); void result.current.register() })
    expect(gateway.register, "第二次点击必须撞上锁").toHaveBeenCalledTimes(1)
    expect(gateway.register).toHaveBeenCalledWith({ folderHandle: ENTERED_PAGE.folderHandle, displayName: "我的媒体" })
    expect(onRegistered).toHaveBeenCalledTimes(1)
    expect(onRegistered).toHaveBeenCalledWith(REGISTERED_LOCATION)
  })

  it("一次性光标失败后不重放，刷新目录重新取页", async () => {
    gateway.root.mockResolvedValueOnce(ROOT_PAGE).mockResolvedValueOnce(REFRESHED_PAGE)
    gateway.next.mockRejectedValueOnce({ code: "CLOUD_CURSOR_CONSUMED", userMessage: "这一页已经过期", retryable: true })
    const { result } = renderHook(() => useCloudBrowse(ACCOUNT_ID, undefined, vi.fn(), vi.fn()))
    await waitFor(() => expect(result.current.page).toEqual(ROOT_PAGE))

    await act(async () => { await result.current.next() })
    expect(gateway.next).toHaveBeenCalledTimes(1)
    expect(gateway.next).toHaveBeenCalledWith(CURSOR)
    expect(result.current.page?.nextCursor, "消费过的光标失败后必须清掉").toBeNull()
    expect(result.current.error?.code).toBe("CLOUD_CURSOR_CONSUMED")

    await act(async () => { await result.current.next() })
    expect(gateway.next, "没有光标时再点下一页不得重放旧光标").toHaveBeenCalledTimes(1)

    await act(async () => { await result.current.reload() })
    expect(gateway.root).toHaveBeenCalledTimes(2)
    expect(result.current.page).toEqual(REFRESHED_PAGE)
    expect(result.current.error).toBeNull()
  })

  it("导入 PDF：只有登记目录里带句柄且标记支持的条目会导入，重复点击只发一次", async () => {
    gateway.location.mockResolvedValue(ROOT_PAGE)
    gateway.importPdf.mockResolvedValue(IMPORTED)
    const onImported = vi.fn()
    const { result } = renderHook(() => useCloudBrowse(ACCOUNT_ID, LOCATION_ID, vi.fn(), onImported))
    await waitFor(() => expect(result.current.page).toEqual(ROOT_PAGE))
    expect(gateway.location).toHaveBeenCalledWith(LOCATION_ID)

    await act(async () => { await result.current.importPdf(HANDLELESS) })
    await act(async () => { await result.current.importPdf(FOLDER) })
    expect(gateway.importPdf, "没有句柄 / 不支持 PDF 的条目都不得导入").not.toHaveBeenCalled()

    await act(async () => { void result.current.importPdf(PDF); void result.current.importPdf(PDF) })
    expect(gateway.importPdf).toHaveBeenCalledTimes(1)
    expect(gateway.importPdf).toHaveBeenCalledWith({ locationId: LOCATION_ID, fileHandle: FILE_HANDLE })
    expect(onImported).toHaveBeenCalledTimes(1)
    expect(result.current.notice).toContain(IMPORTED.displayName)
  })
})
