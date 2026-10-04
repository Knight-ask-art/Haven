// Tauri runs its build hook from the application root, not src-tauri.
// Resolve the product path from this file so a desktop build cannot embed an
// old dist left by a previous preview/session.
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const cwd = fileURLToPath(new URL("../../前端/app/", import.meta.url));
const result = spawnSync(process.platform === "win32" ? "npm.cmd" : "npm", ["run", "build"], {
  cwd, stdio: "inherit", shell: process.platform === "win32",
});
if (result.error) console.error("Desktop frontend build could not start.");
process.exit(result.status ?? 1);
