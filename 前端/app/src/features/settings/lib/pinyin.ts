// 完整字体拼音搜索（界面字体选择器）。
//
// 拼音由 pinyin-pro 生成，覆盖系统字体可能出现的汉字，不维护容易过期的手写字表。
// 原始中文键仍然保留，因此中文、拼音全拼、拼音首字母和英文名都能搜索。

import { pinyin } from "pinyin-pro";

/**
 * 归一化搜索串：小写、去空格与常见分隔符。
 * 只用于比较，不用于展示。
 */
export function normalizeFontQuery(input: string): string {
  return input.toLowerCase().replace(/[\s\-_·.]+/g, "");
}

/**
 * 为字体名生成原文、全拼和拼音首字母搜索键。
 * nonZh: consecutive 保留混合字体名中的连续英文与数字，再统一移除分隔符。
 */
export function fontSearchKeys(text: string): string[] {
  const keys = new Set<string>();
  const original = normalizeFontQuery(text);
  if (original.length > 0) keys.add(original);

  const options = { toneType: "none" as const, nonZh: "consecutive" as const, traditional: true };
  const fullPinyin = normalizeFontQuery(pinyin(text, options));
  const initials = normalizeFontQuery(pinyin(text, { ...options, pattern: "first" }));
  if (fullPinyin.length > 0) keys.add(fullPinyin);
  if (initials.length > 0) keys.add(initials);
  return [...keys];
}
