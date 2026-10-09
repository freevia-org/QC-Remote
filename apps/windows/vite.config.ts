import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
import { qcWebBuild } from "../../scripts/vite-qc-code-splitting.mjs";
import { qcLegalAssets } from "../../scripts/vite-qc-legal-assets.mjs";
import { qcDemoDeviceAssets } from "../../packages/typescript/qc-ui/vite-demo-device-assets.mjs";

const repositoryFile = (path: string) => fileURLToPath(new URL(`../../${path}`, import.meta.url));
const legalOutput = fileURLToPath(new URL("./dist/legal", import.meta.url));

const parameterScalesSource = repositoryFile("docs/reference/qc-live-parameter-scales.json");
const parameterDefaultsSource = repositoryFile("docs/reference/qc-live-parameter-defaults.json");
const referenceOutput = fileURLToPath(new URL("./dist/reference", import.meta.url));

export default defineConfig(() => ({
  plugins: [react(), qcLegalAssets({ outputDirectory: legalOutput }), qcDemoDeviceAssets({ parameterScalesSource, parameterDefaultsSource, referenceOutput })],
  build: qcWebBuild,
  // Theme assets are imported explicitly so local visual references can never
  // leak into development servers or release bundles through publicDir.
  publicDir: false,
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"]
    }
  },
  envPrefix: ["VITE_", "TAURI_"]
}));
