// 云盘（Google Drive 只读切片）Typed IPC 的传输形状与响应守卫测试。
//
// 只断言纯逻辑：vi.fn() 冒充 transport，不读网络、不碰运行时单例、不依赖 React。
// 两条硬边界：
// - 命令名与参数必须逐字对上 src-tauri/src/commands/cloud_storage.rs（camelCase）；
// - 畸形响应（多一个字段、枚举越界、小数 / 越界数值、非 UUID 句柄、Provider 标识）
//   一律整体拒绝并抛 HavenError，绝不降级成"部分可用"。

import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  createCloudStorageIpc,
  guardCloudAccount,
  guardCloudBrowseEntry,
  guardCloudBrowsePage,
  guardCloudConnectAttempt,
  guardCloudConnectPoll,
  guardCloudConnectStatus,
  guardCloudFolder,
  guardCloudLocationId,
  guardCloudObject,
  guardCloudStorageList,
  type CloudStorageClient,
} from "./cloud-storage-client"
import { HavenError } from "./errors"

const ACCOUNT_ID = "11111111-1111-4111-8111-111111111111"
const LOCATION_ID = "22222222-2222-4222-8222-222222222222"
const MEDIA_ITEM_ID = "33333333-3333-4333-8333-333333333333"
const RESOURCE_ID = "44444444-4444-4444-8444-444444444444"
const OBJECT_ID = "55555555-5555-4555-8555-555555555555"
const HANDLE = "66666666-6666-4666-8666-666666666666"
const ATTEMPT_ID = "77777777-7777-4777-8777-777777777777"

function account(overrides: Record<string, unknown> = {}) {
  return { id: ACCOUNT_ID, displayName: "我的云盘", connected: true, generation: 3, ...overrides }
}

function entry(overrides: Record<string, unknown> = {}) {
  return {
    handle: HANDLE,
    displayName: "书.pdf",
    isFolder: false,
    pdfSupported: true,
    sizeBytes: 2048,
    ...overrides,
  }
}

function page(overrides: Record<string, unknown> = {}) {
  return { folderHandle: HANDLE, entries: [entry()], nextCursor: null, ...overrides }
}

function folder(overrides: Record<string, unknown> = {}) {
  return { locationId: LOCATION_ID, accountId: ACCOUNT_ID, createdAtMs: 1_700_000_000_000, ...overrides }
}

function cloudObject(overrides: Record<string, unknown> = {}) {
  return {
    objectId: OBJECT_ID,
    mediaItemId: MEDIA_ITEM_ID,
    resourceId: RESOURCE_ID,
    displayName: "书.pdf",
    sizeBytes: 2048,
    ...overrides,
  }
}

