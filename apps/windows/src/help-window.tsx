/**
 * The page a Help window renders: the User Guide, Feedback, or About.
 *
 * Each is a native window of its own (`open_help_window` in the Rust shell),
 * loaded as `index.html?window=<kind>`, so a reader can move and resize it
 * beside the controller instead of reading through a modal that covers it.
 * The page is the whole window; the OS title bar is its frame.
 *
 * This webview shares no React state with the main window. The diagnostics
 * report is built from the main window's live device and connection state, so
 * the Feedback buttons ask the main window for it over the event bus
 * (`requestDiagnostics`) and show its answer here. A bug report's issue link
 * carries the same report, fetched the same way when Open GitHub issue… is
 * pressed; its version and operating system this window asks for itself.
 */

import { useEffect, useRef, useState } from "react";
import { QC_BRAND } from "@qc-remote/theme";
import { AboutWindowContent } from "./about-dialog";
import { buildIssueLink, buildIssueUrl, feedbackIssueKind, type HelpWindowKind, type IssueKind, type LegalDocument } from "./about-content";
import { GuideContent } from "./guide-content";
import { externalLinks, helpWindows, hostSystem, onFeedbackIssueKind, requestDiagnostics } from "./tauri-transport";
import appPackage from "../package.json";
import "./help-windows.css";

export type { HelpWindowKind } from "./about-content";

const appVersion = appPackage.version;

/**
 * The host operating system, for the Platform field of a bug report.
 *
 * Not `runtime.platform`, which is the device gateway's own descriptor: that
 * answers "which broker" and belongs in the diagnostics report, where it stays.
 * WebView2 answers `userAgentData.platform` with "Windows". It is the fallback
 * for the Rust shell's registry-read version (`hostSystem.osVersion`), which
 * names the release and build. Where neither says, this is undefined and the
 * issue form is left for the reporter to fill rather than prefilled with
 * something untrue.
 */
