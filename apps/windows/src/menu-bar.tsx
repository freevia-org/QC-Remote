import { RemoteAccountDeletionLink } from "@qc-remote/ui";
import { useCallback, useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type RefObject } from "react";
import type { ConnectionState, RuntimeStatus } from "@qc-remote/client";
import type { ControlAccessMode, PublicRelayConnector, PublicRelayStatus } from "@qc-remote/core";
import { ControlAccessSelect, QcLocalAppLogo, QcUiIcon, qcConnectionIndicatorPhase, qcRelayPresentation, type QcIndicatorPhase } from "@qc-remote/ui";
import { qcDeviceStatusDetail } from "./qc-readiness";
import { completeUnpairIfSuccessful } from "./local-unpair";
import type { AntigravityLocalIntegrationStatus, ChatGptLocalIntegrationStatus, ClaudeDesktopExtensionStatus, JanLocalIntegrationStatus, LmStudioLocalIntegrationStatus, OtherMcpStdioSetup } from "./tauri-transport";
import { antigravityIntegrationPresentation, chatgptIntegrationPresentation, claudeExtensionPresentation, lmStudioAppName, lmStudioEnableSwitch, localIntegrationSummary, localRowButtons, localRowClassName, managedMcpIntegrationPresentation, otherMcpPresentation, type LocalIntegrationSummary, type LocalProviderHandlers, type LocalProviderPresentation } from "./local-integration-presentation";

type LocalProviderName = "claude" | "chatgpt" | "antigravity" | "lm-studio" | "jan";

export type ConnectionEvent = { at: string; event: string; result: "pending" | "success" | "warning" | "failure" | "info"; detail: string };
export type MenuCommand =
  | "open-workspace" | "save-workspace" | "save-workspace-as" | "exit"
  | "undo" | "redo" | "copy-preset" | "paste-preset" | "copy-preset-text" | "paste-preset-text" | "copy-setlist" | "paste-setlist" | "copy-block-settings" | "paste-block-settings" | "settings"
  | "view-fit" | "view-actual" | "toggle-fullscreen"
  | "open-preset-directory" | "save-preset-to-device" | "rename-current-preset" | "add-block" | "edit-routing" | "discard-changes"
  | "previous-preset" | "next-preset" | "set-tempo" | `select-mode-${0 | 1 | 2}` | "open-tuner" | "open-gig-view" | "open-io-settings"
  | "export-preset-library" | "device-backup"
  | "connect" | "disconnect" | "reconnect" | "reset-session" | "refresh-state" | "device-info"
  | "user-guide" | "export-diagnostics" | "report-bug" | "send-feedback" | "about";
export type MenuItem = { id: MenuCommand; label: string; disabled?: boolean; checked?: boolean; shortcut?: string } | { separator: true };
export type AppMenu = { name: string; items: MenuItem[] };

export const divider = (): MenuItem => ({ separator: true });

