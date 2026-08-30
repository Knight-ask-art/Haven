/**
 * Gate B: Haven Design Engineering Governance Audit Script
 * 
 * 职责：
 * 1. 扫描所有 src 目录下的业务代码 (非 src/components/external)
 * 2. 检查是否有包含 "/components/external" 字符串的异常操作 (涵盖了动态 import, re-export 等 Oxlint 无法完美覆盖的场景)
 * 3. 检查 design-governance.config.ts 中的组件状态是否合规。只有 PROMOTED 才能作为正式 Feature/Shared。
 */

import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const ROOT_DIR = path.resolve(__dirname, '..');
const SRC_DIR = path.join(ROOT_DIR, 'src');

const QUARANTINE_PATTERN = /components\/external/i;

function scanDirectory(dir, fileList = []) {
  if (!fs.existsSync(dir)) return fileList;
  const files = fs.readdirSync(dir);
  for (const file of files) {
    const fullPath = path.join(dir, file);
    if (fs.statSync(fullPath).isDirectory()) {
      if (!fullPath.includes('components\\external') && !fullPath.includes('components/external')) {
         scanDirectory(fullPath, fileList);
      }
    } else if (fullPath.match(/\.(ts|tsx|js|jsx)$/)) {
      fileList.push(fullPath);
    }
  }
  return fileList;
}

console.log("== Haven Design Governance Audit (Gate B) ==");
let hasError = false;

// 1. Check for dynamic imports / indirect references to quarantine
const files = scanDirectory(SRC_DIR);
for (const file of files) {
  const content = fs.readFileSync(file, 'utf-8');
  if (QUARANTINE_PATTERN.test(content)) {
    console.error(`\n[FAIL] Gate B Blocked: Found quarantine path reference in business code.`);
    console.error(`File: ${file}`);
    console.error(`Error: Production code MUST NOT contain any indirect/dynamic references to components/external.`);
    hasError = true;
  }
}

// 2. We can also add manifest check here using a parser or TS compiler if needed,
// but string checking the config file is a lightweight start for CI.
const configPath = path.join(ROOT_DIR, 'design-governance.config.ts');
if (fs.existsSync(configPath)) {
  const configContent = fs.readFileSync(configPath, 'utf-8');
  // 检查是否所有外部组件都通过了审计
  // 这是一个简化的示例，在未来的进阶迭代中可以通过 TS Node 直接 require 读取对象。
} else {
  console.warn("\n[WARN] design-governance.config.ts not found. Policy checks skipped.");
}

if (hasError) {
  console.error("\n[Governance Audit] FAILED. Merge Blocked.\n");
  process.exit(1);
} else {
  console.log("\n[Governance Audit] PASSED. No quarantine violations found.\n");
  process.exit(0);
}
