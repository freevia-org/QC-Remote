// One legal-document list drives every place a build ships or shows its legal
// documents: the shared Vite plugin, the Windows bundle resources,
// the Windows About window and the Android notices panel.
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import { copyQcLegalDocuments, qcLegalAssets, qcLegalDocuments, repositoryRoot } from "./vite-qc-legal-assets.mjs";

const read = (path) => readFileSync(resolve(repositoryRoot, path), "utf8");
const documents = qcLegalDocuments();

test("the legal-document list names existing files once each, including the protocol licence", () => {
  assert.ok(documents.length >= 9);
  for (const field of ["id", "bundledName", "repositoryPath"]) {
    const values = documents.map((document) => document[field]);
    assert.equal(new Set(values).size, values.length, `duplicate ${field}`);
  }
  for (const document of documents) {
    for (const field of ["id", "label", "openLabel", "bundledName", "repositoryPath"]) assert.equal(typeof document[field], "string", `${document.id}.${field}`);
    assert.ok(existsSync(resolve(repositoryRoot, document.repositoryPath)), `${document.repositoryPath} does not exist`);
  }
  for (const required of ["THIRD_PARTY-NOTICES.md", "legal/THIRD_PARTY-LICENSE-TEXTS.txt", "legal/THIRD_PARTY-SOURCE-OFFER.md", "legal/THIRD_PARTY-LICENSE-INVENTORY.json", "legal/COMMUNITY-PROTOCOL-LICENSE.txt", "LICENSE", "LICENSES/LicenseRef-QC-Remote-Reference.txt", "LICENSE-MIT", "LICENSE-APACHE"]) {
    assert.ok(documents.some((document) => document.repositoryPath === required), `the list does not ship ${required}`);
  }
});

test("the shared plugin copies exactly the listed documents, byte for byte", () => {
  const output = mkdtempSync(join(tmpdir(), "qc-legal-"));
  try {
    const copied = copyQcLegalDocuments(output);
    assert.deepEqual(copied, documents.map((document) => document.bundledName));
    for (const document of documents) {
      assert.ok(readFileSync(join(output, document.bundledName)).equals(readFileSync(resolve(repositoryRoot, document.repositoryPath))), document.bundledName);
    }
    assert.equal(qcLegalAssets({ outputDirectory: output }).name, "qc-remote-legal-assets");
  } finally {
    rmSync(output, { recursive: true, force: true });
  }
});

test("both apps use the shared plugin and hard-code no legal document", () => {
  for (const app of ["windows", "android"]) {
    const config = read(`apps/${app}/vite.config.ts`);
    assert.match(config, /import \{ qcLegalAssets \} from "\.\.\/\.\.\/scripts\/vite-qc-legal-assets\.mjs";/, app);
    assert.match(config, /qcLegalAssets\(\{ outputDirectory: legalOutput \}\)/, app);
    assert.doesNotMatch(config, /copyFileSync/, `${app} copies files itself`);
    for (const document of documents) assert.ok(!config.includes(document.bundledName), `${app}/vite.config.ts names ${document.bundledName}`);
  }
});

test("the Windows bundle resources ship exactly the listed documents", () => {
  const resources = JSON.parse(read("apps/windows/src-tauri/tauri.conf.json")).bundle.resources;
  const legal = Object.entries(resources).filter(([, target]) => String(target).startsWith("legal/"));
  assert.deepEqual(
    legal.map(([source, target]) => `${source} -> ${target}`).sort(),
    documents.map((document) => `../../../${document.repositoryPath} -> legal/${document.bundledName}`).sort()
  );
});

test("the Windows About window and the Android notices panel render the shared list", () => {
  const about = read("apps/windows/src/about-content.ts");
  assert.match(about, /export const LEGAL_DOCUMENTS: readonly LegalDocument\[\] = QC_LEGAL_DOCUMENTS;/);
  for (const document of documents) assert.ok(!about.includes(document.bundledName), `about-content.ts restates ${document.bundledName}`);
  const android = read("apps/android/src/App.tsx");
  assert.match(android, /QC_LEGAL_DOCUMENTS\.map\(\(legalDocument\) => <p key=\{legalDocument\.id\}><a href=\{`\.\/legal\/\$\{legalDocument\.bundledName\}`\}/);
  assert.doesNotMatch(android, /href="\.\/legal\//, "the Android notices panel hard-codes a legal link");
  const theme = read("packages/typescript/qc-theme/src/legal.ts");
  assert.match(theme, /export const QC_LEGAL_DOCUMENTS: readonly QcLegalDocument\[\] = legalDocuments;/);
});
