import type { AntigravityLocalIntegrationStatus, ChatGptLocalIntegrationStatus, ClaudeDesktopExtensionStatus, JanLocalIntegrationStatus, LmStudioLocalIntegrationStatus, LocalAccessStatus, LocalMcpTestResult, LocalRouteRefresh, ManagedJsonMcpIntegrationStatus, OtherMcpStdioSetup } from "./tauri-transport";

/**
 * What a row's colour means, and it means one thing each.
 *
 * `action-required` is the one that had drifted. It had come to mean "not
 * green", so a host that simply was not running a chat wore the same amber as
 * a route that had never been written - and the row then told the user to
 * restart an application that was working. Three facts make that
 * reading untenable rather than merely unkind:
 *
 * - ChatGPT and Codex spawn their MCP servers per chat session, and do not
 *   respawn one when the configuration changes. No bridge between chats is
 *   what healthy looks like.
 * - Every host's bridge is a separate process running this same executable, so
 *   any stop of QC Remote by image name takes all of them down at once, and a
 *   host does not retry a dead STDIO server.
 * - So after almost any restart of this app, every paired host is bridge-less
 *   through nobody's fault, and there is nothing for the user to do.
 *
 * The rule the whole file is now written to:
 *
 * - `ready` - a heartbeat from that host, right now.
 * - `neutral` - nothing is wrong and nothing is being asked. Not installed, or
 *   paired with a route this app is happy with and no bridge at this instant.
 * - `checking` - a read or an announcement is outstanding. Not a verdict.
 * - `available` - a client is installed and can be paired.
 * - `action-required` - a route that is missing, wrong or out of date, or a
 *   decision only the user can take. **Never the mere absence of a heartbeat.**
 * - `error` - a read failed, which is a fact about the read and not about the
 *   pairing.
 */
export type LocalStatusTone = "neutral" | "checking" | "available" | "action-required" | "error" | "ready";
/**
 * `idle` is paired, current, and not connected at this instant - the resting
 * state of a healthy host rather than a stage on the way to one. It is
 * separate from `pairing`, which says something is still being arranged, and
 * from `connected`, which requires a heartbeat.
 */
export type LocalProviderPhase = "checking" | "unavailable" | "available" | "pairing" | "idle" | "connected" | "attention";

/**
 * Every phase a row can wear, exhaustive where a compiler can check it.
 *
 * The `Record` is the whole point of the indirection: add a member to
 * `LocalProviderPhase` and this object stops compiling until the new phase is
 * named here, so the class helper below and the tests that enumerate it cannot
 * fall behind the type.
 */
const LOCAL_PROVIDER_PHASE_SET: Record<LocalProviderPhase, true> = { checking: true, unavailable: true, available: true, pairing: true, idle: true, connected: true, attention: true };
export const LOCAL_PROVIDER_PHASES = Object.keys(LOCAL_PROVIDER_PHASE_SET) as LocalProviderPhase[];

/**
 * The row's own class namespace, and why it is not `phase-`.
 *
 * A row used to be `local-provider-card phase-${phase}`, and `phase-*` is the
 * connection badge's and the context pill's vocabulary: a set of bare,
 * unscoped class rules declaring one colour per indicator state. Two of the
 * seven row phases happen to spell an indicator state, so an `available` row
 * took the badge's blue and an `idle` row its grey - and because the row's own
 * text colours are stated in custom properties the theme does not define, each
 * of them is invalid at computed-value time and falls back to the inherited
 * value, which carried that colour to every word and to the row's border. The
 * other five phases match no such rule, so five rows were painted one way and
 * two another by a sheet written for a different component.
 *
 * `local-phase-*` cannot collide, because nothing outside this panel's
 * namespace names it: a state added to the badge's vocabulary later reaches
 * nothing here. It is a rename rather than an override on the row on purpose -
 * one rule fighting an unrelated one is the fragility, not the fix.
 */
export const localRowPhaseClass = (phase: LocalProviderPhase): string => `local-phase-${phase}`;

/**
 * The row's whole class attribute, decided here instead of at the call site.
 *
 * A phase says which state a row is in and nothing about how it is painted:
 * the status dot carries the colour, it already has a tone per state, and the
 * row's text, border and ground are identical in all seven phases.
 *
 * Phase and tone are not the same axis, which is why only one of them may
 * carry colour. `attention` arrives with tone `action-required` for a route
 * that needs rewriting and with tone `error` for a status that could not be
 * read; `pairing` and `idle` likewise span two tones between them. A colour
 * keyed on the phase would contradict the dot beside it on one of each pair,
 * and in this panel the dot alone carries the status colour.
 *
 * The phase class stays on the row all the same: it is the state a test or a
 * later rule can name, in a namespace only this panel owns.
 */
export const localRowClassName = (phase: LocalProviderPhase): string => `local-provider-card ${localRowPhaseClass(phase)}`;

/**
 * What a row's primary button does, decided once per state.
 *
 * The panel used to decide it twice. A per-provider label function said the
 * word, and a ternary chain at each `<LocalProviderRow>` call site picked the
 * handler, so the two drifted apart in both directions: a connected row's
 * label went `undefined` while a separate `Open` button appeared in a third
 * slot that existed only while the provider was connected, and a row could say
 * `Update` while its handler merely raised a window. Naming the act here makes
 * the word and the handler one decision, and `localRowButtons` below is the
 * only place either is read.
 *
 * `reload` used to be here: `repair` for a provider that can be asked to
 * re-read its own MCP list. It was Antigravity's primary on a route that was
 * already current, which meant a healthy row wore Update and pressing it
 * changed nothing the user could see. Asking a host to re-read its MCP list is
 * QC Remote's own job and it does it on start, so the button went and nothing
 * replaced it: the reload is the same native call as Pair and Update, which
 * the row already offers whenever the route needs it.
 *
 * `configure` opens a provider's own MCP configuration file. It is never a
 * primary: it is the second act on a row whose file could not be read, beside
 * Refresh, because the fix is an edit only the user can make.
 */
export type LocalPrimaryAct = "open" | "repair" | "pair" | "install" | "refresh" | "copy" | "configure";
export type LocalPrimaryAction = { act: LocalPrimaryAct; label: string };

/**
 * An act, or the considered absence of one.
 *
 * `none` is what a state declares when nothing this app can do would carry out
 * what a label would promise. It is not the panel being unsure - that is
 * `checking` before a status arrives and `refresh` after a read fails - and it
 * is not a quieter way of saying a repair is awkward. It says the repair does
 * not exist, so the row offers no primary and lets the acts that do exist,
 * Unpair among them, stand on their own.
 *
 * Every provider can declare it, because what decides it is who owns the thing
 * that would have to be rewritten, not which app is in the row: a managed
 * configuration file is this app's to rewrite at any version, and a client's
 * own extension registry is the client's.
 */
export type LocalPrimaryChoice = LocalPrimaryAct | "none";

/**
 * The one place a primary act becomes a word.
 *
 * Open if all is fine, Update if not, and no button where neither can be kept.
 * Every state QC Remote can act on reaches one of the first two, so the user is
 * never asked to tell a version mismatch from a rewritten configuration file -
 * both are one act through one handler. The remaining words name an act that is
 * not a repair at all: pairing for the first time, installing the desktop app,
 * re-reading a status that could not be read, and copying the STDIO setup.
 *
 * The label is a promise about behaviour. Wherever this returns `Open`, the
 * row's primary must launch that provider's app; wherever it returns `Update`,
 * it must run that provider's own repair. Both then re-read the provider's
 * status so the row settles without a second press. Where no act can keep such
 * a promise this returns nothing at all, which is the only answer that cannot
 * be pressed and then fail to happen.
 *
 * `install` is the only act whose word names the app, because "Install" alone
 * does not say what.
 */
