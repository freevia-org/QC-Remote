// The Neural Capture wizard's five connection steps.
//
// CorOS 4.1.0 shows one rear-panel diagram five times, highlighting different
// jacks and changing the caption, and it asks for CAPTURE OUT into the target
// device with the target's output into INPUT 2 - Send/Return is what step 1
// asks you to *disconnect*.

import type { ReactNode } from "react";
import { QcIoIcon, QcScreenGlyph, QcUiIcon, type QcIoIconName } from "./theme-icons";
import type { QcIoJackState } from "./qc-io-jack";
import "./qc-io-jack.css";
import "./coros-capture-connections.css";

export type CaptureConnectionView =
  | "capture-intro"
  | "capture-monitoring"
  | "capture-connect-out"
  | "capture-connect-input-2"
  | "capture-routing";

type JackKind = "usb" | "trs" | "din" | "xlr" | "input";

// Which ink a port is drawn in is a property of the port, not of the screen.
// The unit substitutes a flat colour per state rather than fading one:
//
//   idle      contextCaptureMuted  the silkscreen grey; the state with no class
//   disabled  unsaved's value      a port this step asks you to unplug (step 1)
//   active    screen accent        the port this step is about
//   plugged   pure white           a port an earlier step already had you connect
//
// The stylesheet names the token each one resolves through.
//
// `capture-connect-input-2` draws INPUT 2 active and INPUT 1, PHONES and
// CAPTURE OUT plugged, which is what the wizard has asked for by then.
// The socket's state vocabulary is QcIoJack's - the same control, drawn a
// second time here because the rear panel's socket is a different drawing
// from io.combo's mark, not a scaled one. `idle` is the one ink the two
// families read differently, and each still states its own.
type JackState = Exclude<QcIoJackState, "idle">;

interface Jack {
  id: string;
  x: number;
  y: number;
  kind: JackKind;
  label?: string;
  labelAbove?: boolean;
  // USB, MIDI OUT and MIDI IN have no port beneath them, and the panel does not
  // hang their names off the port: it drops each name to the lower silkscreen
  // row shared with EXP 1 and joins it to the port with a 2x44 rule in the same
  // ink the disabled state uses.
  leader?: boolean;
}

// Positions are the device's own pixels in its 800x480 framebuffer; the
// stylesheet converts them to container units so the panel scales with the
// screen instead of being pinned to one host size.
const JACKS: Jack[] = [
  { id: "usb", x: 41, y: 207, kind: "usb", label: "USB", leader: true },
  { id: "exp2", x: 98, y: 207, kind: "trs", label: "EXP 2", labelAbove: true },
  { id: "exp1", x: 98, y: 269, kind: "trs", label: "EXP 1" },
  { id: "midi-out", x: 156, y: 207, kind: "din", label: "MIDI\nOUT", leader: true },
  { id: "midi-in", x: 214, y: 207, kind: "din", label: "MIDI\nIN", leader: true },
  { id: "out2", x: 279, y: 224, kind: "xlr", label: "OUT 2" },
  { id: "out1", x: 344, y: 224, kind: "xlr", label: "OUT 1" },
  { id: "phones", x: 418, y: 207, kind: "trs" },
  { id: "capture-out", x: 418, y: 269, kind: "trs", label: "CAPTURE\nOUT" },
  { id: "out4", x: 487, y: 207, kind: "trs", label: "OUT 4/R", labelAbove: true },
  { id: "out3", x: 487, y: 269, kind: "trs", label: "OUT 3/L" },
  { id: "ret2", x: 550, y: 207, kind: "trs", label: "RET 2", labelAbove: true },
  { id: "ret1", x: 550, y: 269, kind: "trs", label: "RET 1" },
  { id: "send2", x: 610, y: 207, kind: "trs", label: "SEND 2", labelAbove: true },
  { id: "send1", x: 610, y: 269, kind: "trs", label: "SEND 1" },
  { id: "input2", x: 677, y: 224, kind: "input", label: "INPUT 2" },
  { id: "input1", x: 749, y: 224, kind: "input", label: "INPUT 1" }
];

interface Annotation {
  x: number;
  y: number;
  text: string;
  direction: "up" | "down" | "down-above";
}

interface Step {
  caption: ReactNode;
  jacks: Partial<Record<string, JackState>>;
  crossed?: string[];
  annotations?: Annotation[];
  footer?: ReactNode;
  warning?: boolean;
}

