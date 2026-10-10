/**
 * The data behind the three Help windows: User Guide, Feedback, and About.
 *
 * It lives beside the components rather than inside them because Node's
 * `--experimental-strip-types` runner cannot load a `.tsx` module
 * (ERR_UNKNOWN_FILE_EXTENSION), and the window kinds, the About window's tab
 * order, the legal document table, and the issue-URL builder are exactly the
 * parts a test has to execute rather than read.
 */

import { QC_LEGAL_DOCUMENTS, type QcLegalDocument } from "@qc-remote/theme";

/** The three Help windows, as the Help menu names them to the Rust side. */
export type HelpWindowKind = "guide" | "feedback" | "about";

export const HELP_WINDOW_KINDS: readonly HelpWindowKind[] = ["guide", "feedback", "about"];

/** Each Help window's title, the same one the Rust shell gives its native window. */
export const HELP_WINDOW_TITLES: Readonly<Record<HelpWindowKind, string>> = {
  guide: "QC Remote — User Guide",
  feedback: "QC Remote — Feedback",
  about: "QC Remote — About"
};

/**
 * Which Help window this webview is, from its `?window=` query parameter.
 *
 * The main window loads `index.html` with no query at all and gets null, so it
 * renders the controller exactly as it always has.
 */
export function helpWindowKind(search: string): HelpWindowKind | null {
  const kind = new URLSearchParams(search).get("window");
  return HELP_WINDOW_KINDS.find((entry) => entry === kind) ?? null;
}

/** The About window's tabs. The guide and feedback are windows of their own. */
export type AboutTab = "privacy" | "legal" | "notices" | "about";

export const ABOUT_TABS: readonly { id: AboutTab; label: string }[] = [
  { id: "privacy", label: "Privacy" },
  { id: "legal", label: "Legal" },
  { id: "notices", label: "Notices" },
  { id: "about", label: "About" }
];

/**
 * A legal document the build ships.
 *
 * `bundledName` is the name the shared Vite legal-assets plugin copies into
 * `dist/legal/`, which is where the About window fetches it from. `repositoryPath`
 * is where the same document lives in the source tree, which is what the
 * "Open on GitHub" fallback points at when a development session has no
 * `dist/legal/` to read. The list itself is qc-theme's QC_LEGAL_DOCUMENTS, the
 * one list the plugin and the Android notices panel read too.
 */
export type LegalDocument = QcLegalDocument;

export const LEGAL_DOCUMENTS: readonly LegalDocument[] = QC_LEGAL_DOCUMENTS;

export type IssueKind = "bug" | "feature";

/**
 * The repository URL with any trailing slash removed.
 *
 * `sourceRepositoryUrl` is a brand-contract value an editor may well write with
 * a trailing slash, and every destination below is built by appending a path to
 * it. Without this, one keystroke in `brand.json` turns each of them into
 * `…/QC-Remote//issues/new`.
 */
export function repositoryRoot(repositoryUrl: string): string {
  return repositoryUrl.replace(/\/+$/, "");
}

export const ISSUE_KINDS: readonly IssueKind[] = ["bug", "feature"];

/**
 * Which kind of report a Feedback window starts on, from its `?issue=` query.
 *
 * Help > Report an Issue… opens it on a bug report and Help > Send Feedback…
 * on a feature request; the Rust shell adds the parameter. A window opened
 * without one (the About window's button, say) starts on a bug report.
 */
export function feedbackIssueKind(search: string): IssueKind {
  const issue = new URLSearchParams(search).get("issue");
  return ISSUE_KINDS.find((entry) => entry === issue) ?? "bug";
}

/**
 * The longest issue link the Feedback window opens, in characters.
 *
 * GitHub's front end refuses a request line much past 8 KB, and the Rust
 * command that opens the link refuses anything over its own 8192-character
 * ceiling (`MAX_URL_LEN` in `external_links.rs`). 6000 leaves room under both
 * for the rest of the request. A link that would be longer loses its
 * diagnostics rather than being cut, since cut JSON is not a report anyone can
 * read; the reporter is told to use Copy diagnostics instead.
 */
export const ISSUE_URL_MAX_LENGTH = 6000;