export function localPrimaryAction(act: LocalPrimaryAct, installLabel?: string): LocalPrimaryAction;
export function localPrimaryAction(act: LocalPrimaryChoice, installLabel?: string): LocalPrimaryAction | undefined;
export function localPrimaryAction(act: LocalPrimaryChoice, installLabel = "Install"): LocalPrimaryAction | undefined {
  if (act === "none") return undefined;
  return {
    act,
    label: act === "open"
      ? "Open"
      : act === "repair"
        ? "Update"
        : act === "pair"
          ? "Pair"
          : act === "refresh"
            ? "Refresh"
            : act === "copy"
              ? "Copy STDIO setup"
              : act === "configure"
                ? "Open config"
                : installLabel
  };
}

export type LocalProviderPresentation = {
  phase: LocalProviderPhase;
  status: string;
  tone: LocalStatusTone;
  detail: string;
  canUnpair: boolean;
  /**
   * Absent for two reasons and no third: the provider's first status read is
   * still outstanding, or the state declared `none` because no act this app
   * has would do what a label said. A row of the second kind still offers
   * every act it really can perform, Unpair included.
   */
  primary?: LocalPrimaryAction;
  /**
   * A second act beside the primary, for the one state that needs two: a
   * configuration file that could not be read, where Refresh re-reads it and
   * Open config is how the user fixes it. Every other state leaves it absent.
   */
  secondary?: LocalPrimaryAction;
};

export type LocalRowAct = LocalPrimaryAct | "unpair";
/** One handler per act the row can reach. A row omits the acts it cannot perform. */
export type LocalProviderHandlers = Partial<Record<LocalRowAct, () => void>>;
export type LocalRowButton = { act: LocalRowAct; label: string; tone: "plain" | "primary" | "danger"; run: () => void };

/**
 * Every button a row shows, in the two positions it always uses.
 *
 * A paired row is primary-then-Unpair and nothing else. `Open` used to live in
 * a third slot that appeared only once a provider connected, so the controls
 * moved under the pointer as the row went green, and Claude's Unpair wore
 * neutral styling while the other five wore red. One list, one order, one tone
 * per role.
 *
 * A state that declares no primary act comes out of here as Unpair alone,
 * assembled by the same two pushes as every other row. No call site has to
 * know which kind of row it is drawing, and none may decide it: a conditional
 * at a call site is how a button came to say one thing and do another.
 */
export const localRowButtons = (presentation: LocalProviderPresentation, handlers: LocalProviderHandlers): LocalRowButton[] => {
  const buttons: LocalRowButton[] = [];
  const primary = presentation.primary;
  const run = primary ? handlers[primary.act] : undefined;
  if (primary && run) buttons.push({
    act: primary.act,
    label: primary.label,
    tone: primary.act === "pair" || primary.act === "copy" ? "primary" : "plain",
    run
  });
  const secondary = presentation.secondary;
  const runSecondary = secondary ? handlers[secondary.act] : undefined;
  if (secondary && runSecondary) buttons.push({ act: secondary.act, label: secondary.label, tone: "plain", run: runSecondary });
  if (presentation.canUnpair && handlers.unpair) buttons.push({ act: "unpair", label: "Unpair", tone: "danger", run: handlers.unpair });
  return buttons;
};

/**
 * The LOCAL badge is an availability indicator, not a progress meter.
 *
 * Keep these phases explicit and shared by the menu bar and its details
 * panel: grey means no supported desktop client, blue means one is available
 * to pair or paired and resting, green means one is connected, amber means a
 * route is missing, wrong or out of date, and red means a status check failed.
 *
 * Amber used to mean "not green", which is how the badge came to go amber a
 * minute after every successful pairing: every `paired && !connected` term was
 * counted as trouble. It now carries exactly the conditions that paint a row
 * amber, and absence is not one of them.
 */
export type LocalIntegrationSummary = {
  phase: "idle" | "available" | "syncing" | "ready" | "action-required" | "error";
  status: string;
};

export type LocalIntegrationSummaryOptions = {
  claudeInstallPending?: boolean;
  /**
   * ChatGPT's two restart flags used to be here and are deliberately gone.
   * They said the configuration had been rewritten under a running client,
   * which - given that ChatGPT spawns its MCP servers per chat and does not
   * respawn one on a configuration change - is not a condition anybody has to
   * act on. Counting them as trouble is what turned the badge amber minutes
   * after every successful pairing. The rows still read them, to say which
   * connection the next chat will start.
   */
  refreshFailed?: boolean;
  lmStudio?: LmStudioLocalIntegrationStatus;
  jan?: JanLocalIntegrationStatus;
  other?: OtherMcpStdioSetup;
};

const availableStatus = (version?: string) => version ? `Available · ${version}` : "Available";

/**
 * The fine row, shared by every provider: one sentence saying the provider can
 * drive the hardware now, beside the one button that puts it in front of you.
 *
 * It used to be the only row with nothing to press - the primary label went
 * `undefined` on a connection - and the three providers that did say something
 * each said it differently, two of them telling the user to open an app that
 * the row itself now opens.
 */
const connectedPresentation = (app: string, connectedSuffix = ""): LocalProviderPresentation => ({
  phase: "connected", status: "Connected", tone: "ready",
  detail: `${app} can control your Quad Cortex now.${connectedSuffix} Open brings it to the front.`,
  canUnpair: true, primary: localPrimaryAction("open")
});

/**
 * Liveness as the managed providers report it. `connectionActive` is the only
 * field that licenses the word "Connected": a configuration file of the right
 * version proves nothing about whether the desktop app is running, and the
 * panel claimed a green connection at the exact moment Claude Desktop was shut
 * and its MCP startup failed. An absent field is "not observed", so a native
 * payload that predates these two degrades to the honest state, never the
 * flattering one.
 *
 * `bridgeAttached` now survives a restart of this app, which changes what its
 * absence means. It used to be forgotten along with every heartbeat, so
 * `false` said only "not since this process started" - and the panel read six
 * healthy hosts as six that had never connected. Durable, `false` means the
 * host has genuinely never started this app's MCP server, and `true` means it
 * has, whatever it is doing at this instant. Neither value is evidence that
 * anything is wrong; only `connectionActive` says a bridge is talking now.
 */
type ObservedLiveness = { connectionActive?: boolean; bridgeAttached?: boolean; connectionPending?: boolean }
  & Pick<ManagedJsonMcpIntegrationStatus, "skillState" | "skillStatusError">;

/**
 * A paired row whose liveness is not yet established.
 *
 * QC Remote forgets every heartbeat when it restarts, by design - a heartbeat
 * is a claim about now - so for one announcement interval after the app starts
 * it has heard from nobody. Reading that silence as "never connected" would
 * mean restarting six hosts whose bridges were running the whole time.
 *
 * So this row claims nothing in either direction. It does not say Connected,
 * which only an observed heartbeat licenses, and it does not offer Update,
 * because nothing is known to be broken - the same reason a failed status read
 * offers the read again rather than a repair.
 */
const checkingLivenessPresentation = (app: string, canUnpair: boolean): LocalProviderPresentation => ({
  phase: "checking", status: "Checking…", tone: "checking",
  detail: `Paired. Waiting for ${app} to report its connection. Refresh reads it again now.`,
  canUnpair, primary: localPrimaryAction("refresh")
});

/** True while a healthy bridge may simply not have announced itself yet. */
const livenessPending = (liveness: ObservedLiveness) =>
  liveness.connectionActive !== true && liveness.connectionPending === true;

/**
 * A paired row whose route this app is happy with, in whatever liveness it is.
 *
 * Both unconnected rows are `neutral` and both offer `Open`, and those are the
 * two corrections this function exists for.
 *
 * They were amber and offered `Update`. Amber was false - an absent bridge is
 * the resting state of these hosts, not a fault - and `Update` was worse than
 * false: the route was already correct and current, so rewriting it changed
 * nothing the user could see - "clicking Update does nothing but display more
 * messages." `Open` is the act that helps, because every one of these hosts
 * opens the connection itself once it is running and something needs it.
 *
 * `reconnects` is how this particular host does that, said in its own terms,
 * because the four of them differ and the difference is the content of the
 * sentence.
 */
