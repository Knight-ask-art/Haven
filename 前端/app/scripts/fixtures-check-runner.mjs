// fixtures-check 执行器（R-C04：tsc --noEmit 之外，必须实际执行守卫）。
// 步骤：类型检查 → 编译到临时目录 → node 运行编译产物（运行时 guard 与语义断言）→ 清理。
// 失败时保留临时目录（便于排查），并在 stdout 打印失败原因。

import { execSync } from "node:child_process";
import { rmSync, readdirSync } from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const root = process.cwd();
const outDir = path.join(root, ".tmp-fixtures-check");

function tsc(args) {
  execSync(`npx tsc --ignoreConfig ${args.join(" ")}`, {
    stdio: "inherit",
    shell: true,
    cwd: root,
  });
}

try {
  // 1. 编译期类型检查（wire.ts 形状）
  tsc(["--noEmit", "--strict", "--moduleResolution", "bundler", "--module", "esnext", "--target", "es2022", "--skipLibCheck", "--resolveJsonModule", "scripts/fixtures-check.ts"]);

  // 2. 编译到临时目录（保留 JSON fixture 与 import attributes）
  rmSync(outDir, { recursive: true, force: true });
  tsc(["--outDir", ".tmp-fixtures-check", "--strict", "--moduleResolution", "bundler", "--module", "esnext", "--target", "es2022", "--skipLibCheck", "--resolveJsonModule", "scripts/fixtures-check.ts"]);

  // 3. 运行编译产物 → 执行运行时 guard 与语义断言
  // 注意排除 settings-fixtures-check.js：入口必须是主 fixtures-check.js（其 import 链已含 settings 检查）。
  const js = readdirSync(outDir, { recursive: true }).find(
    (f) => typeof f === "string" && f.endsWith("fixtures-check.js") && !f.endsWith("settings-fixtures-check.js"),
  );
  if (!js) {
    throw new Error("编译产物缺失（fixtures-check.js 未生成）");
  }
  await import(pathToFileURL(path.join(outDir, js)).href);
  console.log("fixtures:check PASS（编译 + 运行时守卫均已执行）");
} catch (err) {
  console.error("fixtures:check FAIL:", err.message);
  console.error(`临时产物保留于 ${outDir}（排查后可手动删除）`);
  process.exitCode = 1;
} finally {
  if (process.exitCode !== 1) {
    rmSync(outDir, { recursive: true, force: true });
  }
}