describe("云盘 Typed IPC：命令名与 camelCase 参数", () => {
  it("14 条命令逐条对上 Rust 命令签名", async () => {
    const cases: Array<{
      run: (client: CloudStorageClient) => Promise<unknown>
      response: unknown
      command: string
      args: Record<string, unknown> | undefined
    }> = [
      {
        run: (client) => client.cloudStorageList(),
        response: { oauthAvailable: false, accounts: [] },
        command: "cloud_storage_list",
        args: undefined,
      },
      {
        run: (client) => client.cloudAccountConnectBegin({}),
        response: { attemptId: ATTEMPT_ID, expiresAtMs: 1_700_000_000_000 },
        command: "cloud_account_connect_begin",
        args: { request: { accountId: null } },
      },
      {
        run: (client) => client.cloudAccountConnectBegin({ accountId: ACCOUNT_ID }),
        response: { attemptId: ATTEMPT_ID, expiresAtMs: 1_700_000_000_000 },
        command: "cloud_account_connect_begin",
        args: { request: { accountId: ACCOUNT_ID } },
      },
      {
        run: (client) => client.cloudAccountConnectPoll(ATTEMPT_ID),
        response: { status: "pending", expiresAtMs: 1_700_000_000_000 },
        command: "cloud_account_connect_poll",
        args: { attemptId: ATTEMPT_ID },
      },
      {
        run: (client) => client.cloudAccountConnectComplete(ATTEMPT_ID),
        response: account(),
        command: "cloud_account_connect_complete",
        args: { attemptId: ATTEMPT_ID },
      },
      {
        run: (client) => client.cloudAccountConnectCancel(ATTEMPT_ID),
        response: "cancelled",
        command: "cloud_account_connect_cancel",
        args: { attemptId: ATTEMPT_ID },
      },
      {
        run: (client) =>
          client.cloudAccountDisconnect({ accountId: ACCOUNT_ID, expectedGeneration: 3 }),
        response: account({ connected: false, generation: 4 }),
        command: "cloud_account_disconnect",
        args: { request: { accountId: ACCOUNT_ID, expectedGeneration: 3 } },
      },
      {
        run: (client) => client.cloudFolderBindingGet(LOCATION_ID),
        response: folder(),
        command: "cloud_folder_binding_get",
        args: { locationId: LOCATION_ID },
      },
      {
        run: (client) => client.cloudFolderBindingGet(LOCATION_ID),
        response: null,
        command: "cloud_folder_binding_get",
        args: { locationId: LOCATION_ID },
      },
      {
        run: (client) => client.cloudFolderRemove(LOCATION_ID),
        response: true,
        command: "cloud_folder_remove",
        args: { locationId: LOCATION_ID },
      },
      {
        run: (client) => client.cloudBrowseRoot(ACCOUNT_ID),
        response: page(),
        command: "cloud_browse_root",
        args: { accountId: ACCOUNT_ID },
      },
      {
        run: (client) => client.cloudBrowseFolder(HANDLE),
        response: page(),
        command: "cloud_browse_folder",
        args: { folderHandle: HANDLE },
      },
      {
        run: (client) => client.cloudBrowseNextPage(HANDLE),
        response: page({ nextCursor: null }),
        command: "cloud_browse_next_page",
        args: { cursor: HANDLE },
      },
      {
        run: (client) => client.cloudBrowseLocation(LOCATION_ID),
        response: page(),
        command: "cloud_browse_location",
        args: { locationId: LOCATION_ID },
      },
      {
        run: (client) =>
          client.cloudRegisterFolder({ folderHandle: HANDLE, displayName: "书库" }),
        response: LOCATION_ID,
        command: "cloud_register_folder",
        args: { request: { folderHandle: HANDLE, displayName: "书库" } },
      },
      {
        run: (client) => client.cloudImportPdf({ locationId: LOCATION_ID, fileHandle: HANDLE }),
        response: cloudObject(),
        command: "cloud_import_pdf",
        args: { request: { locationId: LOCATION_ID, fileHandle: HANDLE } },
      },
    ]

    for (const item of cases) {
      const transport = vi.fn().mockResolvedValue(item.response)
      const client = createCloudStorageIpc(transport)
      await item.run(client)
      expect(transport, item.command).toHaveBeenCalledWith(item.command, item.args)
    }
  })

  it("把守卫通过后的对象原样返回，不做任何加工", async () => {
    const projection = { oauthAvailable: false, accounts: [] }
    const client = createCloudStorageIpc(vi.fn().mockResolvedValue(projection))
    await expect(client.cloudStorageList()).resolves.toBe(projection)
  })
})

describe("云盘响应守卫：字段集合与闭合枚举", () => {
  it("接受后端真实形状", () => {
    expect(guardCloudStorageList({ oauthAvailable: true, accounts: [account()] })).toBe(true)
    expect(guardCloudStorageList({ oauthAvailable: false, accounts: [] })).toBe(true)
    expect(guardCloudConnectStatus("completing")).toBe(true)
    expect(guardCloudConnectPoll({ status: "expired", expiresAtMs: 1 })).toBe(true)
  })

  it("拒绝未知键与 symbol / 不可枚举键，而不是忽略它们", () => {
    for (const extra of ["driveId", "fileId", "pageToken", "email", "accessToken", "credentialRef"]) {
      expect(guardCloudAccount(account({ [extra]: "任意值" })), extra).toBe(false)
      expect(guardCloudStorageList({ oauthAvailable: false, accounts: [], [extra]: "x" }), extra).toBe(false)
      expect(guardCloudObject(cloudObject({ [extra]: "x" })), extra).toBe(false)
    }
    const withSymbol = account() as Record<string | symbol, unknown>
    withSymbol[Symbol("fileId")] = "drive-file-id"
    expect(guardCloudAccount(withSymbol)).toBe(false)
    const hidden = account()
    Object.defineProperty(hidden, "driveId", { value: "drive-file-id", enumerable: false })
    expect(guardCloudAccount(hidden)).toBe(false)
  })

  it("枚举是闭合集合：近似词、大小写变体与未知值都不接受", () => {
    for (const status of ["Authorized", "authorized ", "unknown", "", "pending_oauth"]) {
      expect(guardCloudConnectStatus(status), status).toBe(false)
    }
    expect(guardCloudConnectPoll({ status: "unknown", expiresAtMs: 1 })).toBe(false)
    expect(guardCloudConnectPoll({ status: "pending" })).toBe(false)
  })
})

