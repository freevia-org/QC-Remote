import "./midi-out-art.css";

/**
 * The two pieces of art on the unit's Preset MIDI Out screen that are not
 * ordinary controls, drawn once here so the live editor and the fixture show
 * the same shapes. Both are measured from a 4.1.0 frame: every coordinate is
 * a pixel of the unit's 800x480 screen, and each SVG's viewBox is the pixel
 * box it occupies there, so one SVG unit is one device pixel at any scale.
 */

/**
 * The delete glyph in the header. The unit draws it as a thin, two-pixel
 * outline, not the Directory's heavy trash mark, in the screen's one dim
 * green-grey while there is nothing to clear. 20x24 at (633, 18).
 */
export function QcMidiOutTrashIcon() {
  return <svg className="qc-midi-out-trash-icon" viewBox="0 0 20 24" aria-hidden="true" focusable="false">
    <path className="qc-midi-out-trash-fill" d="M7.4 0.4H12.6Q13 0.4 13 1V2H7V1Q7 0.4 7.4 0.4Z" />
    <rect className="qc-midi-out-trash-fill" x="0" y="2" width="20" height="2" />
    <path className="qc-midi-out-trash-stroke" d="M2 7H18V19Q18 23 14 23H6Q2 23 2 19Z" />
    <path className="qc-midi-out-trash-stroke qc-midi-out-trash-cross" d="M7.5 12.4L12.5 17.6M12.5 12.4L7.5 17.6" />
  </svg>;
}

/**
 * An expression pedal: a dark, rounded outline that narrows toward the heel,
 * a lighter treadle plate inside it, and the two groups of grip ribs - seven
 * across the toe, five across the heel. 108x260 at (522, 139); the second
 * pedal is the same art 146 px to the right.
 */
export function QcMidiOutExpressionPedal() {
  return <svg className="qc-midi-out-pedal" viewBox="522 139 108 260" aria-hidden="true" focusable="false">
    <path className="qc-midi-out-pedal-body" d="M535 141.2H617C622.5 141.3 627.6 147 627.6 158L626.5 200L624 240L622 300L620.7 340L618.6 380C618.1 389 616 396.3 607 396.3H545C536 396.3 533.9 389 533.5 380L531.3 340L530 300L528 240L526.5 200L524.4 160L524.4 158C524.4 147 529.5 141.3 535 141.2Z" />
    <path className="qc-midi-out-pedal-plate" d="M539 163H613Q619 163 619 169V195L609.8 383Q609.6 389 603.4 389H548.6Q542.4 389 542.2 383L533.2 195V169Q533.2 163 539 163Z" />
    <g className="qc-midi-out-pedal-ribs">
      {[0, 1, 2, 3, 4, 5, 6].map((rib) => <rect key={rib} x="544" y={175 + rib * 10} width="64" height="4" />)}
      {[0, 1, 2, 3, 4].map((rib) => <rect key={rib} x="549" y={335 + rib * 10} width="54" height="4" />)}
    </g>
  </svg>;
}

/** What one footswitch / expression source button reports when pressed: the editor's source index, or the preset-load list. */
type MidiOutSourceChoice = number | "preset";

/**
 * The assignment surface: the footswitch panel with the preset-load box and the
 * eight A-H switches, and the two expression pedals. The live editor passes the
 * state it owns; the fixture passes nothing and draws the same screen.
 */
export function QcMidiOutAssignmentSurface({ pending = false, onChoose }: { pending?: boolean; onChoose?(source: MidiOutSourceChoice): void }) {
  const labels = ["A", "B", "C", "D", "E", "F", "G", "H"];
  return <div className="midi-assignment-surface">
    <section className="midi-footswitches" aria-label="Footswitch MIDI assignments">
      <button type="button" className="midi-preset-load" disabled={pending} onClick={() => onChoose?.("preset")}>ON PRESET LOAD<br />MESSAGE</button>
      {labels.map((label, index) => <button type="button" className="midi-switch" key={label} disabled={pending} onClick={() => onChoose?.(index)}><i />{label}</button>)}
    </section>
    <section className="midi-expression" aria-label="Expression MIDI assignments">
      {[8, 9].map((index) => <button type="button" className="midi-expression-source" key={index} aria-label={`Edit EXP ${index - 7} MIDI assignment`} disabled={pending} onClick={() => onChoose?.(index)}><span>EXP {index - 7}</span><QcMidiOutExpressionPedal /></button>)}
    </section>
  </div>;
}
