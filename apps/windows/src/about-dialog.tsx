/**
 * The About window's body: Privacy, Legal, Notices, and About as tabs.
 *
 * The Help menu used to open a guide, a keyboard reference, two identical
 * diagnostics exports, a privacy note, a legal note, and a notices list, each
 * as its own small dialog with no way across. The guide and feedback are now
 * windows of their own (`guide-content.tsx`, `help-window.tsx`); what remains
 * here is the reference material, as four tabs in one window. The notice
 * documents render from the bundled `dist/legal/` folder rather than as links
 * a webview refuses to follow.
 *
 * Every external destination goes through `openExternal`, which is the
 * allowlisted Rust command. Nothing here opens a link by itself.
 */

import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { QC_BRAND, QC_LEGAL } from "@qc-remote/theme";
import { ABOUT_TABS, LEGAL_DOCUMENTS, repositoryRoot, type AboutTab, type HelpWindowKind, type LegalDocument } from "./about-content";

const DOCUMENT_UNAVAILABLE = "This document ships with installed builds and is not available in this development session.";

/**
 * How much of a bundled document the viewer lays out.
 *
 * The license texts are 916 KB and the inventory is 428 KB; laying either out
 * as wrapped `<pre>` text to compute the scroll height of the viewer costs a
 * visible hitch. The cap keeps the panel responsive, and the whole document is
 * one button away.
 */
const DOCUMENT_CHARACTER_LIMIT = 200000;
const DOCUMENT_TRUNCATED = "… truncated; use Open on GitHub for the complete document.";

function renderedDocument(text: string): string {
  if (text.length <= DOCUMENT_CHARACTER_LIMIT) return text;
  return `${text.slice(0, DOCUMENT_CHARACTER_LIMIT)}\n${DOCUMENT_TRUNCATED}`;
}

/**
 * What a bundled document is doing right now.
 *
 * The status lives in the cache rather than in a separate "already requested"
 * ref, so leaving the tab or picking a second document mid-fetch cannot strand
 * the first one: its entry is still `pending`, the request is never abandoned,
 * and the result lands in the cache whenever it arrives.
 */
type DocumentState = { status: "pending" | "loaded" | "unavailable"; text?: string };