export function MenuBar({ menus, onSelect, connection, deviceReady, syncProgress, busy, localIntegrationAction, controlAccessMode, onControlAccessModeChange, claudeApprovalLapsed, localStatusChecking, claudeStatusRefreshFailed, chatgptStatusRefreshFailed, antigravityStatusRefreshFailed, lmStudioStatusRefreshFailed, janStatusRefreshFailed, otherMcpStatusRefreshFailed, runtime, deviceName, physicalDevices, selectedDeviceId, onSelectDevice, presetLabel, events, relayStatus, relayPending, relayPairingStarting, relayPairingError, relayPairingQrCode, relayPairingUrl, claudeExtensionStatus, claudeInstallPending, chatgptIntegrationStatus, chatgptRestartRequired, chatgptRemovalRestartRequired, antigravityIntegrationStatus, lmStudioIntegrationStatus, janIntegrationStatus, otherMcpStatus, relayConnectors, relayConnectorsError, relayUnlinkingGrantId, relayConnectorAddress, onRelayPanelOpen, onRelayPairingOpen, onOpenRelayConnector, onShareRelayConnector, onRelayReconnect, onRelayPairingRefresh, onRelayPairingShare, onRelayUnpair, onRetryRelayWithdrawal, onRelayDeleteAccount, onRelayUnlinkConnector, onInstallClaude, onOpenClaude, onOpenClaudeExtensions, onOpenClaudeDownload, onInstallChatgpt, onRemoveChatgpt, onOpenChatgpt, onOpenChatgptDownload, onOpenChatgptConfiguration, onOpenAntigravityDownload, onInstallAntigravity, onRemoveAntigravity, onOpenAntigravity, onInstallLmStudio, onRemoveLmStudio, onOpenLmStudio, onOpenLmStudioDownload, onInstallJan, onRemoveJan, onOpenJan, onOpenJanDownload, onCopyOtherMcpSetup, onRefreshLocalIntegrations, onLocalPanelChange, onConnect, onDisconnect, demoPending, onStartDemo, onStopDemo, deviceHeldInBackground, onResumeDevice, onClearEvents, onOpenChange }: {
  menus: AppMenu[];
  onSelect: (item: MenuCommand) => void;
  connection: ConnectionState;
  deviceReady: boolean;
  syncProgress: number | null;
  busy: boolean;
  localIntegrationAction: "claude" | "chatgpt" | "remove-chatgpt" | "antigravity" | "remove-antigravity" | "lm-studio" | "remove-lm-studio" | "jan" | "remove-jan" | "other" | null;
  controlAccessMode: ControlAccessMode;
  onControlAccessModeChange: (mode: ControlAccessMode) => void;
  claudeApprovalLapsed: boolean;
  localStatusChecking: boolean;
  claudeStatusRefreshFailed: boolean;
  chatgptStatusRefreshFailed: boolean;
  antigravityStatusRefreshFailed: boolean;
  lmStudioStatusRefreshFailed: boolean;
  janStatusRefreshFailed: boolean;
  otherMcpStatusRefreshFailed: boolean;
  runtime?: RuntimeStatus;
  deviceName: string;
  physicalDevices: readonly {id: string; name: string; connected?: boolean; phase?: string}[];
  selectedDeviceId?: string;
  onSelectDevice: (id: string) => void;
  presetLabel: string;
  events: ConnectionEvent[];
  relayStatus?: PublicRelayStatus;
  relayPending: boolean;
  relayPairingStarting: boolean;
  relayPairingError?: string;
  relayPairingQrCode?: string;
  relayPairingUrl?: string;
  claudeExtensionStatus?: ClaudeDesktopExtensionStatus;
  claudeInstallPending: boolean;
  chatgptIntegrationStatus?: ChatGptLocalIntegrationStatus;
  chatgptRestartRequired: boolean;
  chatgptRemovalRestartRequired: boolean;
  antigravityIntegrationStatus?: AntigravityLocalIntegrationStatus;
  lmStudioIntegrationStatus?: LmStudioLocalIntegrationStatus;
  janIntegrationStatus?: JanLocalIntegrationStatus;
  otherMcpStatus?: OtherMcpStdioSetup;
  relayConnectors: PublicRelayConnector[];
  relayConnectorsError?: string;
  relayUnlinkingGrantId?: string;
  onRelayPairingOpen: () => void;
  onOpenRelayConnector: () => void;
  relayConnectorAddress: string;
  onShareRelayConnector: () => void;
  onRelayReconnect: () => void;
  onRelayPanelOpen: (open: boolean) => void;
  onRelayPairingRefresh: () => void;
  onRelayPairingShare: () => void;
  onRelayUnpair: () => void;
  onRetryRelayWithdrawal: () => void;
  onRelayDeleteAccount: () => void;
  onRelayUnlinkConnector: (grantId: string) => void;
  onInstallClaude: () => void;
  onOpenClaude: () => void;
  /** Opens Claude's own extension page with QC Remote selected; false when it could not be opened. */
  onOpenClaudeExtensions: () => Promise<boolean>;
  onOpenClaudeDownload: () => void;
  onInstallChatgpt: () => void;
  onRemoveChatgpt: () => boolean | Promise<boolean>;
  onOpenChatgpt: () => void;
  onOpenChatgptDownload: () => void;
  /** Opens the shared config.toml, offered only when QC Remote could not read it. */
  onOpenChatgptConfiguration: () => void;
  onOpenAntigravityDownload: () => void;
  onInstallAntigravity: () => void;
  onRemoveAntigravity: () => boolean | Promise<boolean>;
  onOpenAntigravity: () => void;
  onInstallLmStudio: () => void;
  onRemoveLmStudio: () => boolean | Promise<boolean>;
  onOpenLmStudio: () => void;
  onOpenLmStudioDownload: () => void;
  onInstallJan: () => void;
  onRemoveJan: () => boolean | Promise<boolean>;
  onOpenJan: () => void;
  onOpenJanDownload: () => void;
  onCopyOtherMcpSetup: () => void;
  onRefreshLocalIntegrations: () => void;
  onLocalPanelChange: (open: boolean) => void;
  onConnect: () => void;
  onDisconnect: () => void;
  /** The demo device is being opened. */
  demoPending: boolean;
  /**
   * Explicitly select the Demo device, even while a real Quad Cortex is ready.
   * It leaves that session
   * connected in the background (see `deviceHeldInBackground`) rather than
   * requiring a disconnect first.
   */
  onStartDemo: () => void;
  /** Stops Demo, or resumes the Quad Cortex when it remained connected behind Demo. */
  onStopDemo: () => void;
  /** A real Quad Cortex stayed connected in the background while the demo device is active; selecting its device tab resumes it. */
  deviceHeldInBackground: boolean;
  /** Switch back to the backgrounded Quad Cortex without a full reconnect. */
  onResumeDevice: () => void;
  onClearEvents: () => void;
  onOpenChange: (open: boolean) => void;
}) {
  const [openMenu, setOpenMenu] = useState<string | null>(null);
  const [statusPanelOpen, setStatusPanelOpen] = useState<"device" | "local" | "relay" | null>(null);
  const [localUnpair, publishLocalUnpair] = useState<{ provider: LocalProviderName; stage: "confirm" | "complete"; extensionDialog?: "opening" | "opened" | "failed" } | null>(null);
  const liveUnpair = useRef(localUnpair);
  const promptMounted = useRef(true);
  const removingPrompt = useRef<typeof localUnpair>(null);
  const setLocalUnpair = useCallback((next: typeof localUnpair | ((previous: typeof localUnpair) => typeof localUnpair)) => {
    const value = typeof next === "function" ? next(liveUnpair.current) : next;
    liveUnpair.current = value;
    publishLocalUnpair(value);
  }, []);
  useEffect(() => {
    promptMounted.current = true;
    return () => { promptMounted.current = false; liveUnpair.current = null; };
  }, []);
  const root = useRef<HTMLDivElement>(null);
  const deviceStatusRoot = useRef<HTMLDivElement>(null);
  const triggerRefs = useRef(new Map<string, HTMLButtonElement>());
  const itemRefs = useRef(new Map<string, Array<HTMLButtonElement | null>>());
  // The three status badges declare aria-haspopup="dialog"; Escape used to drop
  // focus on <body> instead of handing it back to the badge that opened the
  // panel, which strands keyboard and screen-reader users at the top of the app.
  const statusBadgeRefs = useRef(new Map<"device" | "local" | "relay", HTMLButtonElement>());
  const statusPanelRefs = useRef(new Map<"device" | "local" | "relay", HTMLElement>());
  const unpairPromptRef = useRef<HTMLDivElement>(null);

  const closeMenu = useCallback((restoreFocus = false) => {
    const previous = openMenu;
    setOpenMenu(null);
    onOpenChange(statusPanelOpen !== null);
    if (restoreFocus && previous) requestAnimationFrame(() => triggerRefs.current.get(previous)?.focus());
  }, [onOpenChange, openMenu, statusPanelOpen]);

  const closeStatusPanel = useCallback((restoreFocus = false) => {
    const previous = statusPanelOpen;
    setStatusPanelOpen(null);
    onOpenChange(false);
    if (restoreFocus && previous) requestAnimationFrame(() => statusBadgeRefs.current.get(previous)?.focus());
  }, [onOpenChange, statusPanelOpen]);

  const open = useCallback((name: string, focus: "first" | "last" | null = null) => {
    setStatusPanelOpen(null);
    setOpenMenu(name);
    onOpenChange(true);
    if (focus) requestAnimationFrame(() => {
      const candidates = itemRefs.current.get(name)?.filter((item): item is HTMLButtonElement => Boolean(item?.isConnected && !item.disabled)) ?? [];
      candidates[focus === "first" ? 0 : candidates.length - 1]?.focus();
    });
  }, [onOpenChange]);

  useEffect(() => {
    const dismiss = (event: PointerEvent) => {
      if (root.current && !root.current.contains(event.target as Node)) closeMenu(false);
      if (!deviceStatusRoot.current?.contains(event.target as Node)) {
        setStatusPanelOpen(null);
        if (!root.current?.contains(event.target as Node)) onOpenChange(false);
      }
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || !(openMenu || statusPanelOpen)) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      // Escape cancels the innermost thing first. It used to tear down the
      // whole panel from inside a destructive confirmation, so the only way
      // out of "Unpair?" by keyboard also lost the panel behind it.
      if (localUnpair) {
        setLocalUnpair(null);
        return;
      }
      if (openMenu) closeMenu(true);
      if (statusPanelOpen) closeStatusPanel(true);
    };
    document.addEventListener("pointerdown", dismiss);
    window.addEventListener("keydown", escape, true);
    return () => {
      document.removeEventListener("pointerdown", dismiss);
      window.removeEventListener("keydown", escape, true);
    };
  }, [closeMenu, closeStatusPanel, localUnpair, onOpenChange, openMenu, statusPanelOpen]);

  useEffect(() => {
    onLocalPanelChange(statusPanelOpen === "local");
    if (statusPanelOpen !== "local") setLocalUnpair(null);
  }, [onLocalPanelChange, statusPanelOpen]);

  // A badge that says aria-haspopup="dialog" owes the dialog focus on open.
  useEffect(() => {
    if (!statusPanelOpen) return;
    const frame = requestAnimationFrame(() => statusPanelRefs.current.get(statusPanelOpen)?.focus());
    return () => cancelAnimationFrame(frame);
  }, [statusPanelOpen]);

  // The removal prompt replaces the provider list in place; without this the
  // caret stays on a button that no longer exists.
  useEffect(() => {
    if (!localUnpair) return;
    const frame = requestAnimationFrame(() => {
      const prompt = unpairPromptRef.current;
      prompt?.querySelector<HTMLButtonElement>("button")?.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [localUnpair]);

  // Closing a removal prompt remounts the provider list, so the control that
  // had focus is gone and focus would fall to <body>. Put it back on the row
  // the prompt was about: its Unpair after a cancelled confirmation, its
  // primary after the completion prompt, else any enabled control in the row,
  // else the dialog itself. One frame later (the list has to exist), and the
  // frame is cancelled if the prompt reopens, the panel closes or this unmounts.
  const previousUnpair = useRef(localUnpair);
  useEffect(() => {
    const previous = previousUnpair.current;
    previousUnpair.current = localUnpair;
    if (localUnpair || !previous || statusPanelOpen !== "local") return;
    const frame = requestAnimationFrame(() => {
      const panel = statusPanelRefs.current.get("local");
      if (!panel?.isConnected) return;
      const row = panel.querySelector<HTMLElement>(`[data-local-provider="${previous.provider}"]`);
      const enabled = (selector: string) => row?.querySelector<HTMLButtonElement>(`${selector}:not([disabled])`) ?? null;
      const target = (previous.stage === "confirm" ? enabled(".local-provider-unpair") : null)
        ?? enabled(".local-provider-action.primary")
        ?? enabled("button")
        ?? panel;
      target.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [localUnpair, statusPanelOpen]);

  // Tab must not walk out of a prompt that asks a yes/no question.
  const trapPromptFocus = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "Tab") return;
    const focusable = [...(unpairPromptRef.current?.querySelectorAll<HTMLButtonElement>("button:not([disabled])") ?? [])];
    if (focusable.length === 0) return;
    const edge = focusable[event.shiftKey ? 0 : focusable.length - 1];
    if (document.activeElement !== edge) return;
    event.preventDefault();
    focusable[event.shiftKey ? focusable.length - 1 : 0].focus();
  };

  const moveFocus = (menuName: string, event: ReactKeyboardEvent, itemIndex: number) => {
    const candidates = itemRefs.current.get(menuName)?.filter((item): item is HTMLButtonElement => Boolean(item?.isConnected && !item.disabled)) ?? [];
    const current = candidates.indexOf(event.currentTarget as HTMLButtonElement);
    if (event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Home" || event.key === "End") {
      event.preventDefault();
      const target = event.key === "Home" ? 0 : event.key === "End" ? candidates.length - 1 : (current + (event.key === "ArrowDown" ? 1 : -1) + candidates.length) % candidates.length;
      candidates[target]?.focus();
    } else if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
      event.preventDefault();
      const menuIndex = menus.findIndex((menu) => menu.name === menuName);
      const next = menus[(menuIndex + (event.key === "ArrowRight" ? 1 : -1) + menus.length) % menus.length];
      open(next.name, "first");
    } else if (event.key === "Escape") closeMenu(true);
    void itemIndex;
  };

  const usbActionRequired = Boolean(runtime?.usbDiagnostics?.writesQuarantined);
  const displayedDevicePhase = qcConnectionIndicatorPhase(connection, deviceReady, syncProgress, { actionRequired: usbActionRequired });
  // Read from `source`, never from `demo`: a demo session reports demo: false.
  const demoActive = connection.source === "virtual";
  const hardwareConnected = deviceReady || deviceHeldInBackground;
  const synchronizedPreset = deviceReady ? presetLabel : connection.lastSync ? `${presetLabel} (last known)` : "Not synchronized";
  const deviceStatusDetail = usbActionRequired
    ? runtime?.usbDiagnostics?.writeQuarantineReason ?? "Writes are paused until the USB session is reset."
    : qcDeviceStatusDetail(connection, deviceReady, syncProgress);
  const lastSynchronized = connection.lastSync ? new Date(connection.lastSync).toLocaleString() : "Not yet synchronized";
  // USB and Demo are two data sources shown through one stable layout. Keeping
  // their fields and action in this view model prevents either mode from
  // gaining a one-off row, disappearing button, or different alignment.
  const devicePanel = demoActive
    ? {
        tone: "demo",
        detail: deviceStatusDetail,
        actionLabel: deviceHeldInBackground ? "Resume Quad Cortex" : "Stop demo",
        onAction: onStopDemo,
        fields: [
          ["Desktop runtime", "Local demo runtime"],
          ["Device gateway", deviceHeldInBackground ? "Available · Quad Cortex connected in the background" : "Local simulation · no hardware changes"],
          ["Quad Cortex", "Demo device · saved on this computer"],
          ["Current preset", presetLabel],
          ["Last synchronized", lastSynchronized]
        ]
      }
    : {
        tone: "usb",
        detail: deviceStatusDetail,
        actionLabel: hardwareConnected ? "Disconnect" : "Connect",
        onAction: hardwareConnected ? onDisconnect : onConnect,
        fields: [
          ["Desktop runtime", runtime?.platform ?? "Starting…"],
          ["Device gateway", `${runtime?.gatewayAvailable ? "Available" : "Unavailable"}${runtime?.message ? ` · ${runtime.message}` : ""}`],
          ["Quad Cortex", deviceStatusDetail],
          ["Current preset", synchronizedPreset],
          ["Last synchronized", lastSynchronized]
        ]
      };
  // Never "pairing in progress": this panel keeps a code live the whole time
  // it is open, so reading that as a pairing under way reported every paired
  // host as mid-sign-in the moment its owner looked at it, and took the
  // Reconnect button away with it, because a sign-in under way offers no
  // action. Nor is there an honest waiting state to put back: the code is
  // redeemed in a browser this host cannot see, and the first this host
  // hears of it is the credential arriving.
  const remotePresentation = qcRelayPresentation(relayStatus, false, relayPairingError);
  // Reported from the rendered state rather than from the click that
  // caused it: REMOTE is also closed by Escape, by a click outside it, and
  // by opening another panel, and a pairing code that outlives its panel is
  // a live invitation on a screen nobody is watching.
  useEffect(() => { onRelayPanelOpen(statusPanelOpen === "relay"); }, [onRelayPanelOpen, statusPanelOpen]);
  const claudePresentation = claudeExtensionPresentation(claudeExtensionStatus, claudeInstallPending, claudeStatusRefreshFailed, claudeApprovalLapsed);
  const chatgptPresentation = chatgptIntegrationPresentation(chatgptIntegrationStatus, chatgptRestartRequired, chatgptStatusRefreshFailed, chatgptRemovalRestartRequired);
  const antigravityPresentation = antigravityIntegrationPresentation(antigravityIntegrationStatus, antigravityStatusRefreshFailed);
  // Named once, from the installed build, and used by the row, its unpair
  // prompt and every sentence either of them draws - see `lmStudioAppName`.
  const lmStudioName = lmStudioAppName(lmStudioIntegrationStatus);
  const lmStudioPresentation = managedMcpIntegrationPresentation(lmStudioName, lmStudioIntegrationStatus, Boolean(lmStudioIntegrationStatus?.lmStudioInstalled), lmStudioEnableSwitch(lmStudioIntegrationStatus), lmStudioStatusRefreshFailed);
  const janPresentation = managedMcpIntegrationPresentation("Jan", janIntegrationStatus, Boolean(janIntegrationStatus?.janInstalled), "Settings → MCP Servers", janStatusRefreshFailed);
  const otherPresentation = otherMcpPresentation(otherMcpStatus, otherMcpStatusRefreshFailed);
  const localSummary = localIntegrationSummary(claudeExtensionStatus, chatgptIntegrationStatus, antigravityIntegrationStatus, {
    claudeInstallPending,
    refreshFailed: claudeStatusRefreshFailed || chatgptStatusRefreshFailed || antigravityStatusRefreshFailed || lmStudioStatusRefreshFailed || janStatusRefreshFailed || otherMcpStatusRefreshFailed,
    lmStudio: lmStudioIntegrationStatus,
    jan: janIntegrationStatus,
    other: otherMcpStatus
  });
  const toggleStatusPanel = (panel: "device" | "local" | "relay") => {
    const next = statusPanelOpen === panel ? null : panel;
    closeMenu(false);
    setStatusPanelOpen(next);
    onOpenChange(next !== null);
  };

  return <nav className="menu-bar" aria-label="Application menu">
    <div className="menubar-device-side">
    <div className="menus" role="menubar" ref={root}>
      {menus.map((menu, menuIndex) => <div key={menu.name} className={`menu${openMenu === menu.name ? " is-open" : ""}`}>
        <button
          className="menu-trigger"
          role="menuitem"
          aria-haspopup="menu"
          aria-expanded={openMenu === menu.name}
          ref={(node) => { if (node) triggerRefs.current.set(menu.name, node); }}
          onClick={() => openMenu === menu.name ? closeMenu(false) : open(menu.name)}
          onKeyDown={(event) => {
            if (event.key === "ArrowDown") { event.preventDefault(); open(menu.name, "first"); }
            else if (event.key === "ArrowUp") { event.preventDefault(); open(menu.name, "last"); }
            else if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
              event.preventDefault();
              const next = menus[(menuIndex + (event.key === "ArrowRight" ? 1 : -1) + menus.length) % menus.length];
              triggerRefs.current.get(next.name)?.focus();
              if (openMenu) open(next.name);
            }
          }}
        >{menu.name}</button>
        {openMenu === menu.name && <div className="menu-popover" role="menu" aria-label={`${menu.name} menu`}>
          {menu.items.map((item, index) => "separator" in item
            ? <div className="menu-separator" role="separator" key={`separator-${index}`} />
            : <button
                key={item.id}
                role={item.checked === undefined ? "menuitem" : "menuitemcheckbox"}
                aria-checked={item.checked}
                disabled={item.disabled}
                ref={(node) => {
                  const refs = itemRefs.current.get(menu.name) ?? [];
                  refs[index] = node;
                  itemRefs.current.set(menu.name, refs);
                }}
                onKeyDown={(event) => moveFocus(menu.name, event, index)}
                onClick={() => { closeMenu(false); onSelect(item.id); }}
              ><span className="menu-check" aria-hidden="true">{item.checked ? <QcUiIcon kind="check" /> : null}</span><span>{item.label}</span>{item.shortcut && <kbd>{item.shortcut}</kbd>}</button>
          )}
        </div>}
      </div>)}
    </div>
    <div className="menubar-actions" ref={deviceStatusRoot}>
      <LocalBadge summary={localSummary} expanded={statusPanelOpen === "local"} busy={localStatusChecking} badgeRef={(node) => { if (node) statusBadgeRefs.current.set("local", node); }} onClick={() => toggleStatusPanel("local")} />
      {statusPanelOpen === "local" && <section className="connection-panel relay-status-panel local-access-panel" role="dialog" aria-modal="false" aria-label="Local AI connections" tabIndex={-1} ref={(node) => { if (node) statusPanelRefs.current.set("local", node); }}>
        {/* The refresh button used to disable itself on every 5s auto-poll, so
            it flickered under the pointer for no reason the user could see.
            Only a pairing action takes it away now; polling says aria-busy. */}
        {/* The control that decides what an AI may do to the hardware lived
            only in Settings, four menus away from the panel that hands an AI
            control of the hardware, so it stays here - on the header line, with
            no label of its own. Its own value is the readout, and ariaLabel
            keeps it a named combobox for anyone who cannot see that. */}
        <header className="local-access-header"><h2>Pair an installed desktop AI app to control QC Remote</h2><ControlAccessSelect className="local-access-level-select" value={controlAccessMode} ariaLabel="AI access level" disabled={localIntegrationAction !== null} onChange={onControlAccessModeChange} /><button className="local-refresh-button" type="button" aria-label="Refresh local AI apps" title="Refresh" aria-busy={localStatusChecking} disabled={localIntegrationAction !== null} onClick={onRefreshLocalIntegrations}><QcUiIcon kind="refresh" /></button></header>
        {localUnpair ? <LocalUnpairPrompt
          promptRef={unpairPromptRef}
          onTrapFocus={trapPromptFocus}
          provider={localUnpair.provider}
          lmStudioName={lmStudioName}
          stage={localUnpair.stage}
          extensionDialog={localUnpair.extensionDialog}
          pending={localIntegrationAction?.startsWith("remove-") === true}
          onCancel={() => setLocalUnpair(null)}
          onConfirm={() => {
            const origin = localUnpair;
            if (!promptMounted.current || liveUnpair.current !== origin || origin.stage !== "confirm" || removingPrompt.current === origin) return;
            removingPrompt.current = origin;
            const complete = () => {
              if (promptMounted.current && liveUnpair.current === origin) {
                setLocalUnpair({ provider: origin.provider, stage: "complete" });
              }
            };
            const settle = (remove: () => boolean | Promise<boolean>) => {
              void completeUnpairIfSuccessful(remove, complete).finally(() => {
                if (removingPrompt.current === origin) removingPrompt.current = null;
              });
            };
            if (origin.provider === "chatgpt") { settle(onRemoveChatgpt); return; }
            if (origin.provider === "antigravity") { settle(onRemoveAntigravity); return; }
            if (origin.provider === "lm-studio") { settle(onRemoveLmStudio); return; }
            settle(onRemoveJan);
          }}
          onOpen={localUnpair.provider === "claude" ? onOpenClaude : localUnpair.provider === "chatgpt" ? onOpenChatgpt : localUnpair.provider === "antigravity" ? onOpenAntigravity : localUnpair.provider === "lm-studio" ? onOpenLmStudio : onOpenJan}
        /> : <div className="local-provider-list">
          {/* One handler per act, and the row reads the act off its own
              presentation. Each row used to carry a ternary chain that re-derived
              the decision the label had already made, which is how a button came
              to say Update and only raise a window. */}
          <LocalProviderRow
            provider="claude"
            name="Claude Desktop"
            presentation={claudePresentation}
            actionPending={localIntegrationAction === "claude"}
            actionDisabled={localIntegrationAction !== null}
            handlers={{
              open: onOpenClaude,
              repair: onInstallClaude,
              pair: onInstallClaude,
              install: onOpenClaudeDownload,
              refresh: onRefreshLocalIntegrations,
              // Claude owns extension removal, so QC Remote cannot perform it.
              // What it can do is put the user in front of the control that
              // does: spawning Claude with the bundle opens its extension page
              // with QC Remote selected - the same native call the repair makes,
              // not a second mechanism - and the numbered steps survive only for
              // the case where that could not be opened. The button is spelled
              // and coloured like every other row's Unpair, because six rows
              // showing one control in two colours looked inconsistent.
              unpair: () => {
                const origin = { provider: "claude", stage: "complete", extensionDialog: "opening" } as const;
                setLocalUnpair(origin);
                void onOpenClaudeExtensions().then((opened) => {
                  if (promptMounted.current && liveUnpair.current === origin) setLocalUnpair({ ...origin, extensionDialog: opened ? "opened" : "failed" });
                });
              }
            }}
          />
          <LocalProviderRow
            provider="chatgpt"
            name="ChatGPT / Codex"
            presentation={chatgptPresentation}
            actionPending={localIntegrationAction === "chatgpt"}
            actionDisabled={localIntegrationAction !== null}
            handlers={{
              open: onOpenChatgpt,
              repair: onInstallChatgpt,
              pair: onInstallChatgpt,
              install: onOpenChatgptDownload,
              refresh: onRefreshLocalIntegrations,
              configure: onOpenChatgptConfiguration,
              unpair: () => setLocalUnpair({ provider: "chatgpt", stage: "confirm" })
            }}
          />
          <LocalProviderRow
            provider="antigravity"
            name="Google Antigravity"
            presentation={antigravityPresentation}
            actionPending={localIntegrationAction === "antigravity"}
            actionDisabled={localIntegrationAction !== null}
            handlers={{
              open: onOpenAntigravity,
              // Antigravity's MCP-list reload is not here. It was this row's
              // primary on a route that was already current, so a healthy row
              // wore Update and pressing it changed nothing visible. QC Remote
              // reattaches on its own at start, and the reload is the same
              // native call as Pair and Update, which this row already offers.
              repair: onInstallAntigravity,
              pair: onInstallAntigravity,
              install: onOpenAntigravityDownload,
              refresh: onRefreshLocalIntegrations,
              unpair: () => setLocalUnpair({ provider: "antigravity", stage: "confirm" })
            }}
          />
          <LocalProviderRow
            provider="lm-studio"
            name={lmStudioName}
            presentation={lmStudioPresentation}
            actionPending={localIntegrationAction === "lm-studio"}
            actionDisabled={localIntegrationAction !== null}
            handlers={{
              open: onOpenLmStudio,
              repair: onInstallLmStudio,
              pair: onInstallLmStudio,
              install: onOpenLmStudioDownload,
              refresh: onRefreshLocalIntegrations,
              unpair: () => setLocalUnpair({ provider: "lm-studio", stage: "confirm" })
            }}
          />
          <LocalProviderRow
            provider="jan"
            name="Jan"
            presentation={janPresentation}
            actionPending={localIntegrationAction === "jan"}
            actionDisabled={localIntegrationAction !== null}
            handlers={{
              open: onOpenJan,
              repair: onInstallJan,
              pair: onInstallJan,
              install: onOpenJanDownload,
              refresh: onRefreshLocalIntegrations,
              unpair: () => setLocalUnpair({ provider: "jan", stage: "confirm" })
            }}
          />
          {/* No app is named, so there is nothing to open and nothing to
              repair. Its one act is handing over a configuration to paste. */}
          <LocalProviderRow
            provider="other"
            name="Other MCP app"
            presentation={otherPresentation}
            actionPending={localIntegrationAction === "other"}
            actionDisabled={localIntegrationAction !== null}
            handlers={{ copy: onCopyOtherMcpSetup }}
          />
        </div>}
      </section>}
      <RelayBadge phase={remotePresentation.phase} detail={remotePresentation.detail} expanded={statusPanelOpen === "relay"} badgeRef={(node) => { if (node) statusBadgeRefs.current.set("relay", node); }} onClick={() => toggleStatusPanel("relay")} />
      {statusPanelOpen === "relay" && <section className="connection-panel relay-status-panel remote-access-panel" role="dialog" aria-modal="false" aria-label="Remote access" tabIndex={-1} ref={(node) => { if (node) statusPanelRefs.current.set("relay", node); }}>
        {relayPairingError
          ? <RemotePairingFailureState error={relayPairingError} onRetry={onRelayPairingRefresh} />
          : <RemotePairingState
              qrCode={relayPairingQrCode}
              pairUrl={relayPairingUrl}
              starting={relayPairingStarting || !relayStatus}
              onOpenLink={onRelayPairingOpen}
              onShare={onRelayPairingShare}
            />}
        {relayStatus?.paired && relayStatus.pairing && <RemoteAccessCard
          pairing={relayStatus.pairing}
          phase={remotePresentation.phase}
          pending={relayPending}
          canReconnect={remotePresentation.action === "reconnect"}
          onUnpair={onRelayUnpair}
          onReconnect={onRelayReconnect}
          address={relayConnectorAddress}
          onOpenAddress={onOpenRelayConnector}
          onShare={onShareRelayConnector}
          connectors={relayConnectors}
          connectorsError={relayConnectorsError}
          unlinkingGrantId={relayUnlinkingGrantId}
          onUnlink={onRelayUnlinkConnector}
          accessMode={controlAccessMode}
          onAccessModeChange={onControlAccessModeChange}
        />}
        {relayStatus?.pendingRevocation && <div className="remote-access-card" role="status">
          <p>{relayStatus.paired ? "A previous cloud authorization still needs to be withdrawn." : relayStatus.detail ?? "Cloud authorization withdrawal still needs to finish."}</p>
          <button type="button" disabled={relayPending} onClick={onRetryRelayWithdrawal}>Retry withdrawal</button>
        </div>}
        <RemoteAccountDeletionLink onOpen={onRelayDeleteAccount} />
      </section>}
      <ConnectionBadge connection={connection} deviceReady={deviceReady} syncProgress={syncProgress} actionRequired={usbActionRequired} expanded={statusPanelOpen === "device"} badgeRef={(node) => { if (node) statusBadgeRefs.current.set("device", node); }} onClick={() => toggleStatusPanel("device")} />
      {statusPanelOpen === "device" && <section className={`connection-panel device-connection-panel panel-tone-${devicePanel.tone}`} role="dialog" aria-modal="false" aria-label="Connection details" tabIndex={-1} ref={(node) => { if (node) statusPanelRefs.current.set("device", node); }}>
        <div className="connection-device-tabs" role="group" aria-label="Active device">
          {(physicalDevices.length ? physicalDevices : [{id: selectedDeviceId ?? "", name: "Quad Cortex"}]).map((device) => {
            const selected = device.id === selectedDeviceId || (!selectedDeviceId && physicalDevices.length <= 1);
            return <button key={device.id} type="button" aria-pressed={selected && !demoActive} disabled={busy || demoPending} onClick={() => {
              if (!selected && device.id) onSelectDevice(device.id);
              else if (demoActive) (deviceHeldInBackground ? onResumeDevice : onConnect)();
              else if (!hardwareConnected) onConnect();
            }}>
              <span className={`connection-panel-light panel-phase-${selected ? demoActive ? hardwareConnected ? "ready" : "idle" : displayedDevicePhase : device.phase === "ready" ? "ready" : device.phase === "syncing" || device.phase === "handshaking" ? "syncing" : "idle"}`} aria-hidden="true" />
              {device.name}<small>USB</small>
            </button>;
          })}
          <button type="button" className="demo-device-tab" aria-pressed={demoActive} disabled={busy || demoPending} onClick={() => { if (!demoActive) onStartDemo(); }}>
            <span className={`connection-panel-light panel-phase-${demoPending ? "syncing" : "demo"}`} aria-hidden="true" />
            Demo device<small>{demoPending ? "Opening…" : "DEMO"}</small>
          </button>
        </div>
        <div className="connection-status-line">
          <p>{devicePanel.detail}</p>
          <button type="button" disabled={busy || demoPending} onClick={devicePanel.onAction}>{devicePanel.actionLabel}</button>
        </div>
        <dl>
          {devicePanel.fields.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}
        </dl>
        <div className="connection-panel-log-heading"><strong>Connection log</strong><span>{events.length} events</span><button type="button" onClick={onClearEvents}>Clear history</button></div>
        <div className="connection-panel-log">
          {[...events].reverse().map((entry, index) => <div className={`result-${entry.result}`} key={`${entry.at}-${events.length - index}`}><div className="connection-log-summary"><i aria-hidden="true" /><strong>{entry.event.replaceAll("-", " ")}</strong><time>{new Date(entry.at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })}</time><span>{entry.result}</span></div><p>{entry.detail}</p></div>)}
        </div>
      </section>}
    </div>
    </div>
  </nav>;
}

