/**
 * Shared Vite plugin that copies every legal document a build ships into
 * `dist/legal/`, for both native web shells.
 *
 * The list is qc-theme's `legal-documents.json` (exported to the apps as
 * QC_LEGAL_DOCUMENTS), the same list the Windows About window and the Android
 * notices panel render, so a document cannot be copied without being reachable
 * from the UI, or linked without being copied. It used to be ten copyFileSync
 * lines duplicated in each vite.config.ts beside a separate hand-written list in
 * each app.
 */
import { copyFileSync, mkdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const repositoryRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
export const legalDocumentsPath = resolve(repositoryRoot, "packages/typescript/qc-theme/src/legal-documents.json");

/** The legal documents every build ships, as `{ id, label, openLabel, bundledName, repositoryPath }`. */
export function qcLegalDocuments() {
  return JSON.parse(readFileSync(legalDocumentsPath, "utf8"));
}

/** Copies each listed document from the repository into `outputDirectory`; returns the bundled names. */
export function copyQcLegalDocuments(outputDirectory, root = repositoryRoot) {
  mkdirSync(outputDirectory, { recursive: true });
  return qcLegalDocuments().map((document) => {
    copyFileSync(resolve(root, document.repositoryPath), join(outputDirectory, document.bundledName));
    return document.bundledName;
  });
}

/** The Vite plugin: copies the legal documents into `outputDirectory` once the bundle is written. */
export function qcLegalAssets({ outputDirectory }) {
  return {
    name: "qc-remote-legal-assets",
    closeBundle() {
      copyQcLegalDocuments(outputDirectory);
    }
  };
}
