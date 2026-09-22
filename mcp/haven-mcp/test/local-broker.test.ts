import { EventEmitter } from "node:events";

import { describe, expect, it } from "vitest";

import type { SettingsProposalRequest } from "../src/bridge.js";
import {
  LiveHavenAgentBridge,
  validateHavenEndpoint,
} from "../src/local-broker.js";

// 测试 transport，不是生产模块的一部分。生产源码仍只能使用 outbound node:net。
class FakeSocket extends EventEmitter {
  destroyed = false;
  timeout = 0;
  readonly writes: Record<string, unknown>[] = [];
  // 每个响应帧最多 emit 这么多字节，用来覆盖长度前缀被拆包的情况。
  chunkSize: number | null = null;
  // 发出前改写响应帧，用来构造 Broker 侧的异常响应。
  transform:
    | ((response: Record<string, unknown>, request: Record<string, unknown>) => Record<string, unknown>)
    | null = null;

  write(buffer: Buffer, callback?: () => void): boolean {
    const length = buffer.readUInt32BE(0);
    const frame = JSON.parse(buffer.subarray(4, length + 4).toString("utf8")) as Record<string, unknown>;
    this.writes.push(frame);
    queueMicrotask(() => {
      const type = frame.type;
      if (type === "hello") {
        this.emitData(frame, {
          type: "welcome",
          protocol_version: 1,
          session_id: "00000000-0000-0000-0000-000000000001",
          haven: {
            app_version: "0.1.0-beta.1",
            agent_api_version: 1,
            capabilities: {
              settings_read: true,
              settings_proposal: true,
              library_summary_read: true,
              setting_sources_read: true,
              resource_preference_read: true,
              resource_preference_proposal: true,
              media_capabilities_read: true,
              onboarding_read: true,
              metadata_proposal: false,
              rename_proposal: false,
              secret_read: false,
              filesystem_write: false,
            },
          },
          granted_requests: [
            "context",
            "setting_sources",
            "resource_preference",
            "library_summary",
            "media_capabilities",
            "onboarding",
            "create_proposal",
            "create_resource_proposal",
          ],
        });
      } else if (type === "context") {
        this.emitData(frame, {
          type: "ok",
          id: frame.id,
          payload: {
            schemaVersion: 1,
            contextId: "00000000-0000-0000-0000-000000000002",
            contextHash: "a".repeat(64),
            subject: { section: "reading" },
            revision: "rev-7",
            reading: {
              section: "reading",
              fontFamily: "serif",
              customFontFamily: null,
              fontSize: "medium",
              lineHeight: "comfortable",
              contentWidth: "medium",
              theme: "system",
              customBackground: null,
              customText: null,
              fontWeight: "regular",
              letterSpacing: "normal",
              systemAuto: true,
              pagination: "scroll",
              redactedFields: [],
            },
            capabilities: {
              agentApiVersion: 1,
              capabilities: {
                settingsRead: true,
                settingsProposal: true,
                librarySummaryRead: true,
                settingSourcesRead: true,
                resourcePreferenceRead: true,
                resourcePreferenceProposal: true,
                mediaCapabilitiesRead: true,
                onboardingRead: true,
                metadataProposal: false,
                renameProposal: false,
                secretRead: false,
                filesystemWrite: false,
              },
            },
          },
        });
      } else if (type === "create_proposal") {
        this.emitData(frame, {
          type: "ok",
          id: frame.id,
          payload: {
            schemaVersion: 1,
            proposalId: "00000000-0000-0000-0000-000000000003",
            status: "pending",
            subject: { section: "reading" },
            targetLabel: "全局默认 · 阅读",
            baseRevision: "rev-7",
            digest: "b".repeat(64),
            createdAt: "2026-09-18T00:00:00.000Z",
            expiresAt: "2026-09-19T00:00:00.000Z",
            changes: [{ key: "reading.fontSize", before: "medium", after: "large" }],
          },
        });
      }
      callback?.();
    });
    return true;
  }

  setTimeout(timeout: number): this {
    this.timeout = timeout;
    return this;
  }

  destroy(): this {
    this.destroyed = true;
    return this;
  }

  connect(): void {
    queueMicrotask(() => this.emit("connect"));
  }

  private emitData(request: Record<string, unknown>, value: Record<string, unknown>): void {
    const response = this.transform === null ? value : this.transform(value, request);
    const payload = Buffer.from(JSON.stringify(response), "utf8");
    const frame = Buffer.alloc(4 + payload.length);
    frame.writeUInt32BE(payload.length, 0);
    payload.copy(frame, 4);
    if (this.chunkSize !== null && this.chunkSize > 0 && this.chunkSize < frame.length) {
      this.emit("data", frame.subarray(0, this.chunkSize));
      this.emit("data", frame.subarray(this.chunkSize));
      return;
    }
    this.emit("data", frame);
  }
}