/** The pairing code, and nothing standing in front of it.
 *
 * It used to sit behind a "Pair device" button whose only effect was to reveal
 * it, and pairing then replaced it outright with a summary of the one pairing
 * this host had made - so adding a second computer meant removing the first.
 * The code is what this panel is for; it stays while the panel is open, and
 * what it produced is the list underneath.
 *
 * The copy changes with the pairing because the code's meaning does. A code
 * scanned by an unpaired computer claims it for the account that signs in. A
 * code scanned by a paired one replaces that claim, which is worth saying
 * plainly rather than letting someone discover it. */
/** The pairing code, the link that carries it, and nothing else.
 *
 * There is no expiry shown and no "waiting" line. A code that lapses while the
 * panel is open is replaced silently, so the only states this can be in are
 * "still being created" and "ready to scan" - neither of which needs a status
 * strip to explain it.
 *
 * The link is shown in full and opens in the system browser, because signing in
 * on this machine is a legitimate way to pair and was previously reachable only
 * by sharing the link to yourself. */
function RemotePairingState({ qrCode, pairUrl, starting, onOpenLink, onShare }: { qrCode?: string; pairUrl?: string; starting: boolean; onOpenLink: () => void; onShare: () => void }) {
  return <div className="remote-state remote-pairing-state">
    <h2>Quad Cortex pairing</h2>
    <p>Scan this QR code or open the link to pair this QC to Remote MCP Cloud Relay using a Google account. You can then connect to it remotely using mobile, web or desktop AI apps via Remote MCP Connectors.</p>
    <div className="remote-pairing-share-panel">
      <button className={`relay-pairing-qr${qrCode ? "" : " is-loading"}`} type="button" disabled={!pairUrl} onClick={onOpenLink} aria-label="Open Quad Cortex pairing link">
        {qrCode ? <img src={qrCode} alt="Quad Cortex pairing QR code" /> : <span className="relay-pairing-placeholder" aria-label="Creating secure pairing code" />}
      </button>
      <div className="relay-pairing-link">
        {pairUrl
          ? <button className="remote-pair-link" type="button" title={pairUrl} onClick={onOpenLink}>{pairUrl}</button>
          : <span className="remote-pair-link is-pending">{starting ? "Creating a secure pairing code…" : "Creating pairing code"}</span>}
        <button className="remote-status-action" type="button" disabled={!pairUrl} onClick={onShare}>Share</button>
      </div>
    </div>
  </div>;
}

