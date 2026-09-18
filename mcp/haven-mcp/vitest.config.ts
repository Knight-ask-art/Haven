import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
    // MCP 的 in-memory transport 是异步的；默认 5s 足够，但列表类断言偶尔偏慢。
    testTimeout: 15_000,
  },
});
