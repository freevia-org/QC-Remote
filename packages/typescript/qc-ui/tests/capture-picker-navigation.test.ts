import assert from "node:assert/strict";
import test from "node:test";
import { createRequire, register } from "node:module";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { act, create } from "react-test-renderer";
import type { CorOsAddDevicePickerState } from "../src/add-device-picker.tsx";

register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;

test("the live capture sidebar returns from a nested folder to its parent and then root", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  const work = mkdtempSync(join(tmpdir(), "qc-capture-picker-"));
  t.after(() => rmSync(work, { recursive: true, force: true }));
  const outfile = join(work, "picker.mjs");
  const rendererRequire = createRequire(createRequire(import.meta.url).resolve("react-test-renderer"));
  const { build } = await import("esbuild");
  await build({
    stdin: { contents: `export { CorOsAddDevicePicker } from ${JSON.stringify(fileURLToPath(new URL("../src/add-device-picker.tsx", import.meta.url)))};`, resolveDir: fileURLToPath(new URL("../", import.meta.url)), loader: "tsx" },
    outfile, bundle: true, format: "esm", platform: "node", jsx: "automatic", logLevel: "error",
    plugins: [{ name: "renderer-react", setup(builder) {
      builder.onResolve({ filter: /^react(?:\/jsx(?:-dev)?-runtime)?$/ }, ({ path }) => ({ path: pathToFileURL(rendererRequire.resolve(path)).href, external: true }));
    } }]
  });
  const { CorOsAddDevicePicker } = await import(pathToFileURL(outfile).href);
  const picker = {
    row: 0, column: 0, models: [], captures: [], activeCategory: "Neural Capture", loading: false, pending: false,
    query: "", pinnedModelIds: [], onQueryChange() {}, onCategoryChange() {}, onSelect() {}, onTogglePinned() {}, onClose() {},
    captureFolders: [
      { key: "root", name: "Library", isLibrary: true },
      { key: "parent", parentKey: "root", name: "Parent" },
      { key: "nested", parentKey: "parent", name: "Nested" },
      { key: "sibling", parentKey: "root", name: "Sibling" }
    ]
  } as unknown as CorOsAddDevicePickerState;
  let renderer!: ReturnType<typeof create>;
  act(() => { renderer = create(React.createElement(CorOsAddDevicePicker, { picker, cellCenter: { x: 100, y: 100 } })); });
  t.after(() => act(() => renderer.unmount()));
  const sidebar = () => renderer.root.findByProps({ "aria-label": "Capture folders" });
  const button = (label: string) => sidebar().findAllByType("button").find((node) => node.findAllByType("span").some((span) => span.children.join("") === label));
  assert.ok(button("Parent"));
  assert.equal(button("Back"), undefined);
  act(() => button("Parent")!.props.onClick());
  assert.ok(button("Nested"));
  act(() => button("Nested")!.props.onClick());
  assert.equal(button("Nested"), undefined);
  act(() => button("Back")!.props.onClick());
  assert.ok(button("Nested"), "Back preserves the immediate parent instead of resetting directly to root");
  act(() => button("Back")!.props.onClick());
  assert.ok(button("Parent"));
  assert.ok(button("Sibling"));
  assert.equal(button("Back"), undefined);
});