export function AboutWindowContent({ appVersion, openExternal, openHelpWindow, loadLegalDocument }: {
  appVersion: string;
  openExternal: (url: string) => Promise<void>;
  /** Opens (or focuses) the User Guide or Feedback window. */
  openHelpWindow: (kind: Exclude<HelpWindowKind, "about">) => Promise<void>;
  loadLegalDocument: (legalDocument: LegalDocument) => Promise<string>;
}) {
  const [tab, setTab] = useState<AboutTab>("about");
  const [documentId, setDocumentId] = useState(LEGAL_DOCUMENTS[0].id);
  // Document texts, kept for the life of the window so switching back to one
  // that has already been read costs nothing.
  const [documents, setDocuments] = useState<Record<string, DocumentState>>({});
  // What the window last said. It has no status strip of its own, so every
  // outcome is shown here, inside the window the reader is looking at.
  const [message, setMessage] = useState<string>();
  const tabButtons = useRef(new Map<AboutTab, HTMLButtonElement | null>());
  // Requests that have been started and have not settled. StrictMode runs the
  // loader effect twice before the `pending` write commits, so the cache alone
  // cannot stop a second read of a 916 KB file; a ref survives the simulated
  // remount and hands the second pass the promise the first one already made.
  const inFlight = useRef(new Map<string, Promise<string>>());
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  const activeDocument = LEGAL_DOCUMENTS.find((item) => item.id === documentId) ?? LEGAL_DOCUMENTS[0];
  const documentState = documents[activeDocument.id];

  useEffect(() => {
    if (tab !== "notices") return;
    const id = activeDocument.id;
    // Pending, loaded, or unavailable: all three mean there is nothing to start.
    if (documents[id]) return;
    if (!inFlight.current.has(id)) {
      const request = loadLegalDocument(activeDocument);
      inFlight.current.set(id, request);
      // Never abandoned: whatever this returns lands in the cache, so leaving
      // the tab or picking another document mid-read cannot strand this one.
      request
        .then((text) => { if (mounted.current) setDocuments((state) => ({ ...state, [id]: { status: "loaded", text } })); })
        .catch(() => { if (mounted.current) setDocuments((state) => ({ ...state, [id]: { status: "unavailable" } })); })
        .finally(() => { inFlight.current.delete(id); });
    }
    setDocuments((state) => state[id] ? state : { ...state, [id]: { status: "pending" } });
  }, [activeDocument, documents, loadLegalDocument, tab]);

  // A message belongs to the tab that produced it.
  useEffect(() => { setMessage(undefined); }, [tab]);

  // One path for every button: an action either names its own outcome by
  // resolving with it, or takes the caller's `success` line, and either way
  // the text lands in the panel.
  const report = async (run: () => Promise<string | void>, success?: string) => {
    try {
      const outcome = await run();
      const detail = typeof outcome === "string" ? outcome : success;
      if (detail) setMessage(detail);
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    }
  };

  const openDocumentOnGitHub = () => void report(() => openExternal(`${repositoryRoot(QC_BRAND.sourceRepositoryUrl)}/blob/HEAD/${activeDocument.repositoryPath}`));

  /** ARIA's tab pattern: the arrow keys move between tabs, Tab leaves the strip. */
  const onTabStripKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const index = ABOUT_TABS.findIndex((entry) => entry.id === tab);
    const last = ABOUT_TABS.length - 1;
    const target =
      event.key === "ArrowRight" ? (index + 1) % ABOUT_TABS.length :
      event.key === "ArrowLeft" ? (index + last) % ABOUT_TABS.length :
      event.key === "Home" ? 0 :
      event.key === "End" ? last : -1;
    if (target < 0) return;
    event.preventDefault();
    event.stopPropagation();
    const entry = ABOUT_TABS[target];
    setTab(entry.id);
    tabButtons.current.get(entry.id)?.focus();
  };

  return <>
    <header className="help-window-header">
      <div className="dialog-kicker">ABOUT</div>
      <h2 id="help-window-title">{QC_BRAND.appName} <span>{appVersion}</span></h2>
      <div className="about-tabs" role="tablist" aria-label={`About ${QC_BRAND.appName}`} onKeyDown={onTabStripKeyDown}>
        {ABOUT_TABS.map((entry) => <button
          key={entry.id}
          type="button"
          role="tab"
          id={`about-tab-${entry.id}`}
          aria-controls="about-panel"
          aria-selected={tab === entry.id}
          tabIndex={tab === entry.id ? 0 : -1}
          ref={(node) => { tabButtons.current.set(entry.id, node); }}
          className={tab === entry.id ? "is-active" : undefined}
          onClick={() => setTab(entry.id)}
        >{entry.label}</button>)}
      </div>
    </header>

    <div className="about-panel help-window-scroll" id="about-panel" role="tabpanel" tabIndex={0} aria-labelledby={`about-tab-${tab}`}>
      {tab === "privacy" && <>
        <h3>Privacy policy</h3>
        <p>{QC_LEGAL.privacy.pointer}</p>
        {QC_BRAND.privacyPolicyUrl && <div className="dialog-actions">
          <button type="button" onClick={() => void report(() => openExternal(QC_BRAND.privacyPolicyUrl))}>Open the complete privacy policy</button>
        </div>}
      </>}

      {tab === "legal" && <>
        <h3>Independent companion</h3>
        <p>{QC_LEGAL.independence}</p>
        <p>{QC_LEGAL.trademarks}</p>
        <p>{QC_LEGAL.productSafety}</p>
        <p>{QC_LEGAL.copyright}</p>
        <p>{QC_LEGAL.license}</p>
        {(QC_LEGAL.publisher.website || QC_BRAND.termsUrl || QC_LEGAL.publisher.securityPolicyUrl) && <div className="dialog-actions">
          {QC_LEGAL.publisher.website && <button type="button" onClick={() => void report(() => openExternal(QC_LEGAL.publisher.website))}>Publisher website</button>}
          {QC_BRAND.termsUrl && <button type="button" onClick={() => void report(() => openExternal(QC_BRAND.termsUrl))}>Open the terms</button>}
          {QC_LEGAL.publisher.securityPolicyUrl && <button type="button" onClick={() => void report(() => openExternal(QC_LEGAL.publisher.securityPolicyUrl))}>Open the security policy</button>}
        </div>}
      </>}

      {tab === "notices" && <>
        <h3>Runtime components</h3>
        {QC_LEGAL.thirdParty.map((notice) => <p key={notice}>{notice}</p>)}
        <div className="document-picker">
          {LEGAL_DOCUMENTS.map((entry) => <button
            key={entry.id}
            type="button"
            aria-pressed={entry.id === activeDocument.id}
            onClick={() => setDocumentId(entry.id)}
          >{entry.label}</button>)}
        </div>
        {documentState?.status === "loaded" && <pre className="legal-document">{renderedDocument(documentState.text ?? "")}</pre>}
        {documentState?.status === "unavailable" && <p>{DOCUMENT_UNAVAILABLE}</p>}
        {(documentState === undefined || documentState.status === "pending") && <p>Opening {activeDocument.label}…</p>}
        <div className="dialog-actions">
          <button type="button" onClick={openDocumentOnGitHub}>Open on GitHub</button>
        </div>
      </>}

      {tab === "about" && <>
        <p>An independent desktop companion for controlling a connected Quad Cortex.</p>
        <p className="legal-note">{QC_LEGAL.independence}</p>
        <p>{QC_LEGAL.copyright}</p>
        <div className="dialog-actions">
          <button type="button" onClick={() => void report(() => openHelpWindow("guide"))}>User guide</button>
          <button type="button" onClick={() => void report(() => openHelpWindow("feedback"))}>Report an issue</button>
          <button type="button" className="primary" onClick={() => void report(() => openExternal(QC_BRAND.sourceRepositoryUrl))}>Source code on GitHub</button>
        </div>
      </>}

      {message && <p className="about-message" role="status">{message}</p>}
    </div>
  </>;
}
