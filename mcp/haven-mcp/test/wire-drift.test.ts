// 契约漂移守卫：MCP 的枚举取值必须等于生成的 wire.ts。
//
// 为什么必须存在：这些枚举是**唯一的**契约镜像。一旦某一侧被"顺手"改动
// （例如把 `eyeCare` 写成 `eye_care`，或给 font_size 加一个后端不认的档位），
// 症状会是"Haven 拒绝整份提案，而模型以为是 Haven 的 bug"——排查成本极高。
// 直接读生成物比对，是唯一不依赖人工记性的做法。

import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { PREFERENCE_ENUM_BINDINGS } from "../src/schemas.js";

const PACKAGE_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const WIRE_PATH = path.resolve(
  PACKAGE_ROOT,
  "../../前端/app/src/lib/ipc/generated/wire.ts",
);

const WIRE_SOURCE = readFileSync(WIRE_PATH, "utf8");

/** 从生成的 wire.ts 里取出一个字符串联合类型的取值。 */
function unionValues(typeName: string): string[] {
  const pattern = new RegExp(`^export type ${typeName} = ([^;]+);$`, "m");
  const match = pattern.exec(WIRE_SOURCE);
  if (match?.[1] === undefined) {
    throw new Error(`wire.ts 里找不到 ${typeName}；生成物是否已重新生成？`);
  }
  return [...match[1].matchAll(/"([^"]+)"/g)].map((entry) => entry[1] as string);
}

describe("MCP 枚举 vs 生成 wire", () => {
  it("wire.ts 存在且包含偏好枚举（防止路径写错导致测试自我通过）", () => {
    expect(WIRE_SOURCE.length).toBeGreaterThan(10_000);
    expect(WIRE_SOURCE).toContain("PreferenceReadingPatchDto");
    expect(WIRE_SOURCE).toContain("PreferenceComicPatchDto");
  });

  it("每一个偏好枚举都与 wire.ts 的联合类型逐一相等（顺序也一致）", () => {
    const mismatches: string[] = [];
    for (const [typeName, values] of Object.entries(PREFERENCE_ENUM_BINDINGS)) {
      const fromWire = unionValues(typeName);
      const fromMcp = [...values];
      if (JSON.stringify(fromWire) !== JSON.stringify(fromMcp)) {
        mismatches.push(`${typeName}: wire=${JSON.stringify(fromWire)} mcp=${JSON.stringify(fromMcp)}`);
      }
    }
    expect(mismatches).toEqual([]);
  });

  it("wire 的 patch 字段名与 MCP 的 patch 键一一对应（camelCase ↔ snake_case）", () => {
    // MCP 面用 snake_case（对外协议），Haven Wire 用 camelCase（内部 IPC）。
    // 这条测试把两层映射钉死：多一个或少一个键都会失败。
    const readingKeys = patchKeys("PreferenceReadingPatchDto");
    const comicKeys = patchKeys("PreferenceComicPatchDto");
    // 默认 `.sort()` 按 UTF-16 码元序，大小写敏感（大写字母排在小写之前）。
    expect(readingKeys).toEqual([
      "contentWidth",
      "customBackground",
      "customFontFamily",
      "customText",
      "fontFamily",
      "fontSize",
      "fontWeight",
      "letterSpacing",
      "lineHeight",
      "pagination",
      "systemAuto",
      "theme",
    ]);
    expect(comicKeys).toEqual(["direction", "pageGap", "preloadPages", "viewMode"]);
  });
});

/** 取出 `export type X = { a?: ..., b?: ... };` 的字段名（去 `?`，排序）。 */
function patchKeys(typeName: string): string[] {
  const pattern = new RegExp(`^export type ${typeName} = \\{ ([^}]+) \\};$`, "m");
  const match = pattern.exec(WIRE_SOURCE);
  if (match?.[1] === undefined) throw new Error(`wire.ts 里找不到 ${typeName} 的字段`);
  return [...match[1].matchAll(/(\w+)\??:/g)]
    .map((entry) => entry[1] as string)
    .sort();
}
