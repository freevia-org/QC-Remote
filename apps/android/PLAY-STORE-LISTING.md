# QC Remote Android — Google Play listing draft

Prepared 29 October 2026 against the public v1.0.4 Android release. This file
is a reviewable source for a **listing-only** English (US) full-description
update. It does not change the app, release tracks or Play Console.

The current short description is already concise and accurate; keep it:

> Control your Quad Cortex guitar or bass rig, with guarded AI access via MCP.

## Proposed full description

```text
See and shape your Quad Cortex guitar or bass rig from your Android phone. QC Remote shows the live signal chain, presets, scenes and performance controls in one place. You can also give a compatible AI app structured access to the rig through a remote MCP connector.

CONTROL YOUR RIG
• Connect a compatible Quad Cortex over USB to view and edit the Grid, blocks, routing and parameters.
• Browse the preset library, switch scenes and use footswitch and performance controls.
• Save a preset to the device when you are ready. Changes to a loaded preset are not saved automatically.
• Explore a local demo device without hardware; its presets stay on your phone.

WORK WITH AI
• Pair a compatible remote MCP connector while your Quad Cortex is connected.
• Choose Read-only, Performance, Modify or Full access, and unpair in the app when you want to stop remote access.
• QC Remote provides controls and status, not an AI chat app. Choose an AI client that asks for your approval before it sends a destructive request, such as deleting a preset or setlist. QC Remote does not add a second approval prompt.

Hardware control requires a compatible Quad Cortex, a USB cable or adapter, and an Android phone with USB host support. Remote AI access also needs pairing and an internet connection. AI apps and their subscriptions are separate.

QC Remote is independent, unofficial software by Freevia. It is not affiliated with, authorized, sponsored, endorsed or supported by Neural DSP Technologies Oy. Neural DSP and Quad Cortex are trademarks of Neural DSP Technologies Oy and are named only to identify compatibility.
```

## Evidence and Console scope

- `apps/android/README.md` documents the shared Grid, routing, scene, preset,
  parameter, footswitch, save and performance workflows, USB host requirement,
  local demo, remote pairing and four access tiers. It also confirms there is
  no built-in chat or speech interface.
- `docs/USER-GUIDE.md` describes preset browsing, saving, scene and performance
  controls, and the client-side approval boundary for destructive AI requests.
- The live [English (US) Play listing](https://play.google.com/store/apps/details?id=org.freevia.qcremote)
  already has the short description above and six real screenshots. The
  connected-device Grid and parameter screenshots lead. This draft changes
  only the full description; no image or tag change is proposed.
- Before copying into Console, confirm the public Android build still exposes
  these workflows and that the publishing overview contains no conflicting
  release or listing changes. Submit only an English (US) full-description
  change after release coordination.