describe("云盘响应守卫：UUID、句柄与数值边界", () => {
  it("内部 UUID 必须是规范小写形式，裸句柄不能是 URL / 路径", () => {
    expect(guardCloudAccount(account({ id: "abcdefab-cdef-4abc-8abc-abcdefabcdef".toUpperCase() }))).toBe(false)
    expect(guardCloudAccount(account({ id: "drive-account-1" }))).toBe(false)
    expect(guardCloudLocationId(LOCATION_ID)).toBe(true)
    expect(guardCloudLocationId("https://drive.google.com/drive/folders/abc")).toBe(false)
    expect(guardCloudLocationId("1A2b3C4d5E")).toBe(false)
    expect(guardCloudFolder(folder({ createdAtMs: 0 }))).toBe(false)
  })

  it("数值必须是安全整数并落在合理边界内", () => {
    expect(guardCloudAccount(account({ generation: 1.5 }))).toBe(false)
    expect(guardCloudAccount(account({ generation: -1 }))).toBe(false)
    expect(guardCloudAccount(account({ generation: Number.MAX_SAFE_INTEGER + 1 }))).toBe(false)
    expect(guardCloudAccount(account({ generation: Number.NaN }))).toBe(false)
    expect(guardCloudConnectAttempt({ attemptId: ATTEMPT_ID, expiresAtMs: 0 })).toBe(false)
    expect(guardCloudConnectAttempt({ attemptId: ATTEMPT_ID, expiresAtMs: 1.5 })).toBe(false)
    expect(guardCloudObject(cloudObject({ sizeBytes: -1 }))).toBe(false)
    expect(guardCloudObject(cloudObject({ sizeBytes: 2 ** 53 }))).toBe(false)
    expect(guardCloudBrowseEntry(entry({ sizeBytes: null, handle: null, pdfSupported: false }))).toBe(true)
    expect(guardCloudBrowseEntry(entry({ sizeBytes: 1.5 }))).toBe(false)
  })

  it("浏览条目的跨字段事实不能被拼错", () => {
    // 文件夹永远不支持 PDF。
    expect(guardCloudBrowseEntry(entry({ isFolder: true, pdfSupported: true }))).toBe(false)
    // 有句柄却既不可是文件夹也不可导入 → 自相矛盾。
    expect(guardCloudBrowseEntry(entry({ pdfSupported: false }))).toBe(false)
    // 不可选条目（handle null）是合法事实：已回收站 / 不支持 / 大小越界。
    expect(guardCloudBrowseEntry(entry({ handle: null, pdfSupported: false }))).toBe(true)
    expect(guardCloudBrowseEntry(entry({ handle: null, isFolder: true, pdfSupported: false }))).toBe(true)
    // 不可选条目不得声称支持 PDF。
    expect(guardCloudBrowseEntry(entry({ handle: null, pdfSupported: true }))).toBe(false)
  })

  it("分页响应拒绝非法光标与重复句柄", () => {
    expect(guardCloudBrowsePage(page({ nextCursor: "cursor-1" }))).toBe(false)
    expect(guardCloudBrowsePage(page({ folderHandle: null }))).toBe(true)
    expect(guardCloudBrowsePage(page({ entries: [entry(), entry()] }))).toBe(false)
    expect(guardCloudBrowsePage(page({ entries: [] }))).toBe(true)
  })

  it("账户列表里重复的账户 ID 会被拒绝", () => {
    expect(guardCloudStorageList({ oauthAvailable: true, accounts: [account(), account()] })).toBe(false)
  })
})

