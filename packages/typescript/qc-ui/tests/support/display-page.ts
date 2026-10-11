import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const uiSource = fileURLToPath(new URL("../../src/", import.meta.url));
const themeCss = fileURLToPath(new URL("../../../qc-theme/src/theme.css", import.meta.url));
const windowsStyles = fileURLToPath(new URL("../../../../../apps/windows/src/styles.css", import.meta.url));
const androidStyles = fileURLToPath(new URL("../../../../../apps/android/src/styles.css", import.meta.url));

export const VIEWPORTS = [[1440, 820], [920, 640], [390, 844], [844, 390]] as const;
export const SCREENS = ["live-grid", "settings-device", "directory-presets", "device-search"] as const;
export type HostName = "windows" | "android";

/** The browser to lay pages out in: QC_BROWSER_EXECUTABLE, else a stock Chrome/Chromium install. */
export function findBrowser(): string | undefined {
  const candidates = [
    process.env.QC_BROWSER_EXECUTABLE,
    "C:/Program Files/Google/Chrome/Application/chrome.exe",
    "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
  ];
  return candidates.find((path): path is string => Boolean(path) && existsSync(path as string));
}

/** Bundle a module (with every stylesheet it imports) the way the app bundles it, and import the result. */
export async function bundleModule<T>(workDirectory: string, name: string, source: string): Promise<{ module: T; css: string }> {
  const { build } = await import("esbuild");
  const entry = join(workDirectory, `${name}.tsx`);
  writeFileSync(entry, source);
  const outfile = join(workDirectory, `${name}.bundle.mjs`);
  await build({
    entryPoints: [entry],
    outfile,
    bundle: true,
    format: "esm",
    platform: "node",
    jsx: "automatic",
    loader: { ".ttf": "dataurl", ".svg": "dataurl", ".png": "dataurl", ".woff2": "dataurl" },
    logLevel: "error",
    absWorkingDir: workDirectory,
    nodePaths: [fileURLToPath(new URL("../../../../../node_modules", import.meta.url))],
    banner: { js: "import { createRequire as __createRequire } from 'node:module'; const require = __createRequire(import.meta.url);" }
  });
  const module = await import(new URL(`file:///${outfile.replace(/\\/g, "/")}`).href) as T;
  const cssFile = outfile.replace(/\.mjs$/, ".css");
  return { module, css: existsSync(cssFile) ? readFileSync(cssFile, "utf8") : "" };
}

/**
 * Bundles the real QuadCortexSurface (and every stylesheet it pulls in) with
 * esbuild, renders it to static markup for each screen and presentation, and
 * returns the markup plus the one stylesheet. No app, no dev server and no
 * network is involved: this is the shared display exactly as both hosts mount
 * it, minus their own chrome.
 */
export async function renderDisplays(workDirectory: string): Promise<{ css: string; markup: Record<string, string> }> {
  const { module: bundle, css } = await bundleModule<{ render(screen: string, presentation: string): Promise<string> }>(workDirectory, "entry", `
import { createElement } from "react";
import { prerenderToNodeStream } from "react-dom/static";
import { QuadCortexSurface } from ${JSON.stringify(join(uiSource, "quad-cortex-surface.tsx"))};
import { IDLE_PLACEHOLDER_SNAPSHOT } from ${JSON.stringify(join(uiSource, "index.ts"))};
import { formFactors, skins } from "@qc-remote/form-factors";
import "@qc-remote/theme/theme.css";

const formFactor = formFactors[0];
const skin = skins.find((entry) => entry.id === formFactor.defaultSkinId) ?? skins[0];
const SCREEN_CONTENT = createElement("section", { className: "qc-screen" }, "Display text");
export async function render(screen, presentation) {
  const view = screen === "live-grid" ? undefined : screen;
  const { prelude } = await prerenderToNodeStream(createElement(QuadCortexSurface, {
    formFactor, skin, snapshot: IDLE_PLACEHOLDER_SNAPSHOT, selectedBlockId: "", presentation, screenView: view, onAction() {}, screenContent: screen === "screen-content" ? SCREEN_CONTENT : undefined
  }));
  const chunks = [];
  for await (const chunk of prelude) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}
`);
  const markup: Record<string, string> = {};
  for (const screen of SCREENS) {
    for (const presentation of ["hardware", "display"]) markup[`${screen}/${presentation}`] = await bundle.render(screen, presentation);
  }
  return { css, markup };
}