function RemoteAccessCard({ pairing, phase, pending, canReconnect, onUnpair, onReconnect, address, onOpenAddress, onShare, connectors, connectorsError, unlinkingGrantId, onUnlink, accessMode, onAccessModeChange }: { pairing: NonNullable<PublicRelayStatus["pairing"]>; phase: QcIndicatorPhase; pending: boolean; canReconnect: boolean; onUnpair: () => void; onReconnect: () => void; address: string; onOpenAddress: () => void; onShare: () => void; connectors: PublicRelayConnector[]; connectorsError?: string; unlinkingGrantId?: string; onUnlink: (grantId: string) => void; accessMode: ControlAccessMode; onAccessModeChange: (mode: ControlAccessMode) => void }) {
  const pairedAt = new Date(pairing.pairedAt).toLocaleString([], { dateStyle: "medium", timeStyle: "short" });
  return <section className="remote-access-card">
    <div className="remote-access-summary">
      <div className="local-provider-copy">
        <strong><i className={`local-status-dot tone-${phase}`} aria-hidden="true" />{pairing.email ? `Quad Cortex paired to ${pairing.email}` : "Quad Cortex paired"}</strong>
        <small>Paired {pairedAt}{pairing.device ? ` from ${pairing.device}` : ""}</small>
      </div>
      <div className="remote-access-actions">
        <ControlAccessSelect className="remote-access-level-select" value={accessMode} ariaLabel="Remote AI access level" disabled={pending} onChange={onAccessModeChange} />
        {canReconnect && <button className="local-provider-action" type="button" disabled={pending} onClick={onReconnect}>Reconnect</button>}
        <button className="remote-unpair-button" type="button" disabled={pending} onClick={onUnpair}>{pending ? "Unpairing…" : "Unpair"}</button>
      </div>
    </div>
    <div className="remote-connector-line">
      <div><strong>Remote MCP Connector</strong><button className="remote-connector-address" type="button" title={`Open ${address}`} onClick={onOpenAddress}>{address}</button></div>
      <button className="local-provider-action" type="button" onClick={onShare}>Share</button>
    </div>
    <div className="remote-connector-list">
      <div className="remote-device-heading">
        <strong>Linked Remote MCP apps</strong>
        <span>{connectorsError ? "Could not check" : `${connectors.length} linked`}</span>
      </div>
      {connectorsError
        ? <p className="remote-device-empty">Could not read linked Remote MCP apps: {connectorsError}</p>
        : connectors.length === 0
          ? <p className="remote-device-empty">No Remote MCP app is linked yet. Share the connector address and authorize it with this Google account.</p>
          : connectors.map((connector) => <RemoteConnectorRow key={connector.grantId} connector={connector} unlinking={unlinkingGrantId === connector.grantId} unlinkDisabled={Boolean(unlinkingGrantId)} onUnlink={onUnlink} />)}
    </div>
  </section>;
}