const pairedPresentation = (
  app: string,
  liveness: ObservedLiveness,
  connectedSuffix = "",
  reconnects = `${app} opens the connection when you start a chat`
): LocalProviderPresentation => {
  if (liveness.connectionActive) return connectedPresentation(app, connectedSuffix);
  // A healthy inactive MCP route stays inactive, but an independent failed
  // skill read is still known evidence and must not disappear with liveness.
  const unverifiedSkill = unreadableSkillDetail(liveness);
  // Ahead of both unconnected rows, because each of them makes a claim this
  // one is the absence of: that the bridge has been seen, or that it never
  // has. Inside the window neither is known.
  if (livenessPending(liveness)) {
    const checking = checkingLivenessPresentation(app, true);
    return { ...checking, detail: checking.detail + unverifiedSkill };
  }
  if (liveness.bridgeAttached) return {
    phase: "idle", status: "Paired · idle", tone: "neutral",
    detail: `Paired and working — ${reconnects}. Open starts ${app} now.${unverifiedSkill}`,
    canUnpair: true, primary: localPrimaryAction("open")
  };
  // `bridgeAttached` is durable, so this row now says what its words say: this
  // host has never once started QC Remote's server. That is also the ordinary
  // state of a pairing made a minute ago, which is why it is neutral and
  // forward looking rather than an accusation with a repair attached.
  return {
    phase: "idle", status: "Paired · not yet connected", tone: "neutral",
    detail: `Paired, not connected yet — ${reconnects}. Open starts ${app} now.${unverifiedSkill}`,
    canUnpair: true, primary: localPrimaryAction("open")
  };
};

/**
 * The paired route needs rewriting, and the version is only sometimes why.
 *
 * A route this app wrote at an older version is an ordinary upgrade, and
 * naming both numbers says exactly what Update will do. A route at *this*
 * version can need the same rewrite for a different reason: the identity a
 * heartbeat is credited to travels as a launcher argument as well as in `env`,
 * and a pairing made before that argument existed carries only the environment
 * copy - which a host may keep to itself while running the server perfectly
 * well. Printing one version number twice said nothing there, so the sentence
 * names the act instead of the arithmetic.
 */
const pairedRouteUpdateDetail = (
  status: { installedVersion?: string; availableVersion: string }
): string => status.installedVersion && status.installedVersion !== status.availableVersion
  ? `QC Remote ${status.installedVersion} is paired; ${status.availableVersion} is available. Update installs it.`
  : "QC Remote is paired. Update rewrites the connection so QC Remote can tell when this client is connected.";

/** The most of the host's own words a row will carry. */
const ROUTE_REFRESH_REASON_LIMIT = 120;

/**
 * The reported reason, kept as a quotation and never as this app's own words.
 *
 * `error` is whatever the host filesystem said, arriving through a status
 * document QC Remote does not author. So it is collapsed to one line, stripped
 * of the marks that would close the quotation early, bounded, and placed last
 * inside one - after this app has finished saying what it tried and what the
 * button does. Nothing in it can then be read as an instruction from QC
 * Remote, and no single pathological string can take the row over.
 */