const WINDOWS_ENDPOINT = "\\\\.\\pipe\\haven-agent-v1-0123abcd";

describe("Haven local Broker bridge", () => {
  it("只接受闭合的当前平台端点形态", () => {
    expect(
      validateHavenEndpoint(WINDOWS_ENDPOINT, {
        platform: "win32",
        env: {},
      }),
    ).toEqual({ ok: true });
    expect(
      validateHavenEndpoint("tcp://127.0.0.1:9999", {
        platform: "win32",
        env: {},
      }),
    ).toEqual({ ok: false, reason: "invalid" });
    expect(
      validateHavenEndpoint("/run/user/1000/haven/agent-v1.sock", {
        platform: "linux",
        env: { XDG_RUNTIME_DIR: "/run/user/1000", HOME: "/home/test" },
      }),
    ).toEqual({ ok: true });
  });

  it("完成 hello/welcome 与 context，并把 Wire camelCase 投影为 MCP snake_case", async () => {
    const sockets: FakeSocket[] = [];
    const bridge = new LiveHavenAgentBridge(WINDOWS_ENDPOINT, {
      platform: "win32",
      env: {},
      connect: () => {
        const socket = new FakeSocket();
        sockets.push(socket);
        socket.connect();
        return socket as never;
      },
    });

    const snapshot = await bridge.getSettingsSnapshot();
    expect(snapshot).toMatchObject({
      subject_scope: "global",
      section: "reading",
      context_id: "00000000-0000-0000-0000-000000000002",
      context_hash: "a".repeat(64),
      revision: "rev-7",
      settings: { font_family: "serif", font_size: "medium", system_auto: true },
    });
    expect(sockets[0]?.writes).toHaveLength(2);
    expect(sockets[0]?.writes[0]).toMatchObject({ type: "hello", protocol_version: 1 });
    expect(sockets[0]?.writes[1]).toMatchObject({ type: "context", id: 1, payload: {} });
  });

  it("只创建 pending Proposal，并把 patch 保持为 Broker 要求的 snake_case", async () => {
    let socket: FakeSocket | undefined;
    const bridge = new LiveHavenAgentBridge(WINDOWS_ENDPOINT, {
      platform: "win32",
      env: {},
      connect: () => {
        socket = new FakeSocket();
        socket.connect();
        return socket as never;
      },
    });
    const request: SettingsProposalRequest = {
      section: "reading",
      context_id: "00000000-0000-0000-0000-000000000002",
      context_hash: "a".repeat(64),
      base_revision: "rev-7",
      patch: { font_size: "large" },
    };

    const proposal = await bridge.proposeSettingsPatch(request);
    expect(proposal).toMatchObject({
      status: "pending",
      digest: "b".repeat(64),
      subject_scope: "global",
      section: "reading",
    });
    expect(socket?.writes[1]).toMatchObject({
      type: "create_proposal",
      payload: { section: "reading", patch: { font_size: "large" } },
    });
    expect(JSON.stringify(socket?.writes[1])).not.toContain("session_id");
    expect(JSON.stringify(socket?.writes[1])).not.toContain("request_id");
  });

  it("把被拆进长度前缀内部的响应帧重新组装", async () => {
    const bridge = new LiveHavenAgentBridge(WINDOWS_ENDPOINT, {
      platform: "win32",
      env: {},
      connect: () => {
        const socket = new FakeSocket();
        socket.chunkSize = 3;
        socket.connect();
        return socket as never;
      },
    });

    const snapshot = await bridge.getSettingsSnapshot();
    expect(snapshot).toMatchObject({ context_id: "00000000-0000-0000-0000-000000000002" });
  });

  it("拒绝 Broker 返回的越界 redactedFields 条目", async () => {
    const bridge = new LiveHavenAgentBridge(WINDOWS_ENDPOINT, {
      platform: "win32",
      env: {},
      connect: () => {
        const socket = new FakeSocket();
        socket.transform = (response, request) => {
          if (request.type === "context") {
            const payload = response.payload as { reading: { redactedFields: string[] } };
            payload.reading.redactedFields = ["x".repeat(200)];
          }
          return response;
        };
        socket.connect();
        return socket as never;
      },
    });

    await expect(bridge.getSettingsSnapshot()).rejects.toThrow("不符合协议");
  });

  it("拒绝 id 与请求不匹配的 error 帧", async () => {
    const bridge = new LiveHavenAgentBridge(WINDOWS_ENDPOINT, {
      platform: "win32",
      env: {},
      connect: () => {
        const socket = new FakeSocket();
        socket.transform = (response, request) => {
          if (request.type !== "context") return response;
          return { type: "error", id: 99, error: { code: "stale_request", message: "来自旧请求", retryable: false } };
        };
        socket.connect();
        return socket as never;
      },
    });

    await expect(bridge.getSettingsSnapshot()).rejects.toThrow("不符合协议");
  });
});
