import brand from "./brand.json" with { type: "json" };
import legalDocuments from "./legal-documents.json" with { type: "json" };

export const QC_LEGAL = {
  copyright: brand.copyrightNotice,
  license: "QC Remote's own source code is licensed under MIT OR Apache-2.0. Device-derived reference material (model catalogues, factory names, and text and icons transcribed from the device) and the reconstructed device screens are covered by separate terms, LicenseRef-QC-Remote-Reference: they are included for interoperability only, no license is granted in Neural DSP's material, and that material remains the property of its owners. The project license and those terms are included with each build.",
  independence: "QC Remote is an independent, unofficial application. It is not affiliated with, authorized, sponsored, endorsed, or supported by Neural DSP Technologies Oy.",
  trademarks: "Neural DSP is a trademark, and Quad Cortex® a registered trademark, of Neural DSP Technologies Oy. Those names are used only to identify the product with which this application is compatible. All other trademarks belong to their respective owners.",
  productSafety: "QC Remote uses the connected device's existing interface. It does not contain, replace, or modify Quad Cortex firmware. Review device changes, use safe output levels, and keep independent backups of important presets.",
  publisher: {
    website: brand.publisherPublicWebsite,
    securityPolicyUrl: brand.securityPolicyUrl
  },
  privacy: {
    // A pointer, deliberately without facts of its own. What QC Remote keeps,
    // who operates the relay and for how long are stated once, in PRIVACY.md
    // (published at brand.privacyPolicyUrl). A second copy in the app drifts.
    pointer: "This screen only points to the privacy policy, the one authoritative text. Open it for what stays on your device, what the optional remote relay receives and keeps and for how long, and how to delete it."
  },
  thirdParty: [
    "The application includes open-source components under their respective licenses, including React, Tauri or Capacitor, and the native Rust device stack. Complete locked dependency notices and a machine-readable license inventory are included with each build.",
    "Protocol work incorporates information derived from the MIT-licensed pyquadcortex community project. QC Remote is independently developed and does not include Quad Cortex firmware.",
    "Preset and workspace files are processed locally unless the user separately enables a remote relay."
  ]
} as const;

/**
 * A legal document every build ships.
 *
 * `bundledName` is the file name under `dist/legal/` (and the Windows bundle's
 * `legal/` resources); `repositoryPath` is where the document lives in the
 * source tree. `label` names it in a list and `openLabel` is the text of a link
 * that opens it. This one list drives the shared Vite plugin that copies the
 * documents (scripts/vite-qc-legal-assets.mjs), the Windows About window and
 * the Android notices panel, so a document added here reaches every build.
 */
export type QcLegalDocument = { id: string; label: string; openLabel: string; bundledName: string; repositoryPath: string };

export const QC_LEGAL_DOCUMENTS: readonly QcLegalDocument[] = legalDocuments;
