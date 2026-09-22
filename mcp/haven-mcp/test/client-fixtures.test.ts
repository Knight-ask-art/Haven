import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const TEST_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const FIXTURES_ROOT = path.join(TEST_ROOT, "fixtures", "clients");
const EXPECTED_FILES = ["claude-code.json", "codex.json", "dsh.json", "pi.json"];

type ClientFixture = {
  fixture: boolean;
  verified: boolean;
  client: string;
  server: {
    command: string;
    args: string[];
    env: Record<string, string>;
  };
};

function loadFixtures(): ClientFixture[] {
  return EXPECTED_FILES.map((file) =>
    JSON.parse(readFileSync(path.join(FIXTURES_ROOT, file), "utf8")) as ClientFixture,
  );
}

describe("external client MCP fixtures", () => {
  it("contains exactly the four declared, explicitly unverified fixtures", () => {
    expect(readdirSync(FIXTURES_ROOT).sort()).toEqual(EXPECTED_FILES);
    const fixtures = loadFixtures();
    expect(fixtures.every((fixture) => fixture.fixture && !fixture.verified)).toBe(true);
    expect(fixtures.map((fixture) => fixture.client).sort()).toEqual([
      "Claude Code",
      "Codex",
      "DSH",
      "Pi",
    ]);
  });

  it("normalizes every client to the same live Haven MCP server shape", () => {
    const normalized = loadFixtures().map((fixture) => fixture.server);
    expect(normalized).toEqual(
      Array.from({ length: EXPECTED_FILES.length }, () => ({
        command: "node",
        args: ["<repo>/mcp/haven-mcp/dist/index.js"],
        env: {
          HAVEN_MCP_BRIDGE: "live",
          HAVEN_MCP_ENDPOINT: "<copy-from-haven-ui>",
        },
      })),
    );
  });
});
