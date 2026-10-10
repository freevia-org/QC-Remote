// In this test process, resolve React imports from qc-ui to the exact React
// instance used by react-test-renderer. This is test-local and does not alter
// the workspace dependency graph.
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

const testRequire = createRequire(new URL("../native-state-recovery-hook.test.ts", import.meta.url));
const rendererRequire = createRequire(testRequire.resolve("react-test-renderer"));
const rendererReact = new Map([
  ["react", rendererRequire.resolve("react")],
  ["react/jsx-runtime", rendererRequire.resolve("react/jsx-runtime")],
  ["react/jsx-dev-runtime", rendererRequire.resolve("react/jsx-dev-runtime")]
]);
const qcUiRoot = new URL("../../", import.meta.url).href;

export async function resolve(specifier, context, nextResolve) {
  const sharedReact = rendererReact.get(specifier);
  if (sharedReact && context.parentURL?.startsWith(qcUiRoot)) {
    return nextResolve(pathToFileURL(sharedReact).href, context);
  }
  return nextResolve(specifier, context);
}
