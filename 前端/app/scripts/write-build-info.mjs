import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const app = fileURLToPath(new URL("../", import.meta.url));
const root = path.resolve(app, "../..");
const dist = path.join(app, "dist");
const version = JSON.parse(readFileSync(path.join(app, "package.json"), "utf8")).version;
const git = (...args) => execFileSync("git", ["-C", root, ...args], { encoding: "utf8" }).trim();
const commit = git("rev-parse", "HEAD");
const dirty = git("status", "--porcelain", "--untracked-files=normal").length > 0;
function assets(directory, prefix = "") {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const relative = `${prefix}${entry.name}`;
    if (entry.isSymbolicLink()) throw new Error("Build assets must not be symbolic links.");
    if (entry.isDirectory()) return assets(path.join(directory, entry.name), `${relative}/`);
    if (relative === "build-info.json") return [];
    const bytes = readFileSync(path.join(directory, entry.name));
    return [{ path: relative, byteSize: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") }];
  });
}
const info = { schemaVersion: 1, version, commit, dirty, assets: assets(dist).sort((a, b) => a.path.localeCompare(b.path)) };
writeFileSync(path.join(dist, "build-info.json"), JSON.stringify(info, null, 2) + "\n");
console.log(`Desktop assets: ${version} / ${commit.slice(0, 12)} / ${dirty ? "working tree" : "committed"}`);
