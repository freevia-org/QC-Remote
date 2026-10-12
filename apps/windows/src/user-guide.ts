/**
 * The QC Remote user guide, as data.
 *
 * One guide replaces the three-paragraph help dialog and the separate shortcut
 * list. It is plain data and types so the User Guide window can render it, a test
 * can measure it, and the keyboard rows can be ratcheted against the app's two
 * key handlers — a shortcut cannot be documented that the app does not
 * implement, and a handler cannot gain a key without a line here.
 *
 * Every sentence describes what this source tree implements. Nothing about the
 * Quad Cortex beyond what QC Remote exposes belongs here.
 */

export type ShortcutRow = { keys: string; action: string };

export type GuideBlock =
  | { kind: "paragraph"; text: string }
  | { kind: "steps"; items: string[] }
  | { kind: "list"; items: string[] }
  | { kind: "shortcuts"; rows: readonly ShortcutRow[] }
  | { kind: "note"; text: string };

export type GuideSection = { id: string; title: string; summary: string; blocks: readonly GuideBlock[] };

/**
 * Every key the Grid handler and the application shortcut handler act on.
 *
 * A row's `keys` is the label; the test maps it back to the literal key values
 * the handlers compare against, in both directions.
 */
export const KEYBOARD_SHORTCUTS: readonly ShortcutRow[] = [
  { keys: "F1", action: "Open the user guide" },
  { keys: "1–8", action: "Press Footswitches A–H in whichever mode the unit is in" },
  { keys: "Ctrl+1–8", action: "Select Scenes A–H directly" },
  { keys: "[ / ]", action: "Step a whole bank down or up — not the same as the BANK switches, which move one preset" },
  { keys: "Arrow keys / Enter", action: "Select the nearest Grid block / open its live parameters" },
  { keys: "T / Shift+T", action: "Tap tempo / open the tuner on the unit" },
  { keys: "B", action: "Toggle the selected block on or off" },
  { keys: "Delete", action: "Remove the selected block; Ctrl+Z puts it back" },
  { keys: "Ctrl+Z", action: "Undo the last change QC Remote made" },
  { keys: "Ctrl+Y / Ctrl+Shift+Z", action: "Redo the change that was undone" },
  { keys: "Ctrl+C", action: "Copy the settings of the selected device" },
  { keys: "Ctrl+V", action: "Paste those settings onto another device of the same model" },
  { keys: "Ctrl+S", action: "Save the preset to a workspace file on this computer" },
  { keys: "Ctrl+Shift+S", action: "Open Save to QC for the loaded preset" },
  { keys: "Escape", action: "Cancel a pending edit, or close the open dialog, panel, or browser" }
];

/** What the on-screen chassis and its screen answer to. */
export const MOUSE_INTERACTIONS: readonly ShortcutRow[] = [
  { keys: "Click a Grid block", action: "Open its live parameters; click it again to close them" },
  { keys: "Click an empty Grid cell", action: "Start adding a device at that position" },
  { keys: "Click a row input or output", action: "Open the route picker for that signal line" },
  { keys: "Click a footswitch", action: "Press Footswitch A–H on the unit" },
  { keys: "Click BANK UP / BANK DOWN", action: "Step to the previous / next preset" },
  { keys: "Click TEMPO", action: "Send a tap; tap in rhythm to set the tempo" },
  { keys: "Drag, scroll, or arrow TEMPO", action: "Change the tempo one BPM at a time, from 40 to 240" },
  { keys: "Drag, scroll, or arrow a footswitch", action: "Turn it as an encoder while a device's parameters are open" },
  { keys: "Drag, scroll, or arrow the volume knob", action: "Change the master volume on the unit" },
  { keys: "Click the preset name on the screen", action: "Open the unit's preset Directory" },
  { keys: "Click the save icon on the screen", action: "Open Save to QC for the loaded preset" },
  { keys: "Click the undo arrow on the screen", action: "Undo the last change QC Remote made" }
];

