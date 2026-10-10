// The notice-copy pair and the device-knowledge skill's licence
// copies and links in scripts/verify-license-carveout.mjs.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  bundledSkillFiles,
  DEVICE_KNOWLEDGE_SKILL_BUNDLE,
  DEVICE_KNOWLEDGE_SKILL_DIRECTORY,
  licenseCopyErrors,
  NOTICE_TEXT_COPIES,
  REFERENCE_TERMS_PATH,
  relativeMarkdownLinkTargets,
  SKILL_DOCUMENT_COPIES,
  skillDocumentErrors,
  trackedRepositoryFiles
} from "./verify-license-carveout.mjs";

const [[copy, canonical]] = NOTICE_TEXT_COPIES;
const check = (files) => licenseCopyErrors({ trackedFiles: Object.keys(files), readFile: (file) => files[file] });

test("the community protocol license and the crate's pyquadcortex notice are one text", () => {
  assert.equal(copy, "packages/rust/quadcortex/licenses/pyquadcortex-MIT.txt");
  assert.equal(canonical, "legal/COMMUNITY-PROTOCOL-LICENSE.txt");
  assert.deepEqual(check({ [copy]: "MIT notice\n", [canonical]: "MIT notice\r\n" }), [], "line endings aside, identical copies pass");
  assert.ok(check({ [copy]: "MIT notice, edited\n", [canonical]: "MIT notice\n" }).some((error) => error.includes(`${copy} differs from ${canonical}`)));
  assert.ok(check({ [canonical]: "MIT notice\n" }).some((error) => error.includes(`${copy} is missing`)));
});

const skill = (files) => skillDocumentErrors({ trackedFiles: Object.keys(files), readFile: (file) => files[file] });
const bundle = (paths) => paths.map((path) => `    device_knowledge_skill_file!("${path}"),`).join("\n");
const skillFixture = () => {
  const files = { [DEVICE_KNOWLEDGE_SKILL_BUNDLE]: bundle(SKILL_DOCUMENT_COPIES) };
  for (const path of SKILL_DOCUMENT_COPIES) {
    const text = path === "LICENSE" ? "MIT: [`LICENSE-MIT`](LICENSE-MIT). Terms: [x](LICENSES/LicenseRef-QC-Remote-Reference.txt#top). [web](https://example.org/a)\n" : `${path}\n`;
    files[path] = text;
    files[`${DEVICE_KNOWLEDGE_SKILL_DIRECTORY}/${path}`] = text;
  }
  return files;
};

test("the skill's licence links resolve to bundled files, and its document copies are byte copies", () => {
  assert.deepEqual(skill(skillFixture()), []);
  const crlf = skillFixture();
  crlf[`${DEVICE_KNOWLEDGE_SKILL_DIRECTORY}/TRADEMARKS.md`] = "TRADEMARKS.md\r\n";
  assert.ok(skill(crlf).some((error) => error.includes("TRADEMARKS.md differs from TRADEMARKS.md")), "a line-ending difference is a byte difference");
  const unbundled = skillFixture();
  unbundled[DEVICE_KNOWLEDGE_SKILL_BUNDLE] = bundle(SKILL_DOCUMENT_COPIES.filter((path) => path !== "LICENSE-MIT"));
  assert.ok(skill(unbundled).some((error) => error.includes("does not bundle LICENSE-MIT")));
  assert.ok(skill(unbundled).some((error) => error.includes("links LICENSE-MIT, which the skill does not carry")));
  const dangling = skillFixture();
  dangling[`${DEVICE_KNOWLEDGE_SKILL_DIRECTORY}/LICENSE`] += "[gone](NOTICE.md) [up](../README.md)\n";
  assert.ok(skill(dangling).some((error) => error.includes("links NOTICE.md")));
  assert.ok(skill(dangling).some((error) => error.includes("leaves the skill")));
});