const STEPS: Record<CaptureConnectionView, Step> = {
  "capture-intro": {
    caption: <>Plug your instrument into <b>Input 1</b></>,
    // The four struck ports are drawn a step darker than the rest of the panel,
    // not merely crossed: the disabled ink against the idle grey every other
    // port carries. The strike itself stays at the idle grey.
    jacks: { input1: "active", ret1: "disabled", ret2: "disabled", send1: "disabled", send2: "disabled" },
    crossed: ["ret1", "ret2", "send1", "send2"],
    annotations: [{ x: 749, y: 336, text: "Instrument", direction: "up" }],
    footer: <>For best results, please disconnect any devices<br />from <b>Send 1</b>, <b>Send 2</b>, and <b>Return 1</b>, <b>Return 2</b></>
  },
  "capture-monitoring": {
    caption: <>Connect your <b>headphones</b> and/or use Out 1 &amp; 2 or Out 3 &amp; 4 for monitoring.</>,
    jacks: { phones: "active", out1: "active", out2: "active", out3: "active", out4: "active", input1: "plugged" },
    annotations: [{ x: 418, y: 108, text: "Headphones", direction: "down" }]
  },
  "capture-connect-out": {
    caption: <>Connect <b>Capture Out</b> to the input of the target device</>,
    jacks: { "capture-out": "active", phones: "plugged", input1: "plugged" },
    annotations: [{ x: 418, y: 385, text: "To the input of the\ntarget device", direction: "down-above" }]
  },
  "capture-connect-input-2": {
    caption: <>Position your mic in front of the cabinet and connect it to <b>Input 2</b>.<br />If you have an amplifier that has a D.I. Out or a reactive load box<br />you can opt to use that instead of a microphone</>,
    jacks: { input2: "active", phones: "plugged", "capture-out": "plugged", input1: "plugged" },
    annotations: [{ x: 677, y: 116, text: "From\nmic/amp", direction: "down" }],
    warning: true
  },
  "capture-routing": {
    caption: <>Once everything is connected correctly, you can proceed with<br />creating your Neural Capture</>,
    jacks: { input1: "plugged", input2: "plugged", "capture-out": "plugged", phones: "plugged" },
    annotations: [
      { x: 677, y: 116, text: "From\nmic/amp", direction: "down" },
      { x: 749, y: 336, text: "Instrument", direction: "up" },
      { x: 418, y: 385, text: "To the input of the\ntarget device", direction: "down-above" }
    ]
  }
};

const ORDER: CaptureConnectionView[] = [
  "capture-intro",
  "capture-monitoring",
  "capture-connect-out",
  "capture-connect-input-2",
  "capture-routing"
];

function jackIcon(kind: JackKind): QcIoIconName {
  return kind === "usb" ? "usb" : kind === "din" ? "midi" : kind === "xlr" ? "combo" : kind === "input" ? "input" : "jack";
}