describe("云盘 Typed IPC：错误一律归一为 HavenError", () => {
  it("契约 ErrorDto 被原样保留（code / retryable 不丢）", async () => {
    const transport = vi.fn().mockRejectedValue({
      code: "CLOUD_ACCOUNT_MISSING",
      userMessage: "账户不存在",
      retryable: false,
    })
    const error = await createCloudStorageIpc(transport)
      .cloudStorageList()
      .catch((value: unknown) => value)
    expect(error).toBeInstanceOf(HavenError)
    expect((error as HavenError).code).toBe("CLOUD_ACCOUNT_MISSING")
    expect((error as HavenError).retryable).toBe(false)
  })

  it("非契约拒绝值兜底 INTERNAL_ERROR", async () => {
    const transport = vi.fn().mockRejectedValue(new Error("boom"))
    const error = await createCloudStorageIpc(transport)
      .cloudBrowseRoot(ACCOUNT_ID)
      .catch((value: unknown) => value)
    expect(error).toBeInstanceOf(HavenError)
    expect((error as HavenError).code).toBe("INTERNAL_ERROR")
  })

  it("守卫失败抛 INTERNAL_ERROR，不透传畸形载荷", async () => {
    const transport = vi.fn().mockResolvedValue({
      oauthAvailable: true,
      accounts: [account({ driveId: "drive-account-1" })],
    })
    const error = await createCloudStorageIpc(transport)
      .cloudStorageList()
      .catch((value: unknown) => value)
    expect(error).toBeInstanceOf(HavenError)
    expect((error as HavenError).code).toBe("INTERNAL_ERROR")
  })
})

const { getHavenClient } = vi.hoisted(() => ({ getHavenClient: vi.fn() }))
vi.mock("@/lib/ipc/runtime", () => ({ getHavenClient }))

import * as cloudStorageGateway from "@/features/settings/ipc/cloud-storage-gateway"
import {
  CLOUD_STORAGE_SLICE_CAPABILITIES,
  fetchCloudStorageList,
  importCloudPdf,
  registerCloudFolder,
} from "@/features/settings/ipc/cloud-storage-gateway"

beforeEach(() => {
  getHavenClient.mockReset()
})

describe("云盘 Gateway：只经 getHavenClient", () => {
  it("成功响应原样投影", async () => {
    const projection = { oauthAvailable: false, accounts: [] }
    getHavenClient.mockReturnValue({ cloudStorageList: async () => projection })
    await expect(fetchCloudStorageList()).resolves.toBe(projection)
  })

  it("畸形响应（多一个 Provider 标识字段）整体拒绝", async () => {
    getHavenClient.mockReturnValue({
      cloudStorageList: async () => ({ oauthAvailable: true, accounts: [], driveId: "drive-1" }),
    })
    await expect(fetchCloudStorageList()).rejects.toBeInstanceOf(HavenError)
  })

  it("登记目录只接受裸 UUID：URL / 句柄一律视为非法响应", async () => {
    getHavenClient.mockReturnValue({
      cloudRegisterFolder: async () => "https://drive.google.com/drive/folders/1A2b3C",
    })
    await expect(
      registerCloudFolder({ folderHandle: HANDLE, displayName: "书库" }),
    ).rejects.toBeInstanceOf(HavenError)
  })

  it("导入响应里出现真实文件 ID 就拒绝", async () => {
    getHavenClient.mockReturnValue({
      cloudImportPdf: async () => ({ ...cloudObject(), fileId: "1A2b3C4d5E" }),
    })
    await expect(
      importCloudPdf({ locationId: LOCATION_ID, fileHandle: HANDLE }),
    ).rejects.toBeInstanceOf(HavenError)
  })

  it("客户端抛出的 HavenError 原样保留", async () => {
    getHavenClient.mockReturnValue({
      cloudStorageList: async () => {
        throw new HavenError({ code: "CLOUD_OAUTH_UNAVAILABLE", userMessage: "未配置授权", retryable: false })
      },
    })
    const error = await fetchCloudStorageList().catch((value: unknown) => value)
    expect(error).toBeInstanceOf(HavenError)
    expect((error as HavenError).code).toBe("CLOUD_OAUTH_UNAVAILABLE")
  })

  it("能力面只有浏览与导入：没有上传 / 同步 / 刷新入口", () => {
    expect(CLOUD_STORAGE_SLICE_CAPABILITIES).toEqual({
      browse: true,
      importPdf: true,
      upload: false,
      bidirectionalSync: false,
      backgroundRefresh: false,
    })
    const exported = Object.keys(cloudStorageGateway)
    expect(exported.some((name) => /upload|sync|refresh|delete/i.test(name))).toBe(false)
  })
})
