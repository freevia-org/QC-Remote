import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { register } from "node:module";
import { act, create } from "react-test-renderer";
import { directoryCloudSignedInFromTree, QC_DIRECTORY_LINKED_IMAGES, QC_DIRECTORY_UNLINKED_IMAGES } from "../src/directory-cloud-link.ts";
import { classifyQcGraphicsTree } from "../src/use-adaptive-screen-polling.ts";

register("./support/react-singleton-loader.mjs", import.meta.url);
const React = (await import("react")).default;
const { useAdaptiveQcScreenPolling } = await import("../src/use-adaptive-screen-polling.ts");

const LINKED = readFileSync(new URL("./support/directory-linked-unit-tree.txt", import.meta.url), "utf8");
const withoutCloud = LINKED.replace("icons/24/cloud_upload.png", "icons/24/other.png");

test("the cloud-upload indicator in the Directory's top bar is a linked account", () => {
  assert.equal(classifyQcGraphicsTree(LINKED), "directory");
  assert.equal(directoryCloudSignedInFromTree(LINKED), true);
  assert.equal(directoryCloudSignedInFromTree(LINKED.replace(/\n/g, "\r\n")), true, "CRLF trees read the same");
});

test("the result is said only when the tree says it", () => {
  assert.equal(directoryCloudSignedInFromTree(withoutCloud), undefined, "an unregistered image is not read as signed out");
  assert.equal(directoryCloudSignedInFromTree(""), undefined);
  assert.equal(directoryCloudSignedInFromTree("zenUI::RootGraphicsItem\n  zenUI::Grid"), undefined);
  const outside = ["zenUI::RootGraphicsItem", "  zenUI::DirectoryDialog", "    zenUI::GraphicsItem", "      image: 'icons/24/cloud_upload.png'"].join("\n");
  assert.equal(directoryCloudSignedInFromTree(outside), undefined, "the image counts only inside a PushButtonIndicator");
  const elsewhere = ["zenUI::RootGraphicsItem", "  zenUI::DirectoryDialog", "  zenUI::PushButtonIndicator", "    image: 'icons/24/cloud_upload.png'"].join("\n");
  assert.equal(directoryCloudSignedInFromTree(elsewhere), undefined, "the image counts only inside the Directory dialog");
  const settings = LINKED.replace("DirectoryDialog", "SettingsDialog");
  assert.equal(directoryCloudSignedInFromTree(settings), undefined);
});

test("the registries hold only names, with no overlap", () => {
  for (const name of [...QC_DIRECTORY_LINKED_IMAGES, ...QC_DIRECTORY_UNLINKED_IMAGES]) assert.match(name, /^icons\/[A-Za-z0-9_/]+\.png$/);
  for (const name of QC_DIRECTORY_UNLINKED_IMAGES) assert.ok(!QC_DIRECTORY_LINKED_IMAGES.has(name));
  assert.doesNotMatch(LINKED, /@|[0-9A-Fa-f]{2}(:[0-9A-Fa-f]{2}){5}/, "the fixture carries no account detail");
});

test("a registered unlinked image reads as signed out, and an indicator showing both says nothing", () => {
  const name = "icons/24/synthetic_unlinked.png";
  (QC_DIRECTORY_UNLINKED_IMAGES as Set<string>).add(name);
  try {
    assert.equal(directoryCloudSignedInFromTree(withoutCloud.replace("icons/24/other.png", name)), false);
    assert.equal(directoryCloudSignedInFromTree(LINKED.replace("icons/24/search.png", name)), undefined);
  } finally { (QC_DIRECTORY_UNLINKED_IMAGES as Set<string>).delete(name); }
});

test("the poller hands the Directory's cloud state to the follower, and a change under one structure is an observation of its own", async (t) => {
  globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  let tree = withoutCloud;
  const seen: Array<{ kind: string; linked?: boolean }> = [];
  const gateway = { graphicsTree: async () => ({ tree }) };
  function Probe() {
    useAdaptiveQcScreenPolling({ gateway, enabled: true, connected: true, onObservation: (o) => seen.push({ kind: o.kind, linked: o.directoryCloudSignedIn }) });
    return null;
  }
  let renderer!: ReturnType<typeof create>;
  await act(async () => { renderer = create(React.createElement(Probe)); });
  t.after(async () => { await act(async () => renderer.unmount()); });
  assert.deepEqual(seen, [{ kind: "directory", linked: undefined }]);
  tree = LINKED.replace("icons/24/search.png", "icons/24/other.png");
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 400)); });
  assert.deepEqual(seen.at(-1), { kind: "directory", linked: true });
});
