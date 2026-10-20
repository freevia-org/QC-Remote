import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import { qcDemoDeviceAssets } from "../../packages/typescript/qc-ui/vite-demo-device-assets.mjs";

const repositoryPath = (path: string) => fileURLToPath(new URL(`../../${path}`, import.meta.url));
const androidSource = repositoryPath("apps/android/src");
const publicRoot = repositoryPath("");

export default defineConfig({
  plugins: [react(), qcDemoDeviceAssets({
    parameterScalesSource: repositoryPath("docs/reference/qc-live-parameter-scales.json"),
    parameterDefaultsSource: repositoryPath("docs/reference/qc-live-parameter-defaults.json"),
    referenceOutput: resolve("dist/reference")
  })],
  publicDir: false,
  base: "./",
  resolve: {
    alias: [
      { find: /^\.\/native-services$/, replacement: resolve("src/ios-services.ts") },
      { find: /^\.\/use-android-prompts$/, replacement: resolve("src/use-ios-prompts.tsx") },
      { find: "@qc-remote/android-src", replacement: androidSource }
    ]
  },
  server: { fs: { allow: [publicRoot, repositoryPath("")] } },
  build: { outDir: "dist", emptyOutDir: true }
});
