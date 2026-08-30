import { describe, expect, it } from "vitest"
import listNormal from "../../../../../../contracts/ipc/v1/fixtures/storage/list.normal.json"
import listEmpty from "../../../../../../contracts/ipc/v1/fixtures/storage/list.empty.json"
import cancelAccepted from "../../../../../../contracts/ipc/v1/fixtures/scan/cancel.accepted.json"
import cancelTerminal from "../../../../../../contracts/ipc/v1/fixtures/scan/cancel.terminal.json"
import {
  guardScanCancelResult,
  guardStorageLocationList,
} from "./storage-gateway"

describe("storage wire runtime guards", () => {
  it("accepts the shared list fixtures", () => {
    expect(guardStorageLocationList(listNormal)).toBe(true)
    expect(guardStorageLocationList(listEmpty)).toBe(true)
  })

  it("rejects unknown provider and status values", () => {
    expect(guardStorageLocationList([{
      ...listNormal[0],
      providerType: "future_provider",
    }])).toBe(false)
    expect(guardStorageLocationList([{
      ...listNormal[0],
      status: "future_status",
    }])).toBe(false)
  })

  it("rejects internal paths and credential references", () => {
    for (const [field, value] of Object.entries({
      rootPath: "D:\\Private\\Media",
      rootRef: "D:\\Private\\Media",
      root_ref: "D:\\Private\\Media",
      credentialRef: "haven:local:private",
      credential_ref: "haven:local:private",
    })) {
      expect(guardStorageLocationList([{
        ...listNormal[0],
        [field]: value,
      }])).toBe(false)
    }
  })

  it("accepts the shared cancel fixtures", () => {
    expect(guardScanCancelResult(cancelAccepted)).toBe(true)
    expect(guardScanCancelResult(cancelTerminal)).toBe(true)
  })

  it("rejects a non-terminal phase and an invalid accepted result", () => {
    expect(guardScanCancelResult({
      taskId: "scan-task-003",
      alreadyTerminal: true,
      phase: "indexing",
    })).toBe(false)
    expect(guardScanCancelResult({
      taskId: "scan-task-004",
      alreadyTerminal: false,
      phase: "completed",
    })).toBe(false)
  })
})