export function CorOsCaptureConnections({ view }: { view: CaptureConnectionView }) {
  const step = STEPS[view];
  const index = ORDER.indexOf(view);
  return <section className={`qc-screen coros-capture-connections is-${view}`} aria-label={view.replaceAll("-", " ")}>
    <header>
      <button className="capture-connections-close" aria-label="Close"><QcUiIcon kind="close" /></button>
      <button className="capture-connections-skip" aria-label="Skip"><QcScreenGlyph kind="capture-skip" /></button>
      <span />
      {index > 0 && <button className="capture-connections-back" aria-label="Back"><QcScreenGlyph kind="capture-back" /></button>}
      <button className={`capture-connections-next${index === 0 ? " is-wide" : ""}`} aria-label="Next"><QcScreenGlyph kind="capture-next" /></button>
    </header>
    <p className="capture-connections-caption">{step.caption}</p>
    <div className="capture-connections-panel">
      {JACKS.map((jack) => {
        const state = step.jacks[jack.id];
        return <span
          key={jack.id}
          className={`capture-jack capture-jack-${jack.kind}${state ? ` is-${state}` : ""}${jack.leader ? " has-leader" : ""}${step.crossed?.includes(jack.id) ? " is-crossed" : ""}`}
          style={{ left: `${jack.x / 8}cqw`, top: `${jack.y / 8}cqw` }}
        >
          <QcIoIcon kind={jackIcon(jack.kind)} />
          {jack.label && <small className={jack.labelAbove ? "is-above" : undefined}>{jack.label}</small>}
        </span>;
      })}
      <span className="capture-phones-glyph"><QcIoIcon kind="headphones-symbol" /></span>
      {step.annotations?.map((annotation) => <span
        key={annotation.text}
        className={`capture-annotation is-${annotation.direction}`}
        style={{ left: `${annotation.x / 8}cqw`, top: `${annotation.y / 8}cqw` }}
      >{annotation.text}</span>)}
      {step.warning && <aside className="capture-tube-warning">
        <strong>TUBE AMPLIFIER WARNING</strong>
        <p>Connecting the speaker output from a tube amplifier to<br />Quad Cortex could damage both units. Ensure you are using<br />a D. I. Out and your amplifier is still connected to a cabinet<br />or reactive load box.</p>
        {/* The unit draws a vacuum tube here, and it is a picture rather than a
            geometric mark: an envelope with a pip, a getter bar, two solid plates
            between mica discs and four pins on fanned leads. The viewBox is the
            device crop in device pixels, so the box the stylesheet gives it is
            the crop itself and nothing has to be rescaled. */}
        <svg viewBox="0 0 38 86" aria-hidden="true"><path d="M18.06 1.5L18.5 1.23L19.01 1.5L18.5 2.47ZM17.11 2.5L17.5 1.93L18.48 2.5L18 3.5L17.5 5.75L17.04 5.5L16.97 3.5ZM18.52 2.5L19.5 1.86L19.89 2.5L20 3.5L19.96 5.5L20.5 5.86L20.97 7.5L21 8.5L20.5 9L20.17 8.5L19.95 7.5L19.98 6.5L19.5 5.84L19.03 3.5ZM16.21 6.5L16.5 5.76L17.02 6.5L16.93 8.5L16.5 9.23L15.82 8.5ZM14.99 9.5L15.5 8.82L16.23 9.5L15.5 10.48ZM20.89 9.5L21.5 8.89L22.5 9.99L23.17 10.5L22.5 11.26L21.47 10.5ZM13.89 10.5L14.5 9.99L15.48 10.5L14.5 11.11ZM10.53 11.5L12.5 11.03L13.5 10.99L14 11.5L13.5 11.83L12.5 12.04L11.5 12ZM22.74 11.5L23.5 10.83L25.5 11.17L26.48 11.5L25.5 12.07L23.5 11.93ZM7.98 12.5L8.5 12.17L10.5 11.59L10.75 12.5L9.5 12.97L8.5 13.01ZM26.2 12.5L26.5 11.57L27.5 11.95L28.5 12.12L29.19 12.5L28.5 13.23ZM6.2 13.5L6.5 13.23L7.5 12.89L8.11 13.5L6.61 14.5L5.5 15.48L4.99 14.5ZM28.74 13.5L29.5 12.77L31.5 13.99L33.83 16.5L34.2 17.5L33.5 18.07L32.5 16.61L30.47 14.5ZM3.99 15.5L4.5 14.99L5.48 15.5L4.5 16.48ZM3.27 16.5L3.5 16.22L4.46 16.5L3.5 18.11L2.74 17.5ZM2.07 18.5L2.5 17.74L3.11 18.5L2.88 19.5L2.5 20.32L1.93 19.5ZM33.89 18.5L34.5 17.77L34.93 18.5L35 19.5L35.61 20.5L35.89 21.5L36 23.5V66.5L35.61 68.5L34.5 70.48L33.99 69.5L34.27 68.5L34.93 67.5L35 66.5L35.04 20.5L34.5 20.28ZM9.17 19.5L9.5 19.17H27.5L27.83 19.5L28 20.5L27.5 21.17H9.5L9 20.5ZM1.39 20.5L1.5 20.3L1.96 20.5L2 27.5L2.31 29.5L2 30.5V51.5L2.31 53.5L2 54.5L2.06 67.5L1.5 68.48L1.12 67.5L1 65.5V23.5L1.03 22.5ZM12.17 25.5L12.5 25.17H13.5L13.83 25.5L14.18 28.5L14.5 28.82H16.5L17.5 29L22.5 28.82L22.82 28.5L23.17 25.5L23.5 25.17H24.5L24.83 25.5L25.18 28.5L25.5 28.82H27.5L28.5 29H32.5L33.18 29.5L32.5 29.83H28.5L28 30.5V51.5L28.17 52.5L28.5 53H32.5L33.18 53.5L32.5 53.83H23.5L23 54.5V56.5L22.5 57.47L22.12 56.5L22 54.5L21.5 53.83L21 54.5L20.98 57.5L20.5 58.14L20.12 56.5L20 54.5L19.5 53.83L19 54.5L19.03 69.5L20.01 70.5L20.38 71.5L20.49 72.5L21.5 73.16L22.47 72.5L22.51 71.5L22.99 70.5L23.97 69.5L23.98 66.5L24.5 66.02L24.93 67.5L25.03 69.5L25.5 70.03H26.5L26.97 69.5L26.99 67.5L26.86 66.5L27.5 66.16L27.7 66.5L27.89 67.5L28.03 69.5L29.01 70.5L29.5 71.97L30.1 72.5L30.5 72.68L31 73.5L30.5 73.83L29.5 74L28.62 74.5L28.49 75.5V83.5L27.83 84.5L27.5 84.77L27.12 84.5L26.51 83.5L26.5 75.46L25.49 75.5V83.5L24.83 84.5L24.5 84.77L24.12 84.5L23.51 83.5V75.5L23.12 74.5L22.5 74.11L21.5 74.01L20.5 74.11L19.84 74.5L19.5 75.45L19.49 83.5L18.83 84.5L18.5 84.77L18.12 84.5L17.51 83.5V75.5L17.12 74.5L16.5 74.11L15.5 74.01L14.5 74.11L13.84 74.5L13.5 75.45L13.49 83.5L12.83 84.5L12.5 84.77L12.12 84.5L11.51 83.5L11.5 75.46L10.49 75.5V83.5L9.83 84.5L9.5 84.77L9.12 84.5L8.51 83.5V75.5L8.36 74.5L7.5 74L6.5 73.77L6.07 73.5L6.5 72.89L7.1 72.5L7.99 70.5L8.97 69.5L9.11 67.5L9.27 66.5L9.5 66.05L10.01 66.5L10.03 69.5L10.5 70.03H11.5L11.97 69.5L12.21 66.5L12.5 65.87L13.1 66.5L13.03 69.5L14.01 70.5L14.38 71.5L14.49 72.5L15.5 73.16L16.47 72.5L16.51 71.5L16.99 70.5L17.97 69.5L18 54.5L17.5 53.83L17 54.5L16.76 57.5L16.5 58.06L15.9 57.5L16 54.5L15.5 53.83L15 54.5L15.01 56.5L14.5 57.28L13.99 56.5L14 54.5L13.5 53.83L3.5 53.83L2.82 53.5L3.5 53L8.5 52.99L8.82 52.5L9 51.5L9 50.5V30.5L8.5 29.83L3.5 29.83L2.82 29.5L3.5 29L8.5 28.99L9.5 28.82H11.5L11.82 28.5ZM17 51.5L17.17 52.5L17.5 52.99L18.5 52.82L19.5 52.98L19.82 52.5L20 51.5L20 30.5L19.5 29.83L17.5 29.83L17 30.5ZM13.27 57.5L13.5 57.12L14.17 57.5L13.83 59.5L13.5 60L12.82 59.5ZM22.59 57.5L23.5 57.25L23.61 57.5L23.97 58.5L24.14 59.5L23.5 60L23.14 59.5L22.95 58.5ZM15.22 58.5L15.5 57.95L16.01 58.5L16.04 59.5L15.83 60.5L15.5 61.14L14.82 60.5ZM20.86 58.5L21.5 58.02L21.75 58.5L22 59.5L22.07 60.5L21.5 61L21.17 60.5ZM12.07 60.5L12.5 59.74L13.11 60.5L12.5 61.75L11.98 61.5ZM23.89 60.5L24.5 59.77L25.48 61.5L24.5 61.94L24.17 61.5ZM14.17 61.5L14.5 60.82L15.15 61.5L14.93 62.5L14.5 63.23L13.95 62.5ZM21.99 61.5L22.5 60.93L22.83 61.5L23.11 62.5L22.5 63.48L22.07 62.5ZM10.99 62.5L11.5 61.74L12 62.5L11.7 63.5L11.5 63.75L10.55 63.5ZM24.91 62.5L25.5 61.51L26.45 63.5L25.5 63.75ZM13.27 63.5L13.5 63.17L14.01 63.5L14.07 64.5L13.5 66.06L12.9 65.5L12.92 64.5ZM22.53 63.5L23.5 63.07L23.77 63.5L24.14 65.5L23.5 66.18L23.17 65.5L23.04 64.5ZM10.07 64.5L10.5 63.52L11 64.5L10.83 65.5L10.5 66.14L9.92 65.5ZM25.95 64.5L26.5 63.51L27 64.5L27 65.5L26.5 66.28L26.17 65.5ZM1.95 69.5L2.5 68.51L3.5 70.2L4.5 71.3L6.26 72.5L5.5 73.11L4.5 72.61L3.17 71.5L2.39 70.5ZM33.23 70.5L33.5 70.16L34.47 70.5L33.88 71.5L33.5 71.91L32.5 72.61L31.5 73.11L30.62 72.5L32.28 71.5Z" /></svg>
      </aside>}
    </div>
    {step.footer && <p className="capture-connections-footer">{step.footer}</p>}
    <nav className="capture-connections-dots" aria-hidden="true">
      {ORDER.map((id, dot) => <i key={id} className={dot === index ? "is-active" : ""} />)}
    </nav>
  </section>;
}
