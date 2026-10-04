// 源码层守卫。
//
// 这些是"结构性"约束，运行时测试覆盖不到，但一旦被破坏后果很严重：
// - stdout 是 stdio 传输的 JSON-RPC 通道，一句 console.log 就会破坏协议流；
// - MCP server 不得直接读 SQLite / 文件系统 / 凭据库，也不得 invoke Tauri。
//
// 与其靠评审记得，不如让它们在 CI 里失败。

import { readFileSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const PACKAGE_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SRC_DIR = path.join(PACKAGE_ROOT, "src");

function sourceFiles(dir: string): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = path.join(dir, entry);
    if (statSync(full).isDirectory()) {
      found.push(...sourceFiles(full));
    } else if (entry.endsWith(".ts")) {
      found.push(full);
    }
  }
  return found;
}

const FILES = sourceFiles(SRC_DIR).map((file) => ({
  relative: path.relative(PACKAGE_ROOT, file).replaceAll("\\", "/"),
  text: readFileSync(file, "utf8"),
}));

describe("stdout 洁净", () => {
  it("src/ 里没有 console.log / console.info / console.warn / process.stdout", () => {
    const offenders: string[] = [];
    for (const file of FILES) {
      file.text.split("\n").forEach((line, index) => {
        // 允许注释里提到这些词（本文件的说明就提到了）。
        const code = line.split("//")[0] ?? "";
        if (/console\.(log|info|warn|debug|trace)\s*\(/.test(code)) {
          offenders.push(`${file.relative}:${index + 1} console 输出`);
        }
        if (/process\.stdout/.test(code)) {
          offenders.push(`${file.relative}:${index + 1} process.stdout`);
        }
      });
    }
    expect(offenders).toEqual([]);
  });

  it("唯一允许写 stderr 的地方是 log.ts", () => {
    const writers = FILES.filter((file) => /process\.stderr/.test(file.text)).map((file) => file.relative);
    expect(writers).toEqual(["src/log.ts"]);
  });

  it("index.ts 通过 StdioServerTransport 接入，且不注入 fixture 工厂", () => {
    const index = FILES.find((file) => file.relative === "src/index.ts");
    expect(index).toBeTruthy();
    expect(index?.text).toContain("StdioServerTransport");
    // 生产入口不传 options，因此 fixture 模式在生产里必然被拒绝。
    expect(index?.text).toContain("selectBridge(process.env)");
    expect(index?.text).not.toContain("fixtureFactory");
  });
});

describe("桥接不得绕过 Typed Haven Agent API", () => {
  it("src/ 不引入文件系统 / 数据库 / 凭据库 / Tauri", () => {
    const forbiddenModules = [
      "node:fs",
      "node:fs/promises",
      "fs",
      "node:sqlite",
      "better-sqlite3",
      "sqlite3",
      "@tauri-apps/api",
      "node:child_process",
      "node:http",
      "node:https",
      "node:dgram",
      "node:tls",
      "keytar",
      "node-keytar",
    ];
    const offenders: string[] = [];
    for (const file of FILES) {
      for (const line of file.text.split("\n")) {
        const match = /^\s*(?:import|export)[^"']*from\s+["']([^"']+)["']/.exec(line);
        const bare = match?.[1];
        if (bare !== undefined && forbiddenModules.includes(bare)) {
          offenders.push(`${file.relative} -> ${bare}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  it("local bridge 只允许 outbound node:net，不得监听", () => {
    const bridge = FILES.find((file) => file.relative === "src/local-broker.ts");
    expect(bridge).toBeTruthy();
    expect(bridge?.text).toContain('from "node:net"');
    expect(bridge?.text).not.toMatch(/createServer\s*\(/);
    expect(bridge?.text).not.toMatch(/\.listen\s*\(/);
  });

  it("src/ 没有出现 SQL 语句或 Tauri invoke", () => {
    const offenders: string[] = [];
    for (const file of FILES) {
      for (const pattern of [/\binvoke\s*\(/, /\bSELECT\b[\s\S]{0,40}\bFROM\b/, /\bINSERT\s+INTO\b/]) {
        if (pattern.test(file.text)) offenders.push(`${file.relative} 命中 ${pattern}`);
      }
    }
    expect(offenders).toEqual([]);
  });

  it("HavenAgentBridge 端口没有 apply / approve / delete / write 方法", () => {
    const port = FILES.find((file) => file.relative === "src/bridge.ts");
    expect(port).toBeTruthy();
    // 先剥掉注释：端口文件的注释里**必须**能写"本端口没有 Apply/Approve"，
    // 否则那条说明本身会被当成违规。用 `.` 而不是换行否定类：JS 的 `.` 默认不匹配
    // 换行，正好逐行剥离行注释，也避免在本文件里写转义序列。
    const code = (port?.text ?? "")
      .replaceAll(/\/\*[\s\S]*?\*\//g, "")
      .replaceAll(/\/\/.*/g, "");

    // 只解析 `HavenAgentBridge` 这个接口**本身**的成员。整文件正则会把
    // 能力清单里的 `rename_proposal: boolean;` 和 Promise 执行器的 `reject(...)`
    // 这类无关文本也算进来（两者都不是端口方法）。
    const body = /^export interface HavenAgentBridge \{([\s\S]*?)^\}/m.exec(code)?.[1];
    expect(body, "找不到 HavenAgentBridge 接口体").toBeTruthy();

    const members = [...(body ?? "").matchAll(/^\s{2}([A-Za-z_]\w*)\s*[<(]/gm)].map(
      (match) => match[1] as string,
    );
    // 反向守卫：如果解析不到成员，下面的断言会因为"什么都没找到"而永远通过。
    expect(members.length, `解析到的端口成员：${members.join(", ")}`).toBeGreaterThanOrEqual(9);
    expect(members).toContain("proposeSettingsPatch");
    expect(members).toContain("proposeResourcePreferencePatch");
    expect(members).toContain("getSettingsSnapshot");

    const offenders = members.filter((name) =>
      /^(apply|approve|reject|delete|remove|write|rename|move|invoke|exec|run)/i.test(name),
    );
    expect(offenders).toEqual([]);
  });
});

describe("测试替身不进入生产模块图", () => {
  it("src/ 不引用 fixture 实现", () => {
    for (const file of FILES) {
      expect(file.text, `${file.relative} 不得引用 fixture 桥接`).not.toContain("FixtureHavenAgentBridge");
      expect(file.text, `${file.relative} 不得引用 Leaky`).not.toContain("LeakyHavenAgentBridge");
    }
  });

  it("fixture 实现只存在于 test/ 目录", () => {
    const testOnly = readFileSync(
      path.join(PACKAGE_ROOT, "test", "support", "fixture-bridge.ts"),
      "utf8",
    );
    expect(testOnly).toContain("class FixtureHavenAgentBridge");
  });
});

/**
 * `engines.node` 必须说得出**真实**的最低版本。
 *
 * 曾经写着 `>=20`，而锁定的 vitest 5 只支持 `^22.12.0 || ^24.0.0 || >=26.0.0`：
 * 照着 `package.json` 准备 Node 20 环境的人能装完，然后在 `npm test` 上撞墙——声明比
 * 工具链宽松，是"文档说可以、实际不行"的那一类缺陷，只能靠对着锁文件断言来防。
 */
describe("包声明的 Node 版本与锁定的工具链一致", () => {
  const packageJson = JSON.parse(
    readFileSync(path.join(PACKAGE_ROOT, "package.json"), "utf8"),
  ) as { engines?: { node?: string } };
  const lock = JSON.parse(
    readFileSync(path.join(PACKAGE_ROOT, "package-lock.json"), "utf8"),
  ) as { packages: Record<string, { engines?: { node?: string } } | undefined> };

  /**
   * 从一个 semver 范围里取出"允许的最低版本"。
   *
   * 这里出现的范围只有 `>=x.y.z` 与 `^x.y.z || …` 两种形态，两者的最低允许版本都是
   * 文本里第一个完整的 `x.y.z`，不需要引入 semver 依赖。
   */
  function lowestVersion(range: string): [number, number, number] {
    const match = /(\d+)\.(\d+)\.(\d+)/.exec(range);
    if (match === null) throw new Error(`无法从范围里读出最低版本：${range}`);
    return [Number(match[1]), Number(match[2]), Number(match[3])];
  }

  function asNumber([major, minor, patch]: [number, number, number]): number {
    return major * 1_000_000 + minor * 1_000 + patch;
  }

  it("声明的最低 Node 版本不低于锁定工具链的最低版本", () => {
    const declared = packageJson.engines?.node;
    expect(declared, "package.json 必须声明 engines.node").toBeTruthy();
    const locked = lock.packages["node_modules/vitest"]?.engines?.node;
    expect(locked, "锁文件里必须有 vitest 的 engines.node").toBeTruthy();
    expect(
      asNumber(lowestVersion(declared as string)),
      `engines.node=${declared} 比锁定的 vitest（${locked}）宽松`,
    ).toBeGreaterThanOrEqual(asNumber(lowestVersion(locked as string)));
  });

  it("锁文件的根条目与 package.json 的 engines 逐字一致", () => {
    // npm 会把根 package.json 的 engines 镜像进 lock 的 `""` 条目；两处不一致时下一次
    // `npm install` 会静默改写其中一处，于是声明与实现对不上。
    expect(lock.packages[""]?.engines?.node).toBe(packageJson.engines?.node);
  });
});