test("the link check follows every bundled document, not only the licence texts", () => {
  // A companion document two links away from LICENSE: LICENSE -> CONTRIBUTING.md -> SECURITY.md -> legal/X.md.
  const files = skillFixture();
  const contributing = `${DEVICE_KNOWLEDGE_SKILL_DIRECTORY}/CONTRIBUTING.md`;
  files["CONTRIBUTING.md"] = files[contributing] = "[policy](SECURITY.md)\n";
  files["SECURITY.md"] = files[`${DEVICE_KNOWLEDGE_SKILL_DIRECTORY}/SECURITY.md`] = "[x](legal/X.md)\n";
  const errors = skill(files);
  assert.ok(errors.some((error) => error.includes("SECURITY.md links legal/X.md, which the skill does not carry")), errors.join("\n"));
  files["legal/X.md"] = files[`${DEVICE_KNOWLEDGE_SKILL_DIRECTORY}/legal/X.md`] = "x\n";
  files[DEVICE_KNOWLEDGE_SKILL_BUNDLE] = bundle([...SKILL_DOCUMENT_COPIES, "legal/X.md"]);
  assert.deepEqual(skill(files), [], "closed once the target is carried and bundled");
  files[DEVICE_KNOWLEDGE_SKILL_BUNDLE] = bundle([...SKILL_DOCUMENT_COPIES, "legal/X.md", "legal/Y.md"]);
  assert.ok(skill(files).some((error) => error.includes("bundles legal/Y.md, but")));
});

test("relative link targets resolve against the linking document", () => {
  assert.deepEqual(relativeMarkdownLinkTargets("LICENSE", "[a](LICENSE-MIT) [b](./LICENSES/T.txt#s) [c](mailto:x@y) [d](#here) [e](http://x/y)"), ["LICENSE-MIT", "LICENSES/T.txt"]);
  assert.deepEqual(relativeMarkdownLinkTargets("LICENSES/T.txt", "[a](../LICENSE) [b](U.txt) [c](../../out)"), ["LICENSE", "LICENSES/U.txt", null]);
});

test("the repository's skill carries every link target of every bundled file at the linked path", () => {
  const root = fileURLToPath(new URL("..", import.meta.url));
  const files = trackedRepositoryFiles(root);
  const read = (file) => readFileSync(resolve(root, file), "utf8");
  assert.deepEqual(skillDocumentErrors({ trackedFiles: files, readFile: read }), []);
  const targets = relativeMarkdownLinkTargets("LICENSE", read(`${DEVICE_KNOWLEDGE_SKILL_DIRECTORY}/LICENSE`));
  assert.ok(targets.includes(REFERENCE_TERMS_PATH) && targets.length >= 6, "LICENSE links the reference terms and the documents beside it");
  const bundled = bundledSkillFiles(read(DEVICE_KNOWLEDGE_SKILL_BUNDLE));
  for (const target of targets) assert.ok(bundled.includes(target), `${target} is bundled`);
  const reached = new Set(["LICENSE"]);
  for (const document of reached) {
    for (const target of relativeMarkdownLinkTargets(document, read(`${DEVICE_KNOWLEDGE_SKILL_DIRECTORY}/${document}`))) {
      assert.ok(target !== null && bundled.includes(target), `${document} links ${target}, which is bundled`);
      reached.add(target);
    }
  }
  for (const companion of ["SECURITY.md", "legal/THIRD_PARTY-LICENSE-TEXTS.txt", "legal/THIRD_PARTY-LICENSE-INVENTORY.json", "legal/THIRD_PARTY-SOURCE-OFFER.md"]) {
    assert.ok(reached.has(companion), `LICENSE reaches ${companion} through the bundled documents`);
  }
  assert.ok(!files.includes(`${DEVICE_KNOWLEDGE_SKILL_DIRECTORY}/LicenseRef-QC-Remote-Reference.txt`), "the terms sit where LICENSE links them, not at the skill root");
});
