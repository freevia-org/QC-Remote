import "./qc-io-jack.css";
import { QcScreenVectorGlyph } from "./theme-icons";

/**
 * The CorOS rear-panel socket, drawn once with its state as a parameter.
 *
 * The I/O pages draw the same twelve-socket rear panel throughout, and the
 * only thing that changes between them is which socket is lit. Every socket
 * is one two-layer drawing: a dark body the unit never recolours, and a metal
 * layer whose ink is the state.
 *
 * THE BOXES. The asset box is the registry entry's own, and the ink sits
 * inside it at a fixed offset:
 *
 *   kind     registry entry   asset box   ink      ink at      sockets
 *   usb      io.usb           48 x 48     38 x 38  (6, 3)      1
 *   jack     io.jack          48 x 48     43 x 43  (3, 3)      14
 *   midi     io.midi          48 x 48     46 x 46  (1, 1)      2
 *   combo    io.combo         56 x 56     56 x 56  (0, 0)      2
 *   input    io.input         68 x 68     64 x 64  (2, 4)      2
 *
 * THE STATES. Each is one ink on the metal layer. The body is
 * `reference.screen` in every state, and every ink below is a role, not a
 * literal:
 *
 *   idle      reference.routeGlyphSurface  every unselected socket
 *   active    reference.rotaryAccent       the socket a screen is drawing attention to
 *   plugged   reference.primaryText        a socket already connected
 *   disabled  a fourth state used by the capture panel, where a crossed-through
 *             socket inks var(--qc-palette-313031)
 *
 * `disabled` is declared here because it belongs to this drawing even though
 * the I/O pages never use it; a screen that never shows it never asks for it.
 *
 * Idle and plugged need no rule at all. The registry already inks each entry's
 * metal layer with the role it uses by default, and those two roles are
 * exactly the two inks: io.usb's metal is `reference.primaryText`, the white
 * plate; every other entry's metal is `reference.routeGlyphSurface`, the idle
 * grey. Only `active` and `disabled` restate a layer.
 *
 * THE LIT INPUT'S RIM. The unit draws an accent rim on the outer edge of the
 * selected input's body, at radii 30..32 from the socket's own centre, where
 * the unlit socket has no metal at that radius at all. Its centroid radius is
 * 31.46 and its area gives a width of 0.918, both in the 68-unit asset box
 * about the ink centre (33.5, 35.5). It is a property of this kind in this
 * state only, not a selection ring drawn for every socket.
 *
 * WHAT IS NOT SHARED. io.combo is two drawings, and this does not merge them.
 * The rear panel's 52px socket in the capture family has proportionally
 * thicker strokes and a wider pin than a scaled 56px io.combo mark would.
 * Sharing one geometry across both would break the I/O family to fix the
 * capture one. Sharing here means one component with a `kind` parameter, not
 * one path forced to serve two sizes.
 *
 * Nothing sets shapeRendering: the registry entries carry their own, and every
 * one of these is a sub-pixel contour that crispEdges would snap off its row.
 * Colour is a role throughout; no ink below is a literal.
 */

/** Which rear-panel socket. Each name is the registry entry it draws. */
export type QcIoJackKind = "usb" | "jack" | "midi" | "combo" | "input";

/**
 * What the unit is saying about the socket.
 *
 * The capture panel names the same four, and used to declare them again as its
 * own `JackState` with its own copy of the inks. It imports this one now. The
 * vocabulary is shared; the drawing is not, and must not be - the rear panel's
 * socket in the capture family is proportionally thicker than a scaled
 * io.combo mark, so that screen keeps its own geometry. Sharing a state model
 * across two drawings of one control is the opposite of sharing one path
 * across two sizes.
 */
export type QcIoJackState = "idle" | "active" | "plugged" | "disabled";

/** Asset box of each entry, in the registry's own units. */
export const QC_IO_JACK_BOX: Record<QcIoJackKind, number> = { usb: 48, jack: 48, midi: 48, combo: 56, input: 68 };

/**
 * The accent rim on the selected input, in io.input's 68-unit box: centre,
 * centroid radius and the width its area gives. Drawn only for the input
 * kind in its active state; no other kind or state carries a rim.
 */
export const QC_IO_INPUT_RIM = { cx: 33.5, cy: 35.5, r: 31.46, width: 0.918 };

export type QcIoJackProps = {
  kind: QcIoJackKind;
  /** Defaults to the unlit socket. */
  state?: QcIoJackState;
  className?: string;
  /** Accessible name, where the socket is the control rather than decoration. */
  label?: string;
};

export function QcIoJack({ kind, state = "idle", className, label }: QcIoJackProps) {
  const box = QC_IO_JACK_BOX[kind];
  const rim = kind === "input" && state === "active";
  return (
    <svg
      className={`qc-io-jack${className ? ` ${className}` : ""}`}
      data-kind={kind}
      data-state={state}
      viewBox={`0 0 ${box} ${box}`}
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <QcScreenVectorGlyph icon={`io.${kind}`} />
      {rim && <circle className="qc-io-jack-rim" cx={QC_IO_INPUT_RIM.cx} cy={QC_IO_INPUT_RIM.cy} r={QC_IO_INPUT_RIM.r} strokeWidth={QC_IO_INPUT_RIM.width} />}
    </svg>
  );
}