/** What each host puts around the display: a container, and nothing else the display reads. */
function hostBody(host: HostName, surface: string): string {
  return host === "windows"
    ? `<div id="root"><div class="app-shell"><div class="menu-bar"></div><div class="app-content"><main class="workspace view-fit">${surface}</main></div><div class="status-strip">status</div></div></div>`
    : `<div id="root"><main class="android-app"><header class="mobile-header"><div class="mobile-brand"><span><strong>QC Remote</strong><small>v0</small></span></div></header><section class="mobile-screen">${surface}</section><div class="quick-controls"></div><div class="mobile-status-notice">notice</div></main></div>`;
}

const MEASURE_SCRIPT = `
(async () => {
  // React reveals streamed Suspense boundaries (the lazily loaded screens) on a short timer after load.
  for (let waited = 0; waited < 5000 && document.querySelector('template[id^="B:"]'); waited += 50) await new Promise((resolve) => setTimeout(resolve, 50));
  // The canvas scale is a container-query length; give style and layout time to settle on bezel width / 800.
  const scaleSettled = () => { const box = document.querySelector('.qc-screen-bezel'); const canvas = box.querySelector(':scope > .qc-screen-fixture-root'); return Math.abs(new DOMMatrix(getComputedStyle(canvas).transform).a * 800 - box.getBoundingClientRect().width) < 0.01; };
  for (let waited = 0; waited < 5000 && !scaleSettled(); waited += 50) await new Promise((resolve) => setTimeout(resolve, 50));
  await document.fonts.ready;
  await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  const bezel = document.querySelector(".qc-screen-bezel");
  const root = bezel.querySelector(":scope > .qc-screen-fixture-root");
  const box = (element) => { const r = element.getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; };
  const bezelStyle = getComputedStyle(bezel);
  const matrix = new DOMMatrix(getComputedStyle(root).transform);
  const texts = [];
  for (const element of bezel.querySelectorAll("*")) {
    const own = [...element.childNodes].filter((node) => node.nodeType === 3 && node.textContent.trim());
    if (!own.length) continue;
    const range = document.createRange();
    own.forEach((node) => range.selectNode(node));
    const r = range.getBoundingClientRect();
    texts.push({ text: element.textContent.trim().slice(0, 24), fontSize: parseFloat(getComputedStyle(element).fontSize), box: [r.left, r.top, r.width, r.height] });
  }
  const svg = root.querySelector("svg.coros-vector-canvas");
  return { innerWidth, innerHeight, bezel: box(bezel), border: [bezelStyle.borderLeftWidth, bezelStyle.paddingLeft], root: box(root), matrix: [matrix.a, matrix.b, matrix.c, matrix.d], svg: svg ? box(svg) : null, texts };
})()
`;

type Box = [number, number, number, number];

export interface DisplayMeasurement {
  key: string;
  innerWidth: number;
  innerHeight: number;
  bezel: Box;
  border: [string, string];
  root: Box;
  matrix: [number, number, number, number];
  svg: Box | null;
  texts: Array<{ text: string; fontSize: number; box: Box }>;
}

type CdpMessage = { id?: number; method?: string; sessionId?: string; result?: any; error?: { message: string } };

