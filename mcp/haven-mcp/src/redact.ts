// 输出脱敏与限长。
//
// 规范：docs/architecture/AI_SYSTEM.md §4（凭据边界）与 §3.3（Allowed/Forbidden）。
//
// 为什么需要它：MCP 响应会被写进**外部模型客户端的对话上下文**。一旦某个桥接实现
// 顺手把凭据 target、绝对路径或 Provider 原始报文放进来，泄漏就已经发生，且不可撤回。
// 因此这里不是"可选的美化"，而是所有 tool 返回值的必经关口：
//   1. 结构层：denylist 键整键丢弃（防御桥接侧意外带上字段）；
//   2. 内容层：字符串里的凭据 target、绝对路径、Bearer/API key 形状一律打码；
//   3. 长度层：单字段限长，防止一个超大字符串把响应撑爆。
//
// 这里**不**尝试做"智能判断"：规则是确定的、可测试的，宁可多打码也不放过。

import { MAX_TEXT_FIELD_CHARS } from "./constants.js";

/**
 * 结构层 denylist：这些键一旦出现在任意层级的对象里，**整个键被丢弃**。
 *
 * 全部小写比较，因此 `apiKey` / `api_key` / `API_KEY` 都会被命中。
 *
 * 为什么是丢弃而不是替换成 `[REDACTED]`：保留键名会让"安全"变成一个需要解释的状态
 * （`secret: "[REDACTED]"` 到底是空还是有值？），而 MCP 响应是投影、不是数据库行——
 * 不该出现的字段就不该有位置。丢弃之后，"结果里不存在任何 denylist 键"成为一条
 * 可以直接断言的闭合不变量（见 `containsForbiddenMaterial`）。
 *
 * 注意两个**刻意排除**的键：
 * - `target`：资源偏好与设置提案的 Wire 词汇里 `target` 指"作用域"，不是凭据 target。
 *   真正的凭据 target（`haven:<provider>:<id>`）由字符串层拦截，因此不需要靠键名兜底。
 * - `digest` / 十六进制串：提案 digest 是 64 位小写十六进制，**必须原样返回**给用户
 *   在 UI 上核对。任何"通用十六进制打码"规则都会破坏这条核心契约，所以这里没有它。
 */
const FORBIDDEN_KEYS = new Set([
  "secret",
  "secrets",
  "api_key",
  "apikey",
  "password",
  "passwd",
  "token",
  "access_token",
  "refresh_token",
  "approval_token",
  "approvaltoken",
  "authorization",
  "bearer",
  "credentialref",
  "credential_ref",
  "credential",
  "rootpath",
  "root_path",
  "rootref",
  "root_ref",
  "absolutepath",
  "absolute_path",
  "localpath",
  "local_path",
  "filepath",
  "file_path",
  "path",
  "sql",
  "raw",
  "rawresponse",
  "raw_response",
  "providerresponse",
  "provider_response",
  "cookies",
  "cookie",
  "headers",
  "stack",
  "stacktrace",
  "stack_trace",
  "sqlite",
  "sql_statement",
  "query",
  "db_row",
  "dbrow",
  "rowid",
  // 数据库行的原始列名。桥接层若把投影前的行直接倒出来，这些键会出现；
  // 一个"看起来像模型输出"的响应背后其实是 SELECT * 的结果，是最难发现的一种越界。
  "data_json",
  "payload_json",
]);

/**
 * 内容层模式：命中即整段替换。
 *
 * 顺序有意义：先处理带命名空间的凭据 target，再处理通用路径与密钥形状。
 */