const issuePlatform = (navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData?.platform || undefined;

/**
 * Read a bundled legal document out of the build's own `dist/legal/` folder.
 *
 * Module scope rather than component scope so the About window's loader effect
 * sees one stable function and fetches each document once. The parameter is
 * not called `document`: the global `document` is a different thing.
 */
const loadLegalDocument = async (legalDocument: LegalDocument): Promise<string> => {
  const response = await fetch(`./legal/${legalDocument.bundledName}`);
  if (!response.ok) throw new Error(`${legalDocument.bundledName}: ${response.status}`);
  return response.text();
};

/** What a reporter is asked to include, per kind of report. */
const ISSUE_CHECKLIST: Record<IssueKind, readonly string[]> = {
  bug: [
    "What you expected to happen.",
    "What happened instead.",
    "The steps that reproduce it.",
    "The redacted diagnostics, which Open GitHub issue… fills in for you."
  ],
  feature: [
    "The problem you are trying to solve.",
    "The outcome you want instead."
  ]
};

const ISSUE_KIND_LABELS: readonly { kind: IssueKind; label: string }[] = [
  { kind: "bug", label: "Report a bug" },
  { kind: "feature", label: "Suggest an improvement" }
];

const DIAGNOSTICS_COPIED = "Redacted diagnostics copied. Paste them into the issue's diagnostics field.";
const PASTE_DIAGNOSTICS = "Press Copy diagnostics and paste them into the issue's diagnostics field.";
const DIAGNOSTICS_TOO_LONG = `The diagnostics are too long to fit in the issue link, so its diagnostics field was left empty. ${PASTE_DIAGNOSTICS}`;

function FeedbackContent() {
  // Help > Report an Issue… and Help > Send Feedback… each open this window on
  // their own kind, and switch it when it is already open; the toggle below
  // stays, so a reporter who chose the wrong entry need not reopen anything.
  const [issueKind, setIssueKind] = useState<IssueKind>(() => feedbackIssueKind(window.location.search));
  const [osVersion, setOsVersion] = useState<string>();
  const [message, setMessage] = useState<string>();
  // Which diagnostics request is waiting on the main window. Every button that
  // makes one stays disabled until it settles: a second Export press would
  // otherwise open a second save dialog behind the first.
  const [pending, setPending] = useState<"export" | "copy" | "issue">();
  const mounted = useRef(true);
  const mountGeneration = useRef(0);
  const kindOwner = useRef({ kind: issueKind, generation: 0 });
  const pendingFlight = useRef<object | undefined>(undefined);
  const changeIssueKind = (kind: IssueKind) => {
    if (kindOwner.current.kind !== kind) {
      kindOwner.current = { kind, generation: kindOwner.current.generation + 1 };
      setMessage(undefined);
    }
    setIssueKind(kind);
  };
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; mountGeneration.current += 1; };
  }, []);
  useEffect(() => onFeedbackIssueKind(changeIssueKind), []);
  useEffect(() => {
    let current = true;
    hostSystem.osVersion()
      .then((version) => { if (current && version) setOsVersion(version); })
      .catch(() => undefined);
    return () => { current = false; };
  }, []);
  const issueReport = { repositoryUrl: QC_BRAND.sourceRepositoryUrl, kind: issueKind, appVersion, osVersion, platform: issuePlatform };
  // The link as shown here, before the diagnostics are fetched for it.
  const issueUrl = buildIssueUrl(issueReport);

  // One path for every button: an action either names its own outcome by
  // resolving with it, or takes the caller's `success` line, and either way
  // the text lands in this window's own message line.
  const report = async (run: () => Promise<string | void>, current: () => boolean, success?: string) => {
    try {
      const outcome = await run();
      const detail = typeof outcome === "string" ? outcome : success;
      if (current() && detail) setMessage(detail);
    } catch (error) {
      if (current()) setMessage(error instanceof Error ? error.message : String(error));
    }
  };

  // The main window builds the report; this window writes it, because the
  // clipboard only accepts a write from the document that has focus.
  const copyDiagnostics = async (current: () => boolean) => {
    const text = await requestDiagnostics("copy");
    if (!current()) return;
    await navigator.clipboard.writeText(text);
  };

  // Kind changes invalidate work that has not reached an external write,
  // never an already-admitted save. Keep the single-flight lease until the
  // real request settles, even if its result no longer belongs to this panel.
  const startReport = (kind: "export" | "copy" | "issue", run: (current: () => boolean) => Promise<string | void>, waiting?: string, success?: string) => {
    if (!mounted.current || pendingFlight.current || kindOwner.current.kind !== issueKind) return;
    const token = {};
    pendingFlight.current = token;
    const mount = mountGeneration.current;
    const generation = kindOwner.current.generation;
    const current = () => mounted.current && mountGeneration.current === mount
      && kindOwner.current.kind === issueKind && kindOwner.current.generation === generation;
    setPending(kind);
    setMessage(waiting);
    void report(() => run(current), current, success).finally(() => {
      if (pendingFlight.current !== token) return;
      pendingFlight.current = undefined;
      if (mounted.current) setPending(undefined);
    });
  };

  const runDiagnostics = (kind: "export" | "copy") => {
    startReport(kind, kind === "export" ? () => requestDiagnostics("export") : copyDiagnostics,
      kind === "export" ? "Waiting for the save dialog…" : "Collecting redacted diagnostics…",
      kind === "copy" ? DIAGNOSTICS_COPIED : undefined);
  };

  // A bug report's diagnostics field is filled with the same redacted report
  // Copy diagnostics copies. When the main window cannot supply it, or it is
  // too long for the link, the issue still opens and the reporter is told to
  // paste it instead.
  const openIssue = async (current: () => boolean): Promise<string | void> => {
    let diagnostics: string | undefined;
    let unavailable: string | undefined;
    if (issueReport.kind === "bug") {
      try {
        diagnostics = await requestDiagnostics("copy");
      } catch (error) {
        unavailable = `The diagnostics could not be collected (${error instanceof Error ? error.message : String(error)}), so the issue's diagnostics field was left empty. ${PASTE_DIAGNOSTICS}`;
      }
    }
    if (!current()) return;
    const link = buildIssueLink({ ...issueReport, diagnostics });
    await externalLinks.open(link.url);
    if (link.diagnostics === "omitted") return DIAGNOSTICS_TOO_LONG;
    return unavailable;
  };

  const runOpenIssue = () => {
    startReport("issue", openIssue, issueReport.kind === "bug" ? "Collecting redacted diagnostics for the issue…" : undefined,
      "Opened the issue form in your browser.");
  };

  return <>
    <header className="help-window-header">
      <div className="dialog-kicker">{issueKind === "bug" ? "REPORT AN ISSUE" : "SEND FEEDBACK"}</div>
      <h2 id="help-window-title">{QC_BRAND.appName} <span>{appVersion}</span></h2>
    </header>
    <div className="help-window-scroll feedback-panel">
      <div className="feedback-kind">
        {ISSUE_KIND_LABELS.map((entry) => <button
          key={entry.kind}
          type="button"
          aria-pressed={issueKind === entry.kind}
          onClick={() => changeIssueKind(entry.kind)}
        >{entry.label}</button>)}
      </div>
      <h3>What to include</h3>
      <ul>{ISSUE_CHECKLIST[issueKind].map((item) => <li key={item}>{item}</li>)}</ul>
      <div className="dialog-actions">
        <button type="button" disabled={pending !== undefined} aria-busy={pending === "export"} onClick={() => runDiagnostics("export")}>Export redacted diagnostics…</button>
        <button type="button" disabled={pending !== undefined} aria-busy={pending === "copy"} onClick={() => runDiagnostics("copy")}>Copy diagnostics</button>
        <button type="button" className="primary" disabled={pending !== undefined} aria-busy={pending === "issue"} onClick={runOpenIssue}>Open GitHub issue…</button>
      </div>
      <p className="feedback-url">{issueUrl}</p>
      {issueKind === "bug" && <p>Open GitHub issue… fills in the version, the operating system, a short outline for what happened, and the same redacted diagnostics Copy diagnostics copies, which travel to GitHub in the link. Review them on the form before you submit.</p>}
      <p>The diagnostics contain app and runtime state and lifecycle event names while omitting serial numbers, MAC addresses, usernames, filesystem paths, and preset or setlist names.</p>
      {message && <p className="about-message" role="status">{message}</p>}
    </div>
  </>;
}

export function HelpWindow({ kind }: { kind: HelpWindowKind }) {
  return <main className={`help-window help-window-${kind}`} aria-labelledby="help-window-title">
    {kind === "guide" && <>
      <header className="help-window-header">
        <div className="dialog-kicker">USER GUIDE</div>
        <h2 id="help-window-title">{QC_BRAND.appName} <span>{appVersion}</span></h2>
      </header>
      <div className="help-window-scroll">
        <GuideContent />
      </div>
    </>}
    {kind === "feedback" && <FeedbackContent />}
    {kind === "about" && <AboutWindowContent
      appVersion={appVersion}
      openExternal={externalLinks.open}
      openHelpWindow={helpWindows.open}
      loadLegalDocument={loadLegalDocument}
    />}
  </main>;
}
