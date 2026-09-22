// 脱敏层测试：内容层模式、结构层 denylist、限长与深度上界。

import { describe, expect, it } from "vitest";

import {
  boundText,
  containsForbiddenMaterial,
  redactText,
  redactValue,
} from "../src/redact.js";
import { MAX_TEXT_FIELD_CHARS } from "../src/constants.js";

describe("redactText", () => {
  it("抹掉凭据 target", () => {
    expect(redactText("target=haven:ai:gw-main 已配置")).not.toContain("haven:ai:gw-main");
    expect(redactText("haven:webdav:nas-1")).toContain("[REDACTED_CREDENTIAL_TARGET]");
  });

  it("抹掉 Windows / UNC / POSIX 绝对路径", () => {
    for (const value of [
      "C:\\Users\\someone\\Documents\\haven.db",
      "\\\\server\\share\\media\\movie.mkv",
      "/home/someone/.config/haven/haven.db",
      "/Users/someone/Library/Application Support/haven",
    ]) {
      const redacted = redactText(value);
      expect(redacted, value).toContain("[REDACTED_PATH]");
      expect(redacted).not.toContain("someone");
    }
  });

  it("抹掉 Bearer/Basic 头与常见密钥形状", () => {
    expect(redactText("Authorization: Bearer abcdefghijklmnop")).not.toContain("abcdefghijklmnop");
    expect(redactText("sk-abcdefghijklmnop")).toContain("[REDACTED_KEY]");
    expect(redactText("ghp_abcdefghijklmnop")).toContain("[REDACTED_KEY]");
  });

  it("**不**打码 canonical digest（提案 digest 必须原样返回）", () => {
    const digest = "a1b2c3d4".repeat(8);
    expect(digest).toHaveLength(64);
    expect(redactText(digest)).toBe(digest);
  });
});

describe("boundText", () => {
  it("按码点截断并显式标注被省略的字符数", () => {
    const long = "模".repeat(MAX_TEXT_FIELD_CHARS + 5);
    const bounded = boundText(long);
    expect(bounded).toContain("已截断 5 字符");
    expect(Array.from(bounded).length).toBeLessThan(long.length + 20);
  });

  it("不切断代理对（emoji 不会被截成半个字符）", () => {
    const emoji = "🎬".repeat(10);
    const bounded = boundText(emoji, 3);
    expect(bounded.startsWith("🎬🎬🎬")).toBe(true);
    expect(bounded).not.toContain("\uFFFD");
  });

  it("短文本原样返回", () => {
    expect(boundText("短文本")).toBe("短文本");
  });
});

describe("redactValue", () => {
  it("丢弃 denylist 键（而不是保留键名并写 [REDACTED]）", () => {
    const value = redactValue({
      display_name: "网关",
      secret: "sk-not-real",
      credential_ref: "haven:ai:gw",
      apiKey: "x",
      rootPath: "C:\\x",
    }) as Record<string, unknown>;
    expect(Object.keys(value)).toEqual(["display_name"]);
    expect(value.display_name).toBe("网关");
  });

  it("保留 target 键（它在资源偏好里表示作用域，不是凭据 target）", () => {
    const value = redactValue({ target: { scope: "edition", edition_id: "abc" } }) as Record<string, unknown>;
    expect(value.target).toEqual({ scope: "edition", edition_id: "abc" });
  });

  it("对嵌套结构与数组同样生效", () => {
    const value = redactValue({
      items: [{ title: "正常", path: "C:\\secret\\x" }, { title: "haven:ai:gw" }],
    });
    expect(JSON.stringify(value)).not.toContain("C:\\secret");
    expect(JSON.stringify(value)).not.toContain("haven:ai:gw");
  });

  it("丢弃非 JSON 值（函数 / symbol / bigint / 非有限数）", () => {
    const value = redactValue({
      ok: 1,
      fn: () => 1,
      sym: Symbol("x"),
      big: BigInt(1),
      nan: Number.NaN,
      inf: Number.POSITIVE_INFINITY,
    }) as Record<string, unknown>;
    // 键按字典序输出（redactValue 会排序），非 JSON 值的键整体消失。
    expect(Object.keys(value)).toEqual(["inf", "nan", "ok"]);
    expect(value.nan).toBeNull();
    expect(value.inf).toBeNull();
  });

  it("限制递归深度与数组长度（防御循环结构与无界列表）", () => {
    const deep: Record<string, unknown> = {};
    let cursor = deep;
    for (let index = 0; index < 40; index += 1) {
      const next: Record<string, unknown> = {};
      cursor.next = next;
      cursor = next;
    }
    expect(() => redactValue(deep)).not.toThrow();

    const wide = redactValue({ items: Array.from({ length: 1_000 }, (_value, index) => index) }) as {
      items: unknown[];
    };
    expect(wide.items.length).toBe(500);
  });
});

describe("containsForbiddenMaterial（闭合不变量）", () => {
  it("对原始泄漏载荷返回 true", () => {
    expect(containsForbiddenMaterial({ secret: "x" })).toBe(true);
    expect(containsForbiddenMaterial({ a: "haven:ai:gw" })).toBe(true);
    expect(containsForbiddenMaterial({ a: "C:\\Users\\x\\y" })).toBe(true);
  });

  it("对脱敏后的载荷返回 false", () => {
    const cleaned = redactValue({
      secret: "sk-not-real-value",
      a: "haven:ai:gw",
      b: "C:\\Users\\someone\\x",
      digest: "a".repeat(64),
    });
    expect(containsForbiddenMaterial(cleaned)).toBe(false);
  });
});