const STRING_PATTERNS: readonly { pattern: RegExp; replacement: string }[] = [
  // `haven:<provider>:<profile-id>` —— 凭据 target 绝不出 Haven。
  {
    pattern: /haven:[A-Za-z0-9_-]{1,60}:[A-Za-z0-9_-]{1,60}/g,
    replacement: "[REDACTED_CREDENTIAL_TARGET]",
  },
  // Windows 盘符路径（含反斜杠续段）。
  {
    pattern: /[A-Za-z]:\\(?:[^\\/:*?"<>|\r\n]+\\)*[^\\/:*?"<>|\r\n]*/g,
    replacement: "[REDACTED_PATH]",
  },
  // UNC 路径。
  { pattern: /\\\\[^\\/\s]+\\[^\s]*/g, replacement: "[REDACTED_PATH]" },
  // POSIX 绝对路径（只匹配常见根，避免误伤普通斜杠文本）。
  {
    pattern: /\/(?:home|Users|var|etc|opt|private|mnt|media|srv|root|tmp)\/[^\s"',;)\]}]*/g,
    replacement: "[REDACTED_PATH]",
  },
  // Bearer / Basic 认证头。
  { pattern: /\b(?:Bearer|Basic)\s+[A-Za-z0-9._~+/=-]{8,}/gi, replacement: "[REDACTED_AUTH]" },
  // 常见 API key 形状。
  { pattern: /\bsk-[A-Za-z0-9._-]{8,}/g, replacement: "[REDACTED_KEY]" },
  {
    pattern: /\b(?:ghp|gho|github_pat|xox[baprs])[-_][A-Za-z0-9._-]{8,}/g,
    replacement: "[REDACTED_KEY]",
  },
];

/** 对单个字符串做内容层脱敏 + 限长。 */
export function redactText(value: string): string {
  let result = value;
  for (const { pattern, replacement } of STRING_PATTERNS) {
    result = result.replace(pattern, replacement);
  }
  return boundText(result);
}

/**
 * 单字段限长。
 *
 * 超长文本按字符边界截断（不切断代理对），并显式标注被截断的字符数——
 * 静默截断会让模型以为自己看到了完整内容。
 */
export function boundText(value: string, limit: number = MAX_TEXT_FIELD_CHARS): string {
  // 按码点计数，避免把 emoji/中文切成半个字符。
  const codePoints = Array.from(value);
  if (codePoints.length <= limit) return value;
  return `${codePoints.slice(0, limit).join("")}…[已截断 ${codePoints.length - limit} 字符]`;
}

/** 递归脱敏任意 JSON 值。函数、Symbol、BigInt 等非 JSON 值一律丢弃。 */
export function redactValue(value: unknown, depth = 0): unknown {
  // 深度上界：防御桥接侧的循环结构或异常深的响应。
  if (depth > 12) return null;

  if (value === null || value === undefined) return null;
  if (typeof value === "string") return redactText(value);
  if (typeof value === "number") return Number.isFinite(value) ? value : null;
  if (typeof value === "boolean") return value;
  if (typeof value === "bigint") return null;
  if (typeof value === "function" || typeof value === "symbol") return null;

  if (Array.isArray(value)) {
    // 数组同样有上界：桥接层不该把无界列表塞过来。
    return value.slice(0, 500).map((item) => redactValue(item, depth + 1));
  }

  if (typeof value === "object") {
    const source = value as Record<string, unknown>;
    const result: Record<string, unknown> = {};
    for (const key of Object.keys(source).sort()) {
      // denylist 键整体丢弃（见 FORBIDDEN_KEYS 的说明）。
      if (FORBIDDEN_KEYS.has(key.toLowerCase())) continue;
      // 非 JSON 值所在的键也整键丢弃：`JSON.stringify` 本来就会漏掉函数/undefined，
      // 留一个 `key: null` 只会让"这里原本有东西"变成一个说不清的中间状态，
      // 而且 bigint 会让 outputSchema 校验直接失败。
      if (isNonJsonValue(source[key])) continue;
      result[key] = redactValue(source[key], depth + 1);
    }
    return result;
  }

  return null;
}

/**
 * 脱敏后的值是否仍包含被禁材料。
 *
 * 这是**闭合不变量**：一个通过 `redactValue` 的载荷必须满足
 * 「对象图里不存在任何 denylist 键」且「没有任何字符串命中内容层模式」。
 * 工具测试对每一个成功响应都断言它为 false。
 */
export function containsForbiddenMaterial(value: unknown): boolean {
  const text = typeof value === "string" ? value : (JSON.stringify(value) ?? "");
  for (const { pattern } of STRING_PATTERNS) {
    // 每次用一个新的正则实例，避免 lastIndex 状态在 /g 正则间泄漏。
    if (new RegExp(pattern.source, pattern.flags).test(text)) return true;
  }
  for (const key of Object.keys(snapshotKeys(value))) {
    if (FORBIDDEN_KEYS.has(key.toLowerCase())) return true;
  }
  return false;
}

/** 无法在 JSON 里表达的值：函数、symbol、undefined、bigint。 */
function isNonJsonValue(value: unknown): boolean {
  return (
    typeof value === "function"
    || typeof value === "symbol"
    || typeof value === "undefined"
    || typeof value === "bigint"
  );
}

/** 收集对象图里出现过的全部键（含嵌套），用于守门断言。 */
function snapshotKeys(value: unknown, into: Record<string, true> = {}, depth = 0): Record<string, true> {
  if (depth > 12 || value === null || typeof value !== "object") return into;
  if (Array.isArray(value)) {
    for (const item of value) snapshotKeys(item, into, depth + 1);
    return into;
  }
  for (const [key, child] of Object.entries(value as Record<string, unknown>)) {
    into[key] = true;
    snapshotKeys(child, into, depth + 1);
  }
  return into;
}