/** What a GitHub issue link is built from. */
export type IssueReport = {
  repositoryUrl: string;
  kind: IssueKind;
  appVersion: string;
  /**
   * The operating system's name and version as the Rust shell reads it, such
   * as "Windows 11 Pro 24H2 (build 26100.2033)". Absent when it could not be
   * read.
   */
  osVersion?: string;
  /**
   * What the webview reports for the platform ("Windows"), used only when
   * `osVersion` is absent.
   */
  platform?: string;
  /** The redacted diagnostics report, as Copy diagnostics copies it. */
  diagnostics?: string;
};

/**
 * The starting text of a bug report's "What happened?" field: three prompts
 * with room to write under each, then the environment for whoever reads it.
 * The App version and Platform fields carry the same facts as structured
 * fields; this is the prose a maintainer reads in the issue body.
 */
export function bugReportBody(input: Pick<IssueReport, "appVersion" | "osVersion" | "platform">): string {
  return [
    "What happened:",
    "",
    "",
    "What I expected instead:",
    "",
    "",
    "Steps to reproduce:",
    "1.",
    "",
    "---",
    "Environment",
    `App version: Windows ${input.appVersion}`,
    `OS: ${input.osVersion ?? input.platform ?? "not reported"}`
  ].join("\n");
}

/**
 * The GitHub issue-form URL the Feedback window opens, with every field given.
 *
 * GitHub prefills an issue form from query parameters named after the `id:` of
 * each field in `.github/ISSUE_TEMPLATE/*.yml`, plus its own `title`, so
 * `app-version`, `platform`, `behavior` and `diagnostics` here are field names
 * rather than free-form metadata. A URL cannot attach a file, so the
 * diagnostics travel as the diagnostics field's own text.
 *
 * A feature request's two fields, the problem and the suggested behavior, are
 * the reporter's own words; nothing QC Remote knows belongs in either, so it
 * carries only the template.
 *
 * The Platform field is required on the form, and a reporter who finds it empty
 * fills it in; one prefilled with a wrong answer is worse than one left blank,
 * so when neither the OS version nor the platform is known the parameter is
 * omitted entirely.
 *
 * This builds the URL whatever its length. `buildIssueLink` is what the
 * Feedback window opens, and keeps it under `ISSUE_URL_MAX_LENGTH`.
 */
export function buildIssueUrl(input: IssueReport): string {
  const parameters = new URLSearchParams({ template: input.kind === "bug" ? "bug-report.yml" : "feature-request.yml" });
  if (input.kind === "bug") {
    parameters.set("title", `[Bug] Windows ${input.appVersion}: `);
    parameters.set("app-version", `Windows ${input.appVersion}`);
    const platform = input.osVersion ?? input.platform;
    if (platform) parameters.set("platform", platform);
    parameters.set("behavior", bugReportBody(input));
    if (input.diagnostics) parameters.set("diagnostics", input.diagnostics);
  }
  return `${repositoryRoot(input.repositoryUrl)}/issues/new?${parameters.toString()}`;
}

/**
 * The link the Feedback window opens, and whether the diagnostics made it in.
 *
 * The report arrives pretty-printed. If that makes the link too long, the same
 * JSON on one line is tried; if that is still too long, the diagnostics field
 * is left empty (`diagnostics: "omitted"`) and the Feedback window tells the
 * reporter to paste them from Copy diagnostics. The report is never cut short.
 */
export function buildIssueLink(input: IssueReport): { url: string; diagnostics: "included" | "omitted" | "none" } {
  if (input.kind !== "bug" || !input.diagnostics) {
    return { url: buildIssueUrl({ ...input, diagnostics: undefined }), diagnostics: "none" };
  }
  const candidates = [input.diagnostics];
  try {
    const compact = JSON.stringify(JSON.parse(input.diagnostics));
    if (compact !== input.diagnostics) candidates.push(compact);
  } catch {
    // Not JSON: there is no shorter form of it that says the same thing.
  }
  for (const diagnostics of candidates) {
    const url = buildIssueUrl({ ...input, diagnostics });
    if (url.length <= ISSUE_URL_MAX_LENGTH) return { url, diagnostics: "included" };
  }
  return { url: buildIssueUrl({ ...input, diagnostics: undefined }), diagnostics: "omitted" };
}
