import { defineConfig } from "vitest/config";
import { cloudflareTest } from "@cloudflare/vitest-plugin";

export default defineConfig({
  plugins: [
    cloudflareTest({
      wrangler: { configPath: "./wrangler.kv-proxy-test.jsonc" },
      miniflare: { kvNamespaces: ["TEST_KV", "QCRC_OAUTH", "OAUTH_KV"] },
    }),
  ],
  test: {
    include: ["tests/workers-runtime/kv-proxy.test.ts"],
    testTimeout: 30_000,
    setupFiles: ["tests/workers-runtime/network-setup.ts"],
  },
});