/** A tiny Chrome DevTools Protocol client: enough to open pages at a viewport size and evaluate a script in them. */
async function withBrowser<T>(browser: string, workDirectory: string, browserArguments: string[], run: (page: (viewport: readonly [number, number], url: string, script: string) => Promise<unknown>) => Promise<T>): Promise<T> {
  const profile = join(workDirectory, `profile-${Math.random().toString(36).slice(2)}`);
  mkdirSync(profile, { recursive: true });
  const child = spawn(browser, ["--headless=new", "--disable-gpu", "--no-sandbox", "--no-first-run", "--remote-debugging-port=0", `--user-data-dir=${profile}`, ...browserArguments, "about:blank"], { stdio: "ignore" });
  try {
    const portFile = join(profile, "DevToolsActivePort");
    for (let attempt = 0; !existsSync(portFile); attempt += 1) {
      if (attempt > 300) throw new Error("the browser did not open a debugging port");
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    const [port] = readFileSync(portFile, "utf8").split("\n");
    const version = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json() as { webSocketDebuggerUrl: string };
    const socket = new WebSocket(version.webSocketDebuggerUrl);
    await new Promise<void>((resolve, reject) => { socket.onopen = () => resolve(); socket.onerror = () => reject(new Error("debugging socket failed")); });
    let nextId = 0;
    const pending = new Map<number, { method: string; resolve(result: unknown): void; reject(error: Error): void }>();
    const waiters: Array<{ method: string; sessionId?: string; resolve: () => void }> = [];
    socket.onmessage = (event) => {
      const message = JSON.parse(String(event.data)) as CdpMessage;
      if (message.id !== undefined) {
        const request = Number.isInteger(message.id) ? pending.get(message.id) : undefined;
        if (!request) return;
        pending.delete(message.id);
        if (message.error) request.reject(new Error(`${request.method}: ${message.error.message}`));
        else request.resolve(message.result);
      } else {
        const index = waiters.findIndex((waiter) => waiter.method === message.method && waiter.sessionId === message.sessionId);
        if (index >= 0) waiters.splice(index, 1)[0].resolve();
      }
    };
    const send = (method: string, params: object = {}, sessionId?: string) => new Promise<any>((resolve, reject) => {
      nextId += 1;
      pending.set(nextId, { method, resolve, reject });
      socket.send(JSON.stringify({ id: nextId, method, params, sessionId }));
    });
    const { targetId } = await send("Target.createTarget", { url: "about:blank" });
    const { sessionId } = await send("Target.attachToTarget", { targetId, flatten: true });
    await send("Page.enable", {}, sessionId);
    const page = async (viewport: readonly [number, number], url: string, script: string) => {
      await send("Emulation.setDeviceMetricsOverride", { width: viewport[0], height: viewport[1], deviceScaleFactor: 1, mobile: false }, sessionId);
      const loaded = new Promise<void>((resolve) => waiters.push({ method: "Page.loadEventFired", sessionId, resolve }));
      await send("Page.navigate", { url }, sessionId);
      await loaded;
      const evaluated = await send("Runtime.evaluate", { expression: script, awaitPromise: true, returnByValue: true }, sessionId);
      if (evaluated.exceptionDetails) throw new Error(`page script failed: ${JSON.stringify(evaluated.exceptionDetails).slice(0, 300)}`);
      return evaluated.result.value;
    };
    try { return await run(page); } finally { socket.close(); }
  } finally {
    const exited = new Promise<void>((resolve) => { child.once("exit", () => resolve()); setTimeout(resolve, 5000); });
    child.kill();
    await exited;
  }
}

/** Lays every host x viewport x screen combination out in one headless Chrome, one page at a time at the stated viewport size. */
export async function measureLayouts(browser: string, workDirectory: string, css: string, markup: Record<string, string>, browserArguments: string[] = []): Promise<DisplayMeasurement[]> {
  const pagesDirectory = join(workDirectory, "pages");
  mkdirSync(pagesDirectory, { recursive: true });
  writeFileSync(join(pagesDirectory, "display.css"), css);
  const cases: Array<{ key: string; viewport: readonly [number, number]; url: string }> = [];
  for (const host of ["windows", "android"] as const) {
    writeFileSync(join(pagesDirectory, `${host}.css`), readFileSync(host === "windows" ? windowsStyles : androidStyles, "utf8"));
    for (const viewport of VIEWPORTS) {
      for (const screen of SCREENS) {
        const presentation = host === "windows" ? "hardware" : "display";
        const key = `${host}-${viewport[0]}x${viewport[1]}-${screen}`;
        const file = join(pagesDirectory, `${key}.html`);
        writeFileSync(file, `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="display.css"><link rel="stylesheet" href="${host}.css"></head><body>${hostBody(host, markup[`${screen}/${presentation}`])}</body></html>`);
        cases.push({ key, viewport, url: pathToFileURL(file).href });
      }
    }
  }
  return withBrowser(browser, workDirectory, browserArguments, async (page) => {
    const results: DisplayMeasurement[] = [];
    for (const { key, viewport, url } of cases) results.push({ key, ...(await page(viewport, url, MEASURE_SCRIPT) as Omit<DisplayMeasurement, "key">) });
    return results;
  });
}

export function makeWorkDirectory(): { path: string; cleanup(): void } {
  const path = mkdtempSync(join(tmpdir(), "qc-display-"));
  return { path, cleanup: () => { try { rmSync(path, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 }); } catch { /* a temp directory the browser still holds is the OS temp cleaner's */ } } };
}