function RemoteConnectorRow({ connector, unlinking, unlinkDisabled, onUnlink }: { connector: PublicRelayConnector; unlinking: boolean; unlinkDisabled: boolean; onUnlink: (grantId: string) => void }) {
  const authorizedAt = new Date(connector.authorizedAt).toLocaleString([], { dateStyle: "medium", timeStyle: "short" });
  const access = connector.scopes.includes("qc:control") ? "Control access" : "Read-only access";
  const authorizationClient = [connector.device, connector.browser].filter(Boolean).join(" ") || connector.authorizedFrom;
  const authorizationLocation = [connector.city, connector.country].filter(Boolean).join(", ");
  const authorizationDetail = [authorizationClient && `Authorized from ${authorizationClient}`, authorizationLocation, connector.ipAddress, access, `Linked ${authorizedAt}`].filter(Boolean).join(" · ");
  const lastActive = connector.lastActiveAt && new Date(connector.lastActiveAt).toLocaleString([], { dateStyle: "medium", timeStyle: "short" });
  const activityClient = [connector.activityDevice, connector.activityBrowser].filter(Boolean).join(" ");
  const activityLocation = [connector.activityCity, connector.activityCountry].filter(Boolean).join(", ");
  const activityDetail = lastActive
    ? [connector.active ? "Active now" : `Last active ${lastActive}`, connector.appVersion && `App ${connector.appVersion}`, activityClient && `Provider route: ${activityClient}`, activityLocation, connector.activityIpAddress].filter(Boolean).join(" · ")
    : undefined;
  return <article className="remote-connector-row" title={`OAuth client ${connector.clientId}`}>
    <div className="local-provider-copy">
      <strong>{connector.name}</strong>
      {activityDetail && <small>{activityDetail}</small>}
      <small>{authorizationDetail}</small>
    </div>
    <button className="remote-unpair-button" type="button" disabled={unlinkDisabled} onClick={() => onUnlink(connector.grantId)}>{unlinking ? "Unlinking…" : "Unlink"}</button>
  </article>;
}

