import type { MidiOutMessage } from "@qc-remote/client";

/** A MIDI Out source or the preset-load list holds this many message slots (three pages of four rows). */
export const MIDI_OUT_SLOT_COUNT = 12;

/** An empty slot: wire type 0 (None). Its other values are not sent. */
export const EMPTY_MIDI_OUT_MESSAGE: MidiOutMessage = { type: 0, channel: 1, param1: 0, param2: 0, param3: 0 };

const isActive = (message: MidiOutMessage) => message.type >= 1 && message.type <= 3;

/** A published list as the editor's twelve rows: each message at its own index, the rest empty. */
export function midiOutSlots(messages: readonly MidiOutMessage[] | undefined): MidiOutMessage[] {
  return Array.from({ length: MIDI_OUT_SLOT_COUNT }, (_, index) => ({ ...EMPTY_MIDI_OUT_MESSAGE, ...(messages?.[index] ?? {}) }));
}

export const hasActiveMidiOut = (messages: readonly MidiOutMessage[]) => messages.some(isActive);

const integerWithin = (value: number, minimum: number, maximum: number) =>
  Number.isInteger(value) && value >= minimum && value <= maximum;

const VALUE_NAMES = { param1: "first", param2: "second", param3: "third" } as const;

/**
 * The rows to send for a list: all twelve, in the order shown, with every empty
 * row kept as a type 0 placeholder so no message moves. Observed on a CorOS
 * 4.1.0 unit, 2026-10-07: the unit preserves message positions in the order
 * sent, a type 0 row between two active rows keeps its index, and an empty
 * list clears the source. A short list is padded with empties; a longer one is
 * refused, not truncated. A row with an undefined type or an out-of-range value
 * is refused here rather than left to the device: the unit once restarted after
 * an undefined type.
 */
export function midiOutRows(messages: readonly MidiOutMessage[]): MidiOutMessage[] {
  if (messages.length > MIDI_OUT_SLOT_COUNT) {
    throw new Error(`MIDI Out holds at most ${MIDI_OUT_SLOT_COUNT} messages, not ${messages.length}.`);
  }
  const rows = midiOutSlots(messages);
  rows.forEach((row, index) => {
    const label = `Message ${index + 1}`;
    if (!integerWithin(row.type, 0, 3)) throw new Error(`${label}: type ${row.type} is not defined (use 0 to 3).`);
    if (row.type === 0) return;
    if (!integerWithin(row.channel, 1, 16)) throw new Error(`${label}: channel must be 1 to 16.`);
    for (const field of ["param1", "param2", "param3"] as const) {
      if (!integerWithin(row[field], 0, 127)) throw new Error(`${label}: ${VALUE_NAMES[field]} value must be 0 to 127.`);
    }
  });
  return rows.map((row) => row.type === 0 ? { ...EMPTY_MIDI_OUT_MESSAGE } : { ...row });
}