export const USER_GUIDE: readonly GuideSection[] = [
  {
    id: "overview",
    title: "What QC Remote is",
    summary: "QC Remote draws your connected Quad Cortex on screen and sends every control you touch to the hardware.",
    blocks: [
      {
        kind: "paragraph",
        text: "QC Remote runs on this computer and talks to a Quad Cortex over USB. It shows the loaded preset and sends your controls and edits to the hardware. It is independent and unofficial, and is not affiliated with, authorized by, sponsored by, or endorsed by Neural DSP Technologies Oy. Neural DSP and Quad Cortex name the product it works with."
      },
      {
        kind: "paragraph",
        text: "Open this guide any time with F1 or Help > User Guide. It opens in a window of its own that you can move and resize beside the main window. Every section is listed down its left side, and clicking one jumps to it."
      },
      {
        kind: "list",
        items: [
          "Watch the loaded preset, its signal chain, its scenes, and the current mode.",
          "Press footswitches, select scenes, step through presets and banks, tap tempo, and set the master volume.",
          "Add, move, bypass, and remove devices in the Grid, and edit the parameters of the device you select.",
          "Rename the loaded preset, save it back to the unit, and keep a copy of it in a file on this computer.",
          "Let a desktop or remote AI application drive the same controls, under an access level you set and with the confirmations it must get from you."
        ]
      },
      {
        kind: "paragraph",
        text: "QC Remote uses the Quad Cortex's existing interface and carries no device firmware or changes to the unit's firmware. It adds a mouse, keyboard, larger screen, and the option of letting an assistant work the same controls."
      },
      {
        kind: "note",
        text: "An edit you make to a preset lives in the unit's memory until something saves it. Recalling another preset, reloading the current one, or switching the unit off throws those edits away. Save the preset to the unit when you want to keep a change, and keep your own backups of the presets that matter to you."
      }
    ]
  },
  {
    id: "getting-started",
    title: "Getting started",
    summary: "Connect the unit over USB, wait for the device badge to turn green, and check that the preset on screen matches the preset on the unit.",
    blocks: [
      {
        kind: "steps",
        items: [
          "Close any other application that may be connected to the Quad Cortex. One application at a time can hold the Quad Cortex USB interface.",
          "Connect the Quad Cortex to this computer with a USB cable and switch the unit on.",
          "Start QC Remote. It begins looking for the unit by itself, so there is nothing to press to start.",
          "Watch the device badge at the right end of the menu bar. It moves through Connecting, Searching for Quad Cortex, and Synchronizing active preset before it settles on Live and synchronized.",
          "Click the badge and check that Current preset in the panel matches what the unit's screen shows."
        ]
      },
      {
        kind: "paragraph",
        text: "The badge turns green only when three things agree: the local runtime is answering, the USB link reports a live and synchronized unit, and the active preset has been read at least once. A green badge means the window and the unit are describing the same preset, so anything you press acts on what you can see."
      },
      {
        kind: "paragraph",
        text: "Use the tabs at the top of the USB panel to switch between Quad Cortex and DEMO. On a live device, the button beside the status connects or disconnects the hardware. On DEMO, it connects to a Quad Cortex or switches back to one already connected in the background; it never disconnects the demo device. Connection log shows recent activity, with Clear history beside the event count. Use Help > Export Diagnostics… to save a redacted report."
      },
      {
        kind: "note",
        text: "DEMO stores edits on this computer and sends nothing to the Quad Cortex. Switching to DEMO keeps an already-connected unit available; select its tab to return to it."
      }
    ]
  },
  {
    id: "control-surface",
    title: "The on-screen Quad Cortex",
    summary: "The chassis in the window is the main control surface, and every switch, knob, and screen control on it acts on the connected unit.",
    blocks: [
      {
        kind: "paragraph",
        text: "The window shows a Quad Cortex. The master volume knob and the power button sit on the left, the device screen fills the middle, and eleven switches run along the bottom and beside the screen: Footswitches A through H, BANK UP, BANK DOWN, and TEMPO. The lamps follow the unit, so a footswitch lit here is lit there, and the TEMPO lamp pulses on the beat the unit reports."
      },
      {
        kind: "list",
        items: [
          "Click a footswitch to press it in whichever mode the unit is in.",
          "Click BANK UP or BANK DOWN to step to the previous or next preset.",
          "Click TEMPO to send a tap, tap it in rhythm to set the tempo, or drag, scroll, or arrow it to change the tempo one BPM at a time.",
          "Drag the master volume knob up or down, scroll on it, or give it focus and use the arrow keys.",
          "Drag, scroll, or arrow a footswitch to turn it as an encoder while a device's parameters are open."
        ]
      },
      {
        kind: "paragraph",
        text: "The screen shows the Grid of the loaded preset. The preset name across the top opens the unit's preset Directory. The arrow beside it undoes the last change, and the save icon opens Save to QC. The scene control lists the preset's scenes, the mode control lists the mode slots the unit reports, and the input and output ends of each signal line open a route picker for that row. Tuner and Gig View, from the Performance menu, open on the unit itself rather than in this window."
      },
      {
        kind: "note",
        text: "The power button on the chassis does nothing to the hardware, and says so when you click it. Powering the unit down and locking its screen stay on the physical Quad Cortex, where you can see what you are switching off."
      }
    ]
  },
  {
    id: "grid-editing",
    title: "Editing the Grid",
    summary: "Select a device in the Grid to open its parameters, and use the Preset and Edit menus to add devices, change routing, or throw an edit away.",
    blocks: [
      {
        kind: "paragraph",
        text: "Click a block on the screen to open its live parameters, and click the same block again to close them. The editor shows the device's name and category, its scene selector, its footswitch assignment, and the parameters that fit on a page, with tabs when there is more than one page. While it is open, the switches under the screen act as the unit's encoders: drag one, scroll on it, or use the arrow keys, and the parameter above it moves."
      },
      {
        kind: "steps",
        items: [
          "Click a block, or select one with the arrow keys and press Enter.",
          "Find the parameter you want on the page, using the tabs if the device has more than one.",
          "Drag, scroll, or arrow the control under it until the value reads what you want.",
          "To put the device on a footswitch, click the footswitch button in the editor, then press 1 through 8 or click that switch on the chassis. Pressing the switch it already has clears the assignment.",
          "Press Escape to return to the Grid. The change is already on the unit, and it is not yet saved."
        ]
      },
      {
        kind: "list",
        items: [
          "Preset > Add Device… places a device in an empty cell. The unit may refuse it if the preset has too little DSP left.",
          "Preset > Routing… splits a row at one column and rejoins it at another, or returns the row to a single path.",
          "Preset > Discard Changes… reloads the stored preset and throws away every unsaved edit.",
          "Edit > Copy Device and Edit > Paste Device copy the settings of the selected device onto another device of the same model.",
          "Edit > Undo and Edit > Redo step back and forward through the changes QC Remote has made in this session."
        ]
      },
      {
        kind: "note",
        text: "Adding, moving, removing, routing, or reassigning a device asks before changing the unit. Changes are temporary until saved; Discard Changes… restores the stored preset. Re-routing also warns that splitting or rejoining a row can interrupt audio."
      }
    ]
  },
  {
    id: "presets-and-workspaces",
    title: "Presets, saving, and backups",
    summary: "The Preset menu holds two different kinds of saving: a file on this computer, and a write to the unit.",
    blocks: [
      {
        kind: "paragraph",
        text: "Preset > Preset Library… opens the unit's directory of folders, banks, and presets. Pick one to recall it. The BANK UP and BANK DOWN switches step through the same presets one at a time, and they ask before they move if the loaded preset has unsaved edits. Performance > Previous Preset and Next Preset are grayed out while there are unsaved edits, so save or discard first. Recalling a preset from the library, and the [ and ] keys, do not ask: unsaved edits are gone the moment the next preset loads."
      },
      {
        kind: "steps",
        items: [
          "Choose Preset > Save to QC…, or press Ctrl+Shift+S, or click the save icon on the screen.",
          "Type the name you want. The field takes your keyboard, and the keys drawn under it are there for letters and spaces if you would rather click. A preset that already has a name starts with it filled in.",
          "Confirm the save. QC Remote writes the preset to the slot it was loaded from.",
          "If the active preset changed while the panel was open, the save stops and asks you to open it again for the preset you meant."
        ]
      },
      {
        kind: "paragraph",
        text: "Preset > Save Preset, Save Preset As…, and Open Preset… work on a workspace file on this computer instead. A workspace records the preset as QC Remote currently sees it, along with the selected device and which unit it came from. Opening one shows you what it holds and never writes any of it to the Quad Cortex, so it is a reference copy rather than a restore."
      },
      {
        kind: "list",
        items: [
          "Preset > Rename Current Preset… asks for a name and then writes it over the stored preset name.",
          "Preset > Device Backup asks for a name and a location, then pulls a native backup off the unit, which takes around forty seconds.",
          "Edit > Copy Preset and Edit > Paste Preset copy a whole preset into another slot, overwriting whatever was stored there.",
          "Edit > Copy Setlist and Edit > Paste Setlist use the active device's native setlist duplicate when both commands target the same Quad Cortex or DEMO library. Copying from a Quad Cortex also reads every occupied preset and all eight scenes, then restores the preset and scene you started on. Switch to DEMO and Paste Setlist recreates those presets in their original slots. Model-based devices retain their parameters, scene values, bypass, expressions, lane controls, routing mutes, names, tempo, and assignments. Neural Captures and IRs cannot run in DEMO, so the result reports and leaves out those blocks rather than pretending they were copied.",
          "Edit > Copy Preset as Text puts the current preset on the clipboard as text, reading each device's values for the active scene first. Edit > Paste Preset from Text puts it back: into the demo device it replaces the current preset, and onto a Quad Cortex it clears the preset and adds each device and value one write at a time, then reports anything the unit refused. A copy from the demo device writes every parameter value for the active scene, replacing the unit's own; values never changed on the demo are approximations (the model's known factory default where available, a guess where not), and the paste says so before you confirm it. Bypass states, footswitch assignments, scene names, routing and tempo are not written to the hardware, and nothing is stored until you save.",
          "Preset > Export Preset Library is listed but stays disabled, because the raw-file transfer it needs is not enabled yet."
        ]
      },
      {
        kind: "note",
        text: "Saving to the unit, renaming a preset, and pasting a preset each overwrite something stored on the hardware, and none of them can be undone from this window. Each of them names that slot and that preset in the question it asks. Read the name before you answer."
      }
    ]
  },
  {
    id: "ai-assistants",
    title: "Desktop AI applications",
    summary: "The LOCAL panel pairs an AI application on this computer with QC Remote, under an access level you choose.",
    blocks: [
      {
        kind: "paragraph",
        text: "Click LOCAL in the menu bar to open Pair an installed desktop AI app to control QC Remote. It lists Claude Desktop, ChatGPT / Codex, Google Antigravity, Bionic, Jan, and a row for any other MCP application. The Bionic row says LM Studio instead if this computer still has that application's older build. Each row says whether that application is installed, whether QC Remote is paired with it, and whether it is connected right now. A paired application that is not connected at this moment is normal, because most of them start their connection when you start a conversation."
      },
      {
        kind: "steps",
        items: [
          "Open the LOCAL panel and find the row for your application.",
          "Press the row's button. Pair writes the connection into that application's own configuration, Update repairs one that has drifted, and an Install button named for the application opens its download page when it is missing.",
          "Set the access level on the panel header, or in Edit > Settings…, before you hand an assistant anything.",
          "Open the application and start a conversation. Once an application is paired and up to date, its row in the LOCAL panel has an Open button."
        ]
      },
      {
        kind: "list",
        items: [
          "Read-only lets an assistant inspect the unit and change nothing.",
          "Performance adds the buttons, the master volume, and the tempo.",
          "Modify adds the Grid, presets, and scenes.",
          "Full control also permits system operations.",
          "Confirmation gates apply at every level. The level sets the most an assistant may attempt; persistent and risky actions still need an explicit confirmation.",
          "A deletion is confirmed once, in the AI conversation that asked; QC Remote never asks again."
        ]
      },
      {
        kind: "paragraph",
        text: "For an application that is not in the list, press Copy STDIO setup on the Other MCP app row of the LOCAL panel. That copies a standard connection block to the clipboard. Paste it into the application's own MCP configuration and enable QC Remote there. This row hands over the configuration and does nothing else, because QC Remote does not know where an application it has never heard of keeps its settings."
      },
      {
        kind: "note",
        text: "Unpairing asks first, and states its own limit: QC Remote removes only its own local connection and leaves that application's other MCP servers alone. Claude Desktop is the exception, because Claude owns its extensions. QC Remote opens Claude's extension page with QC Remote selected, and you remove it there."
      }
    ]
  },
  {
    id: "remote-access",
    title: "Remote access",
    summary: "The REMOTE panel pairs this computer with an account over a relay, so an AI application somewhere else can reach the same controls.",
    blocks: [
      {
        kind: "paragraph",
        text: "Click REMOTE in the menu bar. Its QR code and link pair this Quad Cortex host with Remote MCP Cloud Relay through your Google account. The combined panel shows the paired identity, the connector address, and every AI app authorized to use it. Unpair removes this host from the relay; Unlink revokes one AI app's connector authorization."
      },
      {
        kind: "steps",
        items: [
          "Open the REMOTE panel and scan the QR code, or open the link and sign in there.",
          "Share the Remote MCP Connector from the same panel.",
          "Add that address to the remote AI application and sign in with the same Google account.",
          "Leave QC Remote running on this computer. It is the only thing that can reach the unit."
        ]
      },
      {
        kind: "paragraph",
        text: "A remote assistant is held to the same access level and confirmation rules as a local one. For persistent and risky actions, the assistant must send an explicit confirmation after describing the exact target and getting your answer in the AI conversation. That answer is the only confirmation, deletions included: QC Remote does not ask again, locally or remotely. It checks the access level, allowed actions and exact target first. Text from the unit is treated as data, never as instructions."
      },
      {
        kind: "note",
        text: "Unpair removes only this computer from the relay. Every paired device holds its own credential against the same account, so other paired devices keep their remote access until each one is unpaired where it runs."
      }
    ]
  },
  {
    id: "keyboard-and-mouse",
    title: "Keyboard and mouse",
    summary: "The Grid and performance keys work whenever the window has focus and nothing is being typed into.",
    blocks: [
      {
        kind: "paragraph",
        text: "These keys act on the Grid and on the unit. The Grid and performance keys are suspended while a text field, a menu, a dialog, or the preset browser has focus, so typing a preset name never presses a footswitch. Ctrl+S, Ctrl+Shift+S and Escape keep working wherever you are. Menu entries that have a shortcut show it beside the entry."
      },
      { kind: "shortcuts", rows: KEYBOARD_SHORTCUTS },
      {
        kind: "paragraph",
        text: "On the chassis, most controls answer to the same three gestures: click to press, drag or scroll to turn, and the arrow keys to turn in steps once the control has focus."
      },
      { kind: "shortcuts", rows: MOUSE_INTERACTIONS },
      {
        kind: "note",
        text: "Ctrl+Shift+S opens Save to QC, which names what it is about to write; Delete removes the selected block straight away, and Ctrl+Z puts it back. Escape closes what is open: a pending parameter edit and the block editor together, or the open dialog, the preset browser and the route picker together. A menu or a status panel closes on its own Escape, and the focus returns to the badge that opened it."
      }
    ]
  },
  {
    id: "settings",
    title: "Settings and the window",
    summary: "Edit > Settings… holds the application-wide choices, the tray icon keeps QC Remote reachable with the window closed, and the View menu decides how the chassis is drawn.",
    blocks: [
      {
        kind: "paragraph",
        text: "Edit > Settings… opens the Settings window. Device model sets the chassis drawn on screen. AI access level, also on the LOCAL panel header, applies to every paired assistant at once and is remembered. Show tray icon keeps MCP access available with this window closed. Release QC after last client releases the unit a set number of minutes after the last client disconnects; it needs the tray icon."
      },
      {
        kind: "paragraph",
        text: "Internet for the Quad Cortex, on by default, lets the unit use this computer's internet connection to reach Neural DSP's servers (neuraldsp.com addresses, plus any extra domain you add) for updates while it is connected over USB; its serial number and its own credentials go with its requests."
      },
      {
        kind: "paragraph",
        text: "The tray menu shows the unit's status, local MCP endpoint, Connect or Disconnect, active and linked clients, Release QC after last client: …, and Quit. Unlink pauses a local app's listing until its next tool call; remote Unlink asks for confirmation and revokes only that app's authorization. Quit releases the unit and closes the tray icon and the background MCP host; nothing restarts them until a user action, such as opening QC Remote."
      },
      {
        kind: "list",
        items: [
          "View > Fit to Window scales the chassis to the window, which is how it starts.",
          "View > Actual Size draws it at an approximation of its physical width.",
          "View > Full Screen fills the display, and choosing it again returns the window.",
          "Performance > Tempo… asks for a whole number of beats per minute from 40 through 240.",
          "Performance > I/O Settings, Tuner, Looper X, and Gig View open those screens on the unit."
        ]
      },
      {
        kind: "paragraph",
        text: "The menu bar carries three badges at its right end, each opening a panel: LOCAL for desktop AI applications, REMOTE for relay access, and the device badge for the unit. Escape closes the open panel and returns the focus to its badge."
      },
      {
        kind: "note",
        text: "Settings that live on the unit stay on the unit. Everything else QC Remote shows about your rig is read from the Quad Cortex when it connects, so no second copy can get out of date."
      }
    ]
  },
  {
    id: "troubleshooting",
    title: "When something is not working",
    summary: "Most problems are one of three things: nothing has the unit, something else has the unit, or the session went stale.",
    blocks: [
      {
        kind: "list",
        items: [
          "The badge says Searching for Quad Cortex. Check the cable and that the unit is switched on, then press Connect in the device panel.",
          "The badge says Offline or Disconnected. Close any other application that talks to the unit, and press Connect.",
          "The badge is green but commands keep failing. Disconnect, then Connect to establish a fresh device session.",
          "A menu entry is grayed out. Most of them need a ready device, and a few also need the loaded preset to have no unsaved edits."
        ]
      },
      {
        kind: "steps",
        items: [
          "Choose Help > Report an Issue… for a bug, or Help > Send Feedback… for an improvement. Either opens the same Feedback window, on Report a bug or Suggest an improvement; you can switch between the two there.",
          "Read the What to include list under the one you picked.",
          "Press Open GitHub issue…. QC Remote opens the issue form in your browser. For a bug it fills in the version, your Windows version, an outline to describe what happened in, and the redacted diagnostics, which travel to GitHub in the link itself. Nothing is posted until you submit the form.",
          "If the diagnostics are too long for the link, the window says so and the diagnostics field is left empty: press Copy diagnostics and paste them there. Export redacted diagnostics… saves the same report as a file."
        ]
      },
      {
        kind: "paragraph",
        text: "The diagnostics report is a snapshot of state rather than a record of your playing. It carries when it was generated, the application version, the platform, whether the device gateway is available, the connection phase, the location and position of the loaded preset, its mode, scene, tempo, block count, and whether it had unsaved edits, and the list of connection events with their names and details."
      },
      {
        kind: "note",
        text: "The report names no serial numbers, no MAC addresses, no usernames, no file paths, and no preset or setlist names. Device info… shows the serial number and software versions the Quad Cortex reports about itself on this computer only; they are never included in a report, and cloud account identifiers are never read or displayed."
      }
    ]
  },
  {
    id: "privacy-and-safety",
    title: "Privacy, safety, and the legal notices",
    summary: "Where the privacy policy is, what the About window holds, and a safety note for the unit itself.",
    blocks: [
      {
        kind: "paragraph",
        text: "What stays on this computer, what the optional remote relay receives and keeps and for how long, and how to delete it are stated once, in the privacy policy. This guide does not repeat them, so it cannot disagree with them. Read the policy before you pair a relay."
      },
      {
        kind: "list",
        items: [
          "Help > About QC Remote… opens the About window, whose tabs are Privacy, Legal, Notices, and About.",
          "The Privacy tab opens the complete privacy policy.",
          "The Legal tab carries the independence and trademark statements, the publisher's details, and the contact addresses.",
          "The Notices tab lists the open-source components each build includes, with the locked dependency notices and a machine-readable license inventory."
        ]
      },
      {
        kind: "note",
        text: "QC Remote works through the unit's own interface and contains no Quad Cortex firmware. Review what you are about to change before you change it, keep output levels safe when the unit is plugged into an amplifier and somebody is standing in front of it, and keep independent backups of the presets you would hate to lose."
      }
    ]
  }
];
