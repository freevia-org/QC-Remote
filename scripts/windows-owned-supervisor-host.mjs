// Lifetime-only launcher: no Job, command admission or byte deletion authority.
// A detached Node root survives its caller; PowerShell remains hidden and uses
// ordinary console startup (detached pwsh can exit before reading its script).
import { spawn } from "node:child_process";
const [executable, script, id] = process.argv.slice(2);
if (!executable || !script || !/^[A-Za-z0-9_-]{1,128}$/.test(id ?? "")) throw new Error("Invalid supervisor launch");
const child = spawn(executable, ["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", script], {
  windowsHide: true, stdio: [0, "pipe", "pipe"],
});
function forward(source, sink) {
  let closed = false;
  sink.on("error", () => {closed = true; source.resume();});
  source.on("error", () => {process.exitCode = 1;});
  source.on("data", chunk => {if (!closed && !sink.write(chunk)) source.pause();});
  sink.on("drain", () => source.resume());
}
forward(child.stdout, process.stdout);
forward(child.stderr, process.stderr);
if (child.pid) process.stdout.write(JSON.stringify({schemaVersion: 1, id, kind: "supervisor", processId: child.pid, launcherProcessId: process.pid}) + "\n");
child.once("error", () => {process.exitCode = 1;});
child.once("close", code => {process.exitCode = code ?? 1;});