const boundedRouteRefreshReason = (error?: string | null): string => {
  const reported = (error ?? "").replace(/["\u201c\u201d]/g, "").replace(/\s+/g, " ").trim();
  return reported.length > ROUTE_REFRESH_REASON_LIMIT
    ? `${reported.slice(0, ROUTE_REFRESH_REASON_LIMIT)}…`
    : reported;
};

/**
 * The one thing a row says about the start-up route refresh, and the one
 * case in which it says it.
 *
 * QC Remote brings its own paired routes up to date when it starts, unasked,
 * and every managed provider's status carries what that pass decided. Six of
 * the seven outcomes are silent here, each for its own reason:
 *
 * - `rewritten` - the app did its job. A row that announces its own
 *   successful housekeeping is noise, and this is the outcome the silence is
 *   really about.
 * - `current`, `not-paired` - nothing happened.
 * - `paired-with-newer-build` - already has a row of its own, which reports
 *   the version relationship rather than the pass.
 * - `customised` - not an error. The entry has no valid managed identity or
 *   launches a different executable, so the pass could not establish that it
 *   is this app's and left it exactly as it was, which is the right thing to
 *   have done. If it is also out of date the row reads `Update available` on
 *   its own merits, and that one honest signal is enough; a second sentence
 *   saying the same thing would make a deliberate choice look like a fault.
 * - `unreadable` - a judgement rather than a reflex, and it goes three ways.
 *   The read that produces it is the same read the status document is built
 *   from, so a configuration this app still cannot parse is already a row in
 *   `error` tone offering Refresh - the remedy an unreadable file actually
 *   has, and saying it twice is the duplication `customised` is spared. The
 *   outcome is recorded once at start-up while the status is re-read live, so
 *   an `unreadable` that has since healed would pin a claim that is no longer
 *   true. And it is no evidence that the route is this app's or that it is
 *   stale, because the pass never got far enough to know - so neither the
 *   amber nor the word Update would be earned.
 *
 * `failed` is the one that shows, because it is the one where this app tried
 * and could not: the route is its own, needed repair (an older build, or a
 * difference from what this build writes), and the rewrite was refused. The
 * staleness is real, nothing else will clear it, and what
 * remains is the user pressing the control that performs the same write. So
 * it is a route that is out of date plus a decision that has landed back with
 * the user - `action-required` by the rule at the top of this file, and not
 * by the absence of a heartbeat.
 *
 * Read only where the state already says the route is out of date, which is
 * exactly what a refused rewrite leaves behind. That is not narrowing for its
 * own sake: the pass records its outcome once per app start and never again,
 * so a row that read it in any other state would go on reporting a failure
 * that a later press of Update had already repaired.
 */
const routeRefreshFailure = (refresh?: LocalRouteRefresh | null): LocalProviderPresentation | undefined => {
  if (refresh?.outcome !== "failed") return undefined;
  const reason = boundedRouteRefreshReason(refresh.error);
  // Both numbers come off the same status document the row is built from, and
  // the pass sets `pairedVersion` only where it read one, so the sentence
  // cannot name a version nothing is actually carrying.
  const versions = refresh.pairedVersion && refresh.pairedVersion !== refresh.availableVersion
    ? ` from ${refresh.pairedVersion} to ${refresh.availableVersion}`
    : "";
  return {
    phase: "attention", status: "Update needed", tone: "action-required",
    detail: `QC Remote tried to update this connection${versions} when it started and could not. Update rewrites it now.${reason ? ` Reported: "${reason}"` : ""}`,
    canUnpair: true, primary: localPrimaryAction("repair")
  };
};

/**
 * The pairing already installed is newer than the one this build can write.
 *
 * The only thing QC Remote can install is its own bundled copy, and over a
 * newer one that is a downgrade. Whether that downgrade is even possible
 * depends on who owns what would have to be rewritten, which is why the
 * resolution is a parameter here rather than a constant: a managed
 * configuration file is this app's, and it writes the entry at whatever
 * version it is, so Update repairs the row; a client's own extension registry
 * belongs to the client.
 *
 * Claude's does, and Claude will not take an older bundle over a newer one -
 * pressing Update on a row reporting a newer extension opened Claude's
 * extension dialog offering Uninstall alone, and QC Remote carries no
 * self-updater to close the gap the other way. That button could not do what
 * its label said in principle rather than by accident, so that state declares
 * `none` and names the resolution the user can carry out instead.
 *
 * Either way the version relationship - not liveness - is what the row has to
 * report, and Unpair stays: removing a newer pairing is still a real act, and
 * for Claude it opens the very page the removal lives on.
 */
const newerInstalledPresentation = (
  app: string,
  noun: string,
  status: { installedVersion?: string; availableVersion: string },
  /** What the user can actually do about it, and the sentence that says so. */
  resolution: { act: LocalPrimaryChoice; sentence: string }
): LocalProviderPresentation => ({
  phase: "pairing", status: `Paired · newer ${noun}`, tone: "action-required",
  // Both numbers come off the status the native layer just reported, so the
  // row cannot name a version this build is not actually carrying.
  detail: `${status.installedVersion
    ? `${app} has QC Remote ${status.installedVersion}, newer than this app's ${status.availableVersion}.`
    : `${app} has a newer QC Remote ${noun} than this app's ${status.availableVersion}.`} ${resolution.sentence}`,
  canUnpair: true, primary: localPrimaryAction(resolution.act)
});

/**
 * The desktop app is absent. Whether anything is still paired decides the row.
 *
 * Both halves are one field, and getting either half wrong has the same shape:
 * a row reading "Not installed" while offering Unpair contradicts itself, and a
 * row hiding Unpair while this app's own entry is still written leaves a
 * pairing with nothing that can remove it. So every provider whose
 * configuration file this app owns reads that field here instead of answering
 * it for itself - ChatGPT's row and Antigravity's row were answering it the
 * second way, and the entry outliving the app is the ordinary case rather than
 * a corner one: all three files live in the Windows profile, not in the app's
 * install directory, so uninstalling the client leaves the pairing behind.
 *
 * There is no repair while the app is gone, so the primary offers the app.
 * Unpair is a real act either way, because removing the entry is a plain write
 * to a file this app owns and does not consult the client to perform.
 *
 * Claude is deliberately not here. Its extension lives in Claude's own
 * registry and the only control that removes it is a page inside Claude
 * Desktop, so with Claude uninstalled there is no act to offer at all - the
 * same ownership argument `newerInstalledPresentation` records above.
 */
const appAbsentPresentation = (
  app: string,
  installLabel: string,
  integrationInstalled: boolean
): LocalProviderPresentation => integrationInstalled ? {
  phase: "attention", status: "Connection left over", tone: "action-required",
  detail: `${app} is not installed, but a QC Remote connection is still configured for it. Unpair to remove it, or install ${app} to use it.`,
  canUnpair: true, primary: localPrimaryAction("install", installLabel)
} : {
  phase: "unavailable", status: "Not installed", tone: "neutral",
  detail: `Install ${app} to control QC Remote with AI`,
  canUnpair: false, primary: localPrimaryAction("install", installLabel)
};

export const claudeExtensionPresentation = (
  status?: ClaudeDesktopExtensionStatus,
  installPending = false,
  refreshFailed = false,
  approvalLapsed = false
): LocalProviderPresentation => {
  if (!status) return refreshFailed ? {
    phase: "attention", status: "Status unavailable", tone: "error",
    detail: "Could not check Claude Desktop. Refresh to try again.",
    canUnpair: false, primary: localPrimaryAction("refresh")
  } : {
    // Nothing is known yet, so there is nothing to offer. This is the only
    // state a row shows no button at all.
    phase: "checking", status: "Checking…", tone: "checking",
    detail: "Checking Claude Desktop.", canUnpair: false
  };
  if (!status.claudeInstalled) return {
    phase: "unavailable", status: "Not installed", tone: "neutral",
    detail: "Install Claude Desktop to control QC Remote with AI",
    canUnpair: false, primary: localPrimaryAction("install", "Install Claude Desktop")
  };
  // Pressing Update on an out-of-date pairing sets this flag too, so the flag
  // says a new bundle is waiting for Claude's approval - never that the
  // registry is empty. The extension the row would remove is the one already
  // registered, which is why Unpair follows `extensionInstalled` rather than
  // the flag.
  if (installPending) return {
    phase: "pairing", status: "Approval needed", tone: "action-required",
    detail: "Claude is holding the QC Remote extension for your approval. Update opens that approval again.",
    canUnpair: status.extensionInstalled, primary: localPrimaryAction("repair")
  };
  // A status *read* that failed says nothing about the pairing, so the row
  // offers the read again. Offering a repair here rebuilt a connection that was
  // never known to be broken.
  if (refreshFailed || status.statusError) return {
    phase: "attention", status: "Needs attention", tone: "error",
    detail: "QC Remote could not read Claude's extension status. Refresh reads it again.",
    canUnpair: status.extensionInstalled, primary: localPrimaryAction("refresh")
  };
  // Liveness and the version relationship are separate axes, so an observed
  // connection still reads as connected. Only the state that used to borrow
  // "Paired · not yet connected" - which describes liveness and says nothing
  // about versions - names the situation instead.
  if (status.state === "newer-installed" && !status.connectionActive && !status.bridgeAttached) {
    // Claude owns its extension registry and refuses an older bundle over a
    // newer one, so no act of this app's installs anything here. Unpair opens
    // Claude's extension page, where removing it is the one control that
    // exists - which is why the sentence names Unpair by the word on the
    // button beside it.
    return newerInstalledPresentation("Claude Desktop", "extension", status, {
      act: "none",
      sentence: "Unpair removes it, and pairing again then installs the copy this app carries."
    });
  }
  if (status.state === "current" || status.state === "newer-installed") {
    return pairedPresentation("Claude Desktop", status, " QC Device Knowledge is included.");
  }
  // The pending flag clears itself after two minutes. Saying nothing put the
  // row back on "Available", which reads as though nothing had been attempted.
  //
  // An approval lapses over whatever was already registered, and the flag is
  // only cleared by a pairing Claude did confirm - so pressing Update on an
  // out-of-date extension and leaving Claude's dialog alone parks the row here
  // with the old extension still installed and, until now, no way to remove
  // it. The retry word is read off the same function the button reads it off,
  // because a sentence naming Pair beside a button saying Update is how the
  // two drifted apart before.
  if (approvalLapsed) {
    const retry: LocalPrimaryAct = status.extensionInstalled ? "repair" : "pair";
    return {
      phase: "attention", status: "Approval not confirmed", tone: "action-required",
      detail: `Claude did not confirm the QC Remote extension. ${localPrimaryAction(retry).label} asks Claude for that approval again.`,
      canUnpair: status.extensionInstalled, primary: localPrimaryAction(retry)
    };
  }
  if (status.state === "update-available") return {
    phase: "attention", status: "Update available", tone: "action-required",
    detail: `QC Remote ${status.installedVersion ?? "extension"} is paired; ${status.availableVersion} is available. Update installs it.`,
    canUnpair: true, primary: localPrimaryAction("repair")
  };
  if (status.state === "different-version") return {
    phase: "attention", status: "Needs attention", tone: "action-required",
    detail: "The Claude pairing was written by a different QC Remote. Update rewrites it for this one.",
    canUnpair: true, primary: localPrimaryAction("repair")
  };
  return {
    phase: "available", status: availableStatus(status.appVersion), tone: "available",
    detail: "Pair with Claude Desktop to control QC Remote with AI",
    canUnpair: false, primary: localPrimaryAction("pair")
  };
};

export const chatgptIntegrationPresentation = (
  status?: ChatGptLocalIntegrationStatus,
  restartRequired = false,
  refreshFailed = false,
  removalRestartRequired = false
): LocalProviderPresentation => {
  if (!status) return refreshFailed ? {
    phase: "attention", status: "Status unavailable", tone: "error",
    detail: "Could not check ChatGPT or Codex. Refresh to try again.",
    canUnpair: false, primary: localPrimaryAction("refresh")
  } : {
    phase: "checking", status: "Checking…", tone: "checking",
    detail: "Checking ChatGPT and Codex.", canUnpair: false
  };
  const hostName = status.clientName ?? "ChatGPT";
  const hostAppName = hostName === "Codex CLI" ? "Codex CLI" : "ChatGPT Desktop";
  // The QC Remote entry lives in the shared `config.toml` under the Windows
  // profile, which outlives any uninstall of ChatGPT Desktop, so this row had
  // to stop reporting a missing app as a missing pairing.
  if (!status.chatgptInstalled) return appAbsentPresentation(hostAppName, `Install ${hostAppName}`, status.integrationInstalled);
  // `status-error` is the native layer reporting that it could not read or
  // parse config.toml, which is a failed status read like any other, so
  // Refresh stays the primary. The file is also one only the user can repair,
  // and the Local menu that used to carry the route to it is gone, so the row
  // offers Open config beside Refresh. A read that merely failed to arrive
  // (`refreshFailed`) has no file to open and keeps Refresh alone. Unpair is
  // withheld while the file cannot be parsed: removing the entry means
  // rewriting that same file, so it could only fail, and the row keeps to its
  // two buttons.
  if (refreshFailed || status.state === "status-error") return {
    phase: "attention", status: "Needs attention", tone: "error",
    detail: status.state === "status-error"
      ? "QC Remote could not read the shared MCP configuration. Open config to fix config.toml, then Refresh reads it again."
      : "QC Remote could not verify the local pairing. Refresh to try again.",
    canUnpair: status.state === "status-error" && !refreshFailed ? false : status.integrationInstalled,
    primary: localPrimaryAction("refresh"),
    ...(status.state === "status-error" && !refreshFailed ? { secondary: localPrimaryAction("configure") } : {})
  };
  // The entry is gone from `config.toml` and a chat ChatGPT already has open
  // keeps its copy of the server until that chat ends. Nothing is wrong and
  // nothing is owed, so this is the ordinary unpaired row with the one sentence
  // that explains the lag - it used to say "Restart needed" in amber and hand
  // the user a path through ChatGPT's settings to do the app's own job.
  if (removalRestartRequired && !status.integrationInstalled) return {
    phase: "available", status: "Unpaired", tone: "available",
    detail: "QC Remote's connection is removed. A chat already running in ChatGPT keeps it until that chat ends. Pair adds it back.",
    canUnpair: false, primary: localPrimaryAction("pair")
  };
  if (status.state === "newer-installed" && !status.connectionActive && !status.bridgeAttached) {
    // This app writes the QC Remote entry in the shared config.toml itself,
    // and that write is not conditional on the version already there, so the
    // repair really does land - unlike Claude's, which Claude has to accept.
    return newerInstalledPresentation(hostName, "connection", status, {
      act: "repair",
      sentence: "Update replaces it with this app's own and keeps every other MCP server."
    });
  }
  // `restartRequired` says the configuration was rewritten while ChatGPT was
  // running. It used to produce an amber row headed "Restart needed" telling
  // the user to find a reload control inside ChatGPT. ChatGPT and
  // Codex spawn their MCP servers per chat session and do not respawn one when
  // the configuration changes, so there is nothing to restart and nothing for
  // the user to do - the next chat picks the new route up. All the flag is good
  // for is saying *which* connection the next chat will start, which is the
  // confirmation the row owed after a press of Update.
  if (status.state === "current" || status.state === "newer-installed") {
    const reconnects = hostName === "Codex CLI"
      ? (restartRequired || status.restartRequired
        ? `${hostName} starts the rewritten connection in its next session`
        : `${hostName} starts the connection in each new session`)
      : (restartRequired || status.restartRequired
        ? "ChatGPT starts the rewritten connection in your next chat"
        : "ChatGPT starts the connection in each new chat");
    return pairedPresentation(
      hostName,
      status,
      installedSkillDetail(status),
      reconnects
    );
  }
  // A rewrite this app could not make is the one thing the start-up refresh
  // says out loud, and it is said here because this is the state a refused
  // rewrite leaves behind: the older route still on disk. It comes ahead of
  // the skill sentences because a skill one release behind is routine and a
  // write that was refused is not, and one row may only say one thing.
  if (status.state === "update-available") return routeRefreshFailure(status.routeRefresh) ?? {
    phase: "attention", status: "Update available", tone: "action-required",
    detail: status.skillState === "not-installed"
      ? `Install QC Device Knowledge ${status.availableSkillVersion} alongside the existing local connection.`
      : status.skillState === "update-available"
        ? `Update QC Device Knowledge to ${status.availableSkillVersion}; the existing copy will be backed up.`
        : pairedRouteUpdateDetail(status),
    canUnpair: true, primary: localPrimaryAction("repair")
  };
  if (status.state === "different-configuration") return {
    phase: "attention", status: "Needs attention", tone: "action-required",
    detail: status.skillState === "modified" || status.skillState === "different-configuration"
      ? "Update backs up the existing QC Device Knowledge skill before installing the app-managed copy. Other skills remain unchanged."
      : "Update replaces only the QC Remote connection and keeps every other MCP server.",
    canUnpair: true, primary: localPrimaryAction("repair")
  };
  return {
    phase: "available", status: availableStatus(status.appVersion), tone: "available",
    detail: "Pair with ChatGPT / Codex to control QC Remote with AI",
    canUnpair: false, primary: localPrimaryAction("pair")
  };
};

/** A healthy MCP route is independent of an unreadable skill directory. */
const unreadableSkillDetail = (status: Pick<ManagedJsonMcpIntegrationStatus, "skillState" | "skillStatusError">): string =>
  status.skillState === "status-error" || status.skillStatusError
    ? " QC Device Knowledge status could not be read; its installation is unverified. Refresh checks it again."
    : "";

const installedSkillDetail = (status: Pick<ManagedJsonMcpIntegrationStatus, "skillState" | "skillStatusError" | "installedSkillVersion" | "availableSkillVersion">): string =>
  unreadableSkillDetail(status) || ` QC Device Knowledge ${status.installedSkillVersion ?? status.availableSkillVersion} is installed.`;

export const antigravityIntegrationPresentation = (
  status?: AntigravityLocalIntegrationStatus,
  refreshFailed = false
): LocalProviderPresentation => {
  if (!status) return refreshFailed ? {
    phase: "attention", status: "Status unavailable", tone: "error",
    detail: "Could not check Google Antigravity. Refresh to try again.",
    canUnpair: false, primary: localPrimaryAction("refresh")
  } : {
    phase: "checking", status: "Checking…", tone: "checking",
    detail: "Checking Google Antigravity.", canUnpair: false
  };
  // Antigravity's route lives in its MCP configuration under the Windows
  // profile, on the same terms as ChatGPT's and the two managed clients'.
  if (!status.antigravityInstalled) return appAbsentPresentation("Google Antigravity", "Install Antigravity", status.integrationInstalled);
  if (refreshFailed || status.state === "status-error") return {
    phase: "attention", status: "Needs attention", tone: "error",
    detail: status.state === "status-error"
      ? "QC Remote could not read Antigravity's MCP configuration. Refresh reads it again."
      : "QC Remote could not verify Antigravity. Refresh to try again.",
    canUnpair: status.integrationInstalled, primary: localPrimaryAction("refresh")
  };
  // Three rows collapse into the shared paired one, and "Ready to connect" -
  // amber, offering Update, on a route that was already current - goes with
  // them. Antigravity can be asked to re-read its MCP list, but that is QC
  // Remote's own job and it now does it on start; a button asking the user to
  // trigger it was the app delegating its work, and pressing it on a healthy
  // row changed nothing visible. While that reattach is in flight the payload
  // says `connectionPending`, which the shared row already renders as
  // Checking… rather than as a second spelling of the same wait.
  if (status.state === "current") {
    return pairedPresentation(
      "Google Antigravity",
      status,
      installedSkillDetail(status),
      "Google Antigravity opens the connection when it starts"
    );
  }
  // The same precedence as ChatGPT's row, for the same reason.
  if (status.state === "update-available") return routeRefreshFailure(status.routeRefresh) ?? {
    phase: "attention", status: "Update available", tone: "action-required",
    detail: status.skillState === "not-installed"
      ? `Install QC Device Knowledge ${status.availableSkillVersion} alongside the existing local connection.`
      : status.skillState === "update-available"
        ? `Update QC Device Knowledge to ${status.availableSkillVersion}; the existing copy will be backed up.`
        : pairedRouteUpdateDetail(status),
    canUnpair: true, primary: localPrimaryAction("repair")
  };
  if (status.state === "different-configuration") return {
    phase: "attention", status: "Needs attention", tone: "action-required",
    detail: status.skillState === "modified" || status.skillState === "different-configuration"
      ? "Update backs up the existing QC Device Knowledge skill before installing the app-managed copy. Other skills remain unchanged."
      : "Update restores the QC Remote connection and keeps every other MCP server.",
    canUnpair: true, primary: localPrimaryAction("repair")
  };
  return {
    phase: "available", status: availableStatus(status.appVersion), tone: "available",
    detail: "Pair with Google Antigravity to control QC Remote with AI",
    canUnpair: false, primary: localPrimaryAction("pair")
  };
};

/**
 * The name LM Studio's row shows, and the rule that picks it.
 *
 * LM Studio renamed its desktop application to **Bionic**. The Start Menu
 * shortcut is `Bionic.lnk`, it points at
 * `%LOCALAPPDATA%\Programs\Bionic\Bionic.exe`, the version resource's
 * `ProductName` and `FileDescription` both read `Bionic`, the running window
 * title begins `Bionic - `, and the uninstaller is `Uninstall Bionic.exe`.
 * `LM Studio` survives in that resource only as `CompanyName`. So Bionic is
 * the application and LM Studio is the company, and a row headed "LM Studio"
 * names something the owner no longer sees anywhere.
 *
 * The label **follows the installed application** rather than being fixed at
 * Bionic, because a machine still carrying a pre-rename build genuinely has an
 * application called LM Studio. Fixed at Bionic, that owner's row would read
 * "Bionic 0.3.16 is too old to speak local MCP" - naming an app that is not on
 * the machine while quoting a version that is, which is the same shape of
 * unfalsifiable accusation the version-series table was introduced to stop.
 *
 * The rule costs nothing extra to decide, because the status document already
 * decided it. `appVersionSeries` is the numbering line the update verdict was
 * reached in, and those lines are named after their products: `Bionic` for
 * 1.x, `LM Studio` for the 0.3.x line below it. Reading the name off that
 * field makes the label and the verdict one reading instead of two that can
 * disagree.
 *
 * Everything else is Bionic, deliberately:
 *
 * - nothing installed - the row's button is `Install Bionic`, and Bionic is
 *   what https://lmstudio.ai/download ships today;
 * - a version too strange to parse, which belongs to no series;
 * - a status that has not arrived yet, where there is nothing to read.
 *
 * **This is a label, and only a label.** The `lm-studio:` client id prefix
 * underneath it is an identity: it is written into every route's
 * `--local-mcp-client` argument and its `env`, and it is the key of the
 * durable attach-history file that stops the panel calling a working host a
 * stranger. It does not move with the name. `client_packages.rs` pins that
 * separation with a test of its own.
 */
export const BIONIC_APP_NAME = "Bionic";

/** The pre-rename application, which is also what its numbering line is called. */
export const LEGACY_LM_STUDIO_APP_NAME = "LM Studio";

export const lmStudioAppName = (
  status?: Pick<ManagedJsonMcpIntegrationStatus, "appVersionSeries">
): string => status?.appVersionSeries === LEGACY_LM_STUDIO_APP_NAME
  ? LEGACY_LM_STUDIO_APP_NAME
  : BIONIC_APP_NAME;

/**
 * The MCP enable switch this installation still leaves for the owner to flip,
 * where it has one.
 *
 * Pre-rename LM Studio treated a freshly written route as off until the owner
 * turned it on in the host's own UI, so every sentence this app wrote about a
 * new pairing had to name that switch. Bionic's document carries the flag per
 * server and QC Remote writes `enabled: true`, so a Bionic pairing is complete
 * with nothing left to do - and naming a switch there would send the owner
 * looking for one.
 *
 * Read off `appVersionSeries`, which is the field `lmStudioAppName` reads and
 * the same reading `client_packages.rs` selects the document family by. The
 * application the row names, the document the pairing is written to and the
 * switch the row mentions are therefore one decision, and cannot disagree
 * about which product is installed.
 */
export const LM_STUDIO_ENABLE_SWITCH = "the Program tab";

export const lmStudioEnableSwitch = (
  status?: Pick<ManagedJsonMcpIntegrationStatus, "appVersionSeries">
): string | undefined => status?.appVersionSeries === LEGACY_LM_STUDIO_APP_NAME
  ? LM_STUDIO_ENABLE_SWITCH
  : undefined;

export const managedMcpIntegrationPresentation = (
  name: string,
  status: ManagedJsonMcpIntegrationStatus | undefined,
  installed: boolean,
  settingsHint: string | undefined,
  refreshFailed = false
): LocalProviderPresentation => {
  if (!status) return refreshFailed ? {
    phase: "attention", status: "Status unavailable", tone: "error",
    detail: `Could not check ${name}. Refresh to try again.`,
    canUnpair: false, primary: localPrimaryAction("refresh")
  } : {
    phase: "checking", status: "Checking…", tone: "checking",
    detail: `Checking ${name}.`, canUnpair: false
  };
  // This is where the leftover entry was first named, and it is now the shared
  // answer for every provider whose configuration file this app owns.
  if (!installed) return appAbsentPresentation(name, `Install ${name}`, status.integrationInstalled);
  // The repair QC Remote has cannot make an old desktop client speak local MCP,
  // so this row names the act that can: updating the client itself.
  if (status.state === "app-update-required") return {
    phase: "attention", status: "App update required", tone: "action-required",
    detail: `${name} ${status.appVersion ?? "is installed"} is too old to speak local MCP. Update ${name} opens its download page.`,
    canUnpair: status.integrationInstalled,
    primary: localPrimaryAction("install", `Update ${name}`)
  };
  if (refreshFailed || status.state === "status-error") return {
    phase: "attention", status: "Needs attention", tone: "error",
    detail: status.state === "status-error"
      ? `QC Remote could not read ${name}'s MCP configuration. Refresh reads it again.`
      : `Could not verify ${name}. Refresh to try again.`,
    canUnpair: status.integrationInstalled, primary: localPrimaryAction("refresh")
  };
  // The same collapse as Antigravity's, and the same reason: the route is
  // current, so the Update this row offered rewrote a file that was already
  // right. What a host like this needs instead is for something to want the
  // server, which is a fact about it rather than a fault of the pairing - so
  // it is said in the calm row.
  //
  // A host whose route this app can only write disabled needs one more thing:
  // its own switch turned on. The caller names that switch where its
  // installation still has one, and names nothing where the written route is
  // already enabled - see `lmStudioEnableSwitch`. Neither sentence claims when
  // the host reads the file, because that is not known for every host.
  // A host on this shared row may or may not have a skills mechanism, and the
  // three sentences below have to say nothing at all for one that does not.
  // `skillState` is the test: the native side sets it only for a host this app
  // can install the skill into, so Bionic's row grows the same sentences
  // ChatGPT's and Antigravity's carry while Jan's is untouched. Saying "not
  // installed" about a thing Jan cannot install would report a fault that is
  // neither the user's nor fixable, and Jan is not short of the knowledge
  // either way - it reads qc://device-knowledge off the bridge like every
  // paired host.
  const skill = status.skillState;
  if (status.state === "current") {
    return pairedPresentation(name, status, skill
      ? installedSkillDetail(status)
      : "", settingsHint
      ? `${name} opens the connection once QC Remote is enabled in ${settingsHint}`
      : `${name} picks the connection up on its own schedule`);
  }
  // The same precedence as ChatGPT's row, for the same reason.
  if (status.state === "update-available") return routeRefreshFailure(status.routeRefresh) ?? {
    phase: "attention", status: "Update available", tone: "action-required",
    detail: skill === "not-installed"
      ? `Install QC Device Knowledge ${status.availableSkillVersion} alongside the existing local connection.`
      : skill === "update-available"
        ? `Update QC Device Knowledge to ${status.availableSkillVersion}; the existing copy will be backed up.`
        : pairedRouteUpdateDetail(status),
    canUnpair: true, primary: localPrimaryAction("repair")
  };
  if (status.state === "different-configuration") return {
    phase: "attention", status: "Needs attention", tone: "action-required",
    detail: skill === "modified" || skill === "different-configuration"
      ? "Update backs up the existing QC Device Knowledge skill before installing the app-managed copy. Other skills remain unchanged."
      : "Update replaces only the QC Remote connection and keeps every other MCP server.",
    canUnpair: true, primary: localPrimaryAction("repair")
  };
  return {
    phase: "available", status: availableStatus(status.appVersion), tone: "available",
    detail: `Pair with ${name} to control QC Remote with AI`,
    canUnpair: false, primary: localPrimaryAction("pair")
  };
};

/**
 * The STDIO row names no app, so it can neither open one nor repair one.
 *
 * Its one act is handing the user a configuration to paste, in both states.
 */
export const otherMcpPresentation = (status?: OtherMcpStdioSetup, refreshFailed = false): LocalProviderPresentation => {
  if (!status) return refreshFailed ? {
    phase: "attention", status: "Status unavailable", tone: "error",
    // The row's one button is the copy, so the sentence names the copy. It
    // named Refresh, which is a control this row has never carried.
    detail: "QC Remote could not build the standard STDIO setup. Copy STDIO setup tries again.",
    canUnpair: false, primary: localPrimaryAction("copy")
  } : {
    phase: "checking", status: "Checking…", tone: "checking",
    detail: "Preparing a standard STDIO MCP setup. Copy STDIO setup hands it over once it is ready.",
    canUnpair: false, primary: localPrimaryAction("copy")
  };
  if (status.connectionActive) return {
    phase: "connected", status: "Connected", tone: "ready",
    detail: "A desktop MCP app can control your Quad Cortex now. Copy STDIO setup hands the same connection to another app.",
    canUnpair: false, primary: localPrimaryAction("copy")
  };
  // This row names no app, so its one act stays the same in every state; what
  // changes is that it stops claiming the absence of a connection it has not
  // yet had time to observe.
  if (livenessPending(status)) return {
    phase: "checking", status: "Checking…", tone: "checking",
    detail: "Waiting for a desktop MCP app to report its connection. Copy STDIO setup hands the same connection to another app.",
    canUnpair: false, primary: localPrimaryAction("copy")
  };
  // An app that has used this setup before and is not running it now is in the
  // same resting state as a paired row's, and gets the same calm reading. The
  // row used to fall back to "STDIO available", which is true of the setup and
  // says nothing about the app that has been using it.
  if (status.bridgeAttached) return {
    phase: "idle", status: "STDIO idle", tone: "neutral",
    detail: "A desktop MCP app has used this setup and opens it again when it starts. Copy STDIO setup hands the same connection to another app.",
    canUnpair: false, primary: localPrimaryAction("copy")
  };
  return {
    phase: "available", status: "STDIO available", tone: "available",
    detail: "Copy STDIO setup hands you a standard connection for another desktop MCP app.",
    canUnpair: false, primary: localPrimaryAction("copy")
  };
};

export type LocalActionOutcome = {
  tone: "pending" | "ready" | "action-required" | "error";
  /** One sentence, in the user's words, ending in the next thing to do. */
  message: string;
  /** The raw error, kept for a Copy details affordance rather than printed. */
  detail?: string;
};

/**
 * Name the control the user can see, or name none at all.
 *
 * These sentences used to end in "try Pair again" by default, and the rows
 * that produce them most often are paired rows - whose two buttons are their
 * primary and Unpair, neither of which says Pair. So a failed Update told the
 * user to press a button that was not on the row they were looking at, which
 * is the same defect as telling them to restart a host: copy standing in for
 * the app doing its job, and in this case an instruction that cannot be
 * followed.
 *
 * The retry word is therefore the caller's, and every caller passes the label
 * of the button that was pressed - which is on the row by construction. Where
 * a caller has no such label, the sentence says "try again" and names nothing.
 */
const retryPhrase = (retry: string | undefined, sentenceStart: boolean): string => {
  const phrase = retry ? `press ${retry} again` : "try again";
  return sentenceStart ? `${phrase[0].toUpperCase()}${phrase.slice(1)}` : phrase;
};

/**
 * Turn an OS-level failure into a sentence with a next action.
 *
 * Every LOCAL handler used to funnel its `catch` into the status strip at the
 * bottom of the window, which printed strings written for a log:
 * "Could not back up C:\\Users\\...: Access is denied. (os error 5)". The path
 * and the errno are worth keeping for a support report; neither is the thing
 * to put in front of someone who wants to pair an app.
 */
export const localActionFailure = (app: string, error: unknown, retry?: string): LocalActionOutcome => {
  const detail = error instanceof Error ? error.message : String(error);
  if (/os error (?:5|32)\b|access is denied|used by another process/i.test(detail)) return {
    tone: "error",
    message: `Another program is using ${app}'s configuration. Close ${app}, then ${retryPhrase(retry, false)}.`,
    detail
  };
  if (/os error (?:2|3)\b|cannot find the (?:file|path)|no such file/i.test(detail)) return {
    tone: "error",
    message: `${app} has not created its configuration folder yet. Start ${app} once, then ${retryPhrase(retry, false)}.`,
    detail
  };
  return {
    tone: "error",
    message: `QC Remote could not finish with ${app}. ${retryPhrase(retry, true)}, then copy the details if it keeps failing.`,
    detail
  };
};

/**
 * Report which half of a pairing landed.
 *
 * The native result has carried `skillInstalled` and `skillBackupPath` all
 * along and the panel read neither, so a pairing that installed the device
 * knowledge skill but not the MCP connection - or that quietly moved someone's
 * own skill aside - was announced as an unqualified success.
 */
export const localPairOutcome = (app: string, result: {
  installed?: boolean;
  skillInstalled?: boolean;
  skillBackupPath?: string;
  /**
   * Why the skill did not install, from the native result's `skillError`.
   *
   * The native side has reported this for every failure mode since the field
   * existed - a staging write that failed, a backup that could not be created,
   * an activation rename that failed, a rollback that also failed and names
   * where the user's files still are - and nothing here read it, so every one
   * of them arrived as the same "Try again to add it". Some of them cannot be
   * retried into success: a link in the skill folder needs moving aside first,
   * and a failed rollback needs the user to know a backup exists at all.
   */
  skillError?: string;
  nextStep?: string;
}, retry?: string): LocalActionOutcome => {
  const backup = result.skillBackupPath
    ? ` Your existing QC Device Knowledge skill was backed up to ${result.skillBackupPath}.`
    : "";
  if (result.installed === false) return result.skillInstalled
    ? {
      tone: "action-required",
      message: `QC Device Knowledge is installed, but the QC Remote connection was not added to ${app}. ${retryPhrase(retry, true)}.${backup}`
    }
    : {
      tone: "error",
      message: `QC Remote was not paired with ${app}. ${retryPhrase(retry, true)}, then copy the details if it keeps failing.`
    };
  // The reason first, then the action - and the action always, whether or not
  // a reason came with it. Dropping it whenever `skillError` was set left the
  // common case worse than before: only two of the native side's failures say
  // what to do next on their own, and the other eleven are a bare cause, so
  // most of them ended on a dead end. The split that holds is that the native
  // side reports what happened and this decides what to offer, because only
  // this side knows whether the button says Pair or Update.
  if (result.skillInstalled === false) return {
    tone: "action-required",
    // The backup sentence only when no reason came with it. A backup is taken
    // at exactly one point - immediately before the existing skill is removed -
    // so the only failure that can report one is the rename-and-rollback, and
    // its own message already ends by naming where the backup still is.
    // Appending the generic sentence as well stated the same directory twice,
    // in the longest message this panel can produce, inside an element that
    // ellipses.
    //
    // (That sentence used to quote the message verbatim, and the quoted form
    // contains an angle-bracketed word the repository asset audit reads as SVG
    // markup - which made this file fail as an undeclared vector owner.)
    message: `QC Remote is paired with ${app}, but QC Device Knowledge was not installed.${result.skillError ? ` ${result.skillError}` : backup} ${retryPhrase(retry, true)} to add it.`
  };
  return {
    tone: "ready",
    message: `${result.nextStep ?? `QC Remote is paired with ${app}.`}${backup}`
  };
};

/**
 * Report one Test connection in the row that was tested.
 *
 * Until now the only verification this feature offered anywhere was prose
 * telling the user to type `/mcp` in their AI app and see what happened. The
 * states below are separated by remedy, because the remedy is the only part a
 * user can act on: turning local access back on, restarting QC Remote, closing
 * a second copy of it, and plugging the Quad Cortex in are four different acts.
 * The raw diagnosis stays behind Copy details; none of it belongs in the
 * sentence.
 */
export const localMcpTestOutcome = (app: string, result: LocalMcpTestResult): LocalActionOutcome => {
  const { detail } = result;
  switch (result.state) {
    case "ready":
      return {
        tone: "ready",
        message: `QC Remote ${result.appVersion} answered and your Quad Cortex is connected${result.presetName ? ` on ${result.presetName}` : ""}. Ask ${app} about your QC.`,
        detail
      };
    case "deviceNotConnected":
      return {
        tone: "action-required",
        // Naming the version is what separates "the app is fine, the guitar
        // rig is not" from "nothing answered at all".
        message: `QC Remote ${result.appVersion} answered, but no Quad Cortex is connected. Plug the Quad Cortex into this PC over USB and wait for it to finish starting, then test again.`,
        detail
      };
    case "servedByAnotherCopy":
      return {
        tone: "action-required",
        message: `Another copy of QC Remote is answering ${app} on this PC. Close the other copy, then reopen this one and test again.`,
        detail
      };
    case "unauthorized":
      return {
        tone: "error",
        message: `QC Remote refused its own local connection key, so the details ${app} holds are out of date. Restart QC Remote, then test again.`,
        detail
      };
    case "notPublished":
      return {
        tone: "error",
        message: `QC Remote is not offering a local connection for ${app} to use. Restart QC Remote, then test again.`,
        detail
      };
    case "disabled":
      return {
        tone: "action-required",
        message: `Local AI access is switched off, so ${app} cannot reach your Quad Cortex. Turn local AI access back on, then test again.`,
        detail
      };
    default:
      return {
        tone: "error",
        message: `QC Remote is not answering ${app} on its local connection. Restart QC Remote, then test again.`,
        detail
      };
  }
};

/**
 * What is true of the loopback surface itself, whatever the provider rows say.
 *
 * A copy of QC Remote that stood down for a second copy shows six provider
 * rows describing a listener no client is dialling, so the condition has to be
 * reported somewhere. It is a property of the app rather than of any pairing,
 * and as a banner over the rows it was one more block of prose in the panel a
 * user opens to pair an app. Settings carries it now, beside the other
 * app-wide AI control.
 */
export const localAccessNotice = (status?: LocalAccessStatus): LocalActionOutcome | undefined => {
  if (!status) return undefined;
  if (status.servedByAnotherInstance) return {
    tone: "action-required",
    message: "Another copy of QC Remote is serving the AI apps on this PC. Close the other copy, then reopen this one to serve them from here.",
    detail: status.lastError
  };
  if (!status.enabled) return {
    tone: "action-required",
    message: "Local AI access is turned off, so paired AI apps cannot reach your Quad Cortex. Turn local AI access back on to use the connections below.",
    detail: status.lastError
  };
  if (!status.running) return {
    tone: "error",
    message: "QC Remote is not offering a local connection right now, so the connections below cannot be used. Restart QC Remote to publish it again.",
    detail: status.lastError
  };
  return undefined;
};

export const localIntegrationSummary = (
  claude?: ClaudeDesktopExtensionStatus,
  chatgpt?: ChatGptLocalIntegrationStatus,
  antigravity?: AntigravityLocalIntegrationStatus,
  options: LocalIntegrationSummaryOptions = {}
): LocalIntegrationSummary => {
  const {
    claudeInstallPending = false,
    refreshFailed = false,
    lmStudio,
    jan,
    other
  } = options;

  if (refreshFailed) return { phase: "error", status: "Check failed" };
  if (!claude || !chatgpt || !antigravity || ("lmStudio" in options && !lmStudio) || ("jan" in options && !jan) || ("other" in options && !other)) return { phase: "syncing", status: "Checking…" };

  // Every one of these six counts an *observed* connection. The badge used to
  // count Claude as connected on the strength of a file on disk.
  const claudePaired = claude.claudeInstalled
    && (claude.state === "current" || claude.state === "newer-installed");
  const claudeConnected = claudePaired && claude.connectionActive === true;
  const chatgptPaired = (chatgpt.state === "current" || chatgpt.state === "newer-installed")
    && chatgpt.chatgptInstalled;
  // A heartbeat is a heartbeat. This used to be withheld whenever either
  // restart flag was set, so a client that was demonstrably talking to QC
  // Remote was counted as not connected because its configuration had been
  // rewritten at some point while it ran.
  const chatgptConnected = chatgptPaired && chatgpt.connectionActive;
  const antigravityPaired = antigravity.antigravityInstalled && antigravity.state === "current";
  const antigravityConnected = antigravityPaired && antigravity.connectionActive;
  const connected = Number(claudeConnected) + Number(chatgptConnected) + Number(antigravityConnected)
    + Number(Boolean(lmStudio?.connectionActive)) + Number(Boolean(jan?.connectionActive)) + Number(Boolean(other?.connectionActive));
  if (connected > 0) return { phase: "ready", status: `${connected} connected` };

  // A provider that has not had time to announce itself is not a provider in
  // trouble. Without this the badge went amber for a whole announcement
  // interval after every app start, on a machine where nothing was wrong.
  const pending = (claudePaired && livenessPending(claude))
    || (chatgptPaired && livenessPending(chatgpt))
    || (antigravity.state === "current" && livenessPending(antigravity))
    || [lmStudio, jan].some((provider) => provider?.state === "current" && livenessPending(provider))
    || Boolean(other && livenessPending(other));

  const claudeError = Boolean(claude.statusError);
  const chatgptError = chatgpt.state === "status-error";
  const antigravityError = antigravity.state === "status-error";
  const managedError = lmStudio?.state === "status-error" || jan?.state === "status-error";
  if (claudeError || chatgptError || antigravityError || managedError) return { phase: "error", status: "Check failed" };
  // After the errors, because a failed read is a fact and a pending
  // announcement is the absence of one.
  if (pending) return { phase: "syncing", status: "Checking…" };

  // Exactly the conditions that paint a row amber, and nothing else.
  //
  // Every `paired && !connected` term used to be in here, so the badge went
  // amber for six healthy hosts one announcement interval after the app
  // started, and again every time a host finished a chat. The badge is now
  // the same judgement the rows make: a route that is missing, wrong or out of
  // date, or a decision only the user can take.
  const leftover = (installed: boolean, integrationInstalled: boolean) => !installed && integrationInstalled;
  const claudeNeedsAttention = claudeInstallPending
    || claude.state === "different-version"
    || claude.state === "update-available"
    || (claude.state === "newer-installed" && !claude.connectionActive && !claude.bridgeAttached);
  const chatgptNeedsAttention = chatgpt.state === "different-configuration"
    || chatgpt.state === "update-available"
    || (chatgpt.state === "newer-installed" && !chatgpt.connectionActive && !chatgpt.bridgeAttached)
    || leftover(chatgpt.chatgptInstalled, chatgpt.integrationInstalled);
  const antigravityNeedsAttention = antigravity.state === "different-configuration"
    || antigravity.state === "update-available"
    || leftover(antigravity.antigravityInstalled, antigravity.integrationInstalled);
  const managedNeedsAttention = [lmStudio, jan].some((provider) => Boolean(provider) && (
    provider!.state === "app-update-required"
    || provider!.state === "different-configuration"
    || provider!.state === "update-available"
    || (provider!.state === "app-not-installed" && provider!.integrationInstalled)
  ));
  if (claudeNeedsAttention || chatgptNeedsAttention || antigravityNeedsAttention || managedNeedsAttention) {
    return { phase: "action-required", status: "Action needed" };
  }

  // Paired and resting is its own answer. Without it a set of healthy pairings
  // with nobody in a chat read "Available", which is what the badge says when
  // nothing has ever been paired at all.
  const paired = claudePaired || chatgptPaired || antigravityPaired
    || lmStudio?.state === "current" || jan?.state === "current" || Boolean(other?.bridgeAttached);
  if (paired) return { phase: "available", status: "Paired" };

  const installed = Number(claude.claudeInstalled) + Number(chatgpt.chatgptInstalled) + Number(antigravity.antigravityInstalled)
    + Number(Boolean(lmStudio?.lmStudioInstalled)) + Number(Boolean(jan?.janInstalled));
  if (installed === 0 && !other) return { phase: "idle", status: "Not set up" };
  return { phase: "available", status: "Available" };
};