function RemotePairingFailureState({ error, onRetry }: { error: string; onRetry: () => void }) {
  return <div className="remote-state remote-pairing-failure">
    <h2>Quad Cortex pairing</h2>
    <p>QC Remote could not create a pairing code.</p>
    <div className="relay-pairing-status">
      <strong><span className="connection-panel-light panel-phase-error" />Pairing unavailable</strong>
      <button className="remote-status-action" type="button" onClick={onRetry}>Try again</button>
      <small>{error}</small>
    </div>
  </div>;
}

function LocalProviderRow({ provider, name, presentation, handlers, actionPending = false, actionDisabled }: { provider: LocalProviderName | "other"; name: string; presentation: LocalProviderPresentation; handlers: LocalProviderHandlers; actionPending?: boolean; actionDisabled: boolean }) {
  // Primary then Unpair, in those two positions, on every paired row. `Open`
  // used to be a third button that appeared only while a provider was
  // connected - in a different slot, beside a primary whose label had gone
  // blank - so the controls moved as the row went green. The status text was
  // clickable in that state too, which made three ways to open one app.
  const buttons = localRowButtons(presentation, handlers);
  // One in-flight action disables all six rows. Saying which one, and that it
  // is temporary, costs a tooltip; silence made the panel look broken.
  const blockedReason = actionDisabled && !actionPending ? "Another local AI app is being set up. This becomes available when it finishes." : undefined;
  // The row's classes come from one helper, in this panel's own namespace. A
  // row spelling `phase-${phase}` reached the connection badge's bare colour
  // rules, so two of the seven phases were painted by a different component.
  return <article className={localRowClassName(presentation.phase)} data-local-provider={provider}>
    <div className="local-provider-main">
      <QcLocalAppLogo provider={provider} />
      <div className="local-provider-copy">
        <strong>{name}</strong>
        <span className="local-provider-status"><i className={`local-status-dot tone-${presentation.tone}`} aria-hidden="true" />{presentation.status}</span>
        <small>{presentation.detail}</small>
      </div>
    </div>
    {buttons.length > 0 && <div className={`local-provider-actions${buttons.length === 1 ? " is-single-action" : ""}`}>
      {buttons.map((button) => <button
        key={button.act}
        type="button"
        className={button.tone === "danger" ? "local-provider-unpair" : `local-provider-action${button.tone === "primary" ? " primary" : ""}`}
        disabled={actionDisabled}
        title={blockedReason}
        onClick={button.run}
      >{actionPending && button.act !== "unpair" ? "Working…" : button.label}</button>)}
    </div>}
    {/* A row says what it is and what to do about it, and nothing else. Each
        result used to land in a live region inside the card and stay there, so
        six rows accumulated six paragraphs of history. The status strip is a
        role="status" live region too, and it clears itself. */}
  </article>;
}

