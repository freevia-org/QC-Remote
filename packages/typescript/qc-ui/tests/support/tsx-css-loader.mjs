// Test-local loader: lets a headless test import a qc-ui .tsx module (esbuild strips the
// JSX with the automatic runtime, which react-singleton-loader.mjs then points at the
// renderer's React) and ignores the stylesheets such a module imports.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

export async function load(url, context, nextLoad) {
  if (url.endsWith(".css")) return { format: "module", source: "export {};", shortCircuit: true };
  if (url.endsWith(".tsx")) {
    const { transform } = await import("esbuild");
    const { code } = await transform(readFileSync(fileURLToPath(url), "utf8"), { loader: "tsx", jsx: "automatic", format: "esm", sourcefile: fileURLToPath(url) });
    return { format: "module", source: code, shortCircuit: true };
  }
  return nextLoad(url, context);
}
