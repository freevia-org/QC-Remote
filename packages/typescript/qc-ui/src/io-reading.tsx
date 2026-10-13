/** The unit draws a reading's unit ("dB") smaller and dimmer than its number: "-40.0 dB" is two runs of text. */
export function IoReading({ text }: { text: string }) {
  const parts = /^(.*\S)\s+(dB)$/.exec(text);
  return parts ? <>{parts[1]} <small>{parts[2]}</small></> : <>{text}</>;
}