function LocalUnpairPrompt({ provider, lmStudioName, stage, pending, extensionDialog, promptRef, onTrapFocus, onCancel, onConfirm, onOpen }: { provider: LocalProviderName; /** The one provider whose name is read off the installed build rather than fixed. */ lmStudioName: string; stage: "confirm" | "complete"; pending: boolean; extensionDialog?: "opening" | "opened" | "failed"; promptRef: RefObject<HTMLDivElement | null>; onTrapFocus: (event: ReactKeyboardEvent<HTMLDivElement>) => void; onCancel: () => void; onConfirm: () => void; onOpen: () => void }) {
  const claude = provider === "claude";
  const antigravity = provider === "antigravity";
  const lmStudio = provider === "lm-studio";
  const jan = provider === "jan";
  const name = claude ? "Claude Desktop" : antigravity ? "Google Antigravity" : lmStudio ? lmStudioName : jan ? "Jan" : "ChatGPT / Codex";
  // aria-modal="true" inside a panel that declares aria-modal="false" is a lie
  // about the rest of the document. The prompt is a non-modal alert dialog
  // with a real Tab trap and an Escape that cancels it, not the panel.
  if (stage === "confirm") return <div className="local-unpair-prompt" role="alertdialog" aria-label={`Unpair ${name}`} ref={promptRef} onKeyDown={onTrapFocus}>
    <div className="local-unpair-heading"><QcLocalAppLogo provider={provider} /><strong>Unpair {name}?</strong></div>
    <p>QC Remote will remove only its local connection. Other MCP servers will remain unchanged.</p>
    <div className="local-unpair-actions"><button type="button" onClick={onCancel}>Cancel</button><button className="danger" type="button" disabled={pending} onClick={onConfirm}>{pending ? "Unpairing…" : "Unpair"}</button></div>
  </div>;
  // Claude's remove control lives on Claude's own extension page, which QC
  // Remote has just opened with QC Remote selected. Once that is in front of
  // the user there is nothing left to walk them through, so the numbered steps
  // stay only for the case where opening it failed.
  const claudeRemovalSteps = claude && extensionDialog === "failed";
  const claudeRemoval = extensionDialog === "opened"
    ? "Claude Desktop's extension page is open with QC Remote selected. Remove it there - Claude manages its own extensions, so this is the whole of the removal."
    : claudeRemovalSteps
      ? "QC Remote could not open Claude Desktop's extension page, so the removal has to be reached by hand. Claude Desktop stays installed."
      : "Opening Claude Desktop's extension page with QC Remote selected\u2026";
  return <div className="local-unpair-prompt local-unpair-complete" role="status" ref={promptRef} onKeyDown={onTrapFocus}>
    <div className="local-unpair-heading"><QcLocalAppLogo provider={provider} /><strong>{claude ? "Remove QC Remote in Claude Desktop" : "QC Remote was unpaired"}</strong></div>
    <p>{claude ? claudeRemoval : `If ${name} is open:`}</p>
    {(!claude || claudeRemovalSteps) && <ol>{claude
      ? <><li>Open Settings → Extensions.</li><li>Select QC Remote and remove the extension.</li></>
      : antigravity
        ? <><li>Open Settings → Customizations.</li><li>Refresh Installed MCP Servers.</li></>
        : lmStudio
          ? <><li>Open the Program tab.</li><li>Reload MCP servers if QC Remote still appears.</li></>
          : jan
            ? <><li>Open Settings → MCP Servers.</li><li>Start a new chat if QC Remote still appears.</li></>
        : <><li>Open Settings → Plugins → MCP.</li><li>Reload MCP servers, or restart ChatGPT, if QC Remote still appears.</li></>}</ol>}
    <div className="local-unpair-actions"><button type="button" onClick={onCancel}>{claude ? "Back" : "Done"}</button><button className="primary" type="button" onClick={onOpen}>Open {name}</button></div>
  </div>;
}

