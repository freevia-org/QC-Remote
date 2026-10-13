import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const repositoryPath = (path: string) => fileURLToPath(new URL(`../../${path}`, import.meta.url));
const androidSource = repositoryPath("apps/android/src");
const publicRoot = repositoryPath("");

export default defineConfig({
  plugins: [react()],
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
