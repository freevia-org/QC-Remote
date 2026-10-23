// QC Remote __QC_REMOTE_APP_VERSION__; tool contract __QC_REMOTE_TOOL_CONTRACT_SHA256__
"use strict";
const { spawn } = require("node:child_process");
// Claude/Electron supplies virtual stdio streams, not the OS file descriptors.
// Forward those streams explicitly so initialize reaches the signed child.
const child = spawn(__QC_REMOTE_NATIVE_EXECUTABLE_JSON__, ["--local-mcp-bridge", ...process.argv.slice(2)], { stdio: "pipe", windowsHide: true });
// This MCP server implements the standard initialize handshake, not Claude's
// experimental discovery protocol. Reject its probe rather than letting an
// SDK fallback accidentally return a tools/list result with the wrong schema.
const input = require("node:readline").createInterface({ input: process.stdin });
input.on("line", (line) => {
  let request;
  try { request = JSON.parse(line); } catch {}
  if (request?.method === "server/discover") {
    if (request.id !== undefined) process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, error: { code: -32601, message: "Method not found" } }) + "\n");
    return;
  }
  child.stdin.write(line + "\n");
});
input.on("close", () => child.stdin.end());
child.stdout.pipe(process.stdout);
child.stderr.pipe(process.stderr);
child.stdin.on("error", (error) => { if (error.code !== "EPIPE") console.error("QC Remote native input:", error.message); });
child.on("error", (error) => { input.close(); process.stdin.pause(); console.error("QC Remote native bridge:", error.message); process.exitCode = 1; });
child.on("exit", (code, signal) => { input.close(); process.stdin.pause(); process.exitCode = code ?? (128 + (require("node:os").constants.signals[signal] ?? 0)); });
for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, () => child.kill(signal));