function ConnectionBadge({ connection, deviceReady, syncProgress, actionRequired, expanded, badgeRef, onClick }: { connection: ConnectionState; deviceReady: boolean; syncProgress: number | null; actionRequired: boolean; expanded: boolean; badgeRef: (node: HTMLButtonElement | null) => void; onClick: () => void }) {
  const syncing = syncProgress !== null;
  const phase = qcConnectionIndicatorPhase(connection, deviceReady, syncProgress, { actionRequired });
  // This is the transport selector, not a second device-status readout. The
  // light and its details panel communicate connection state; its label stays
  // USB in every hardware state, matching Android and preserving a compact
  // menubar. The one exception is the demo device, which is not USB at all:
  // it says DEMO, in its own violet, so it cannot be read as a live QC.
  const label = "USB";
  const shownLabel = phase === "demo" ? "DEMO" : label;
  const detail = actionRequired
    ? "Writes are paused until the USB session is reset."
    : qcDeviceStatusDetail(connection, deviceReady, syncProgress);
  return <button type="button" className={`connection-badge phase-${phase}`} ref={badgeRef} aria-expanded={expanded} aria-haspopup="dialog" aria-label={syncing ? `Synchronizing device, ${syncProgress}% complete; open connection details` : phase === "demo" ? "DEMO, a demo device with no Quad Cortex attached; open connection details" : `${label}; open connection details`} title={detail} onClick={onClick}>
    <span className="status-light" />
    <span>{shownLabel}</span>
    <QcUiIcon kind="down" className="connection-chevron" />
    {syncing && <span className="connection-progress" aria-hidden="true"><span style={{ width: `${syncProgress}%` }} /></span>}
  </button>;
}

function RelayBadge({ phase, detail, expanded, badgeRef, onClick }: { phase: QcIndicatorPhase; detail: string; expanded: boolean; badgeRef: (node: HTMLButtonElement | null) => void; onClick: () => void }) {
  return <button type="button" className={`connection-badge relay-badge phase-${phase}`} ref={badgeRef} aria-expanded={expanded} aria-haspopup="dialog" aria-label={`REMOTE; ${expanded ? "close" : "open"} remote access`} title={detail} onClick={onClick}>
    <span className="status-light" />
    <span>REMOTE</span>
    <QcUiIcon kind="down" className="connection-chevron" />
  </button>;
}

function LocalBadge({ summary, expanded, busy, badgeRef, onClick }: { summary: LocalIntegrationSummary; expanded: boolean; busy: boolean; badgeRef: (node: HTMLButtonElement | null) => void; onClick: () => void }) {
  // The status text truncated to "Action n…" inside a fixed-width badge, which
  // reads as a defect. The light carries the state, and the aria-label and title
  // that assistive technology and hover already use still spell it out in full.
  return <button type="button" className={`connection-badge local-badge phase-${summary.phase}`} ref={badgeRef} aria-expanded={expanded} aria-haspopup="dialog" aria-busy={busy} aria-label={`LOCAL, ${summary.status}; ${expanded ? "close" : "open"} local AI connections`} title={`Local AI connections: ${summary.status}`} onClick={onClick}>
    <span className="status-light" />
    <span>LOCAL</span>
    <QcUiIcon kind="down" className="connection-chevron" />
  </button>;
}
