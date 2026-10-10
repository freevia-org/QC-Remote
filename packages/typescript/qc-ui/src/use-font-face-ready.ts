import { useEffect, useState } from "react";

/**
 * True once `document.fonts.load(spec, text)` has settled for exactly this font
 * spec and text, so a component that sizes text from `canvas.measureText` in its
 * render body re-renders - and so re-measures - after the face it measures with
 * is usable. Canvas measures with a fallback face until the real one has loaded;
 * a measurement taken before that is otherwise never repeated.
 *
 * The result is keyed on spec + text, so changed text re-arms it (one more load
 * of an already cached face, then one more render) and nothing loops. A missing
 * font API, a server render, or a rejected load leaves it false: the caller keeps
 * the measurement it already has, which is the behaviour without this hook.
 * `enabled` false never touches the font API.
 */
export function useFontFaceReady(spec: string, text: string, enabled = true): boolean {
  const key = `${spec}\n${text}`;
  const [readyKey, setReadyKey] = useState<string | null>(null);
  useEffect(() => {
    if (!enabled || typeof document === "undefined" || !document.fonts || typeof document.fonts.load !== "function") return undefined;
    let active = true;
    let pending: Promise<unknown>;
    try {
      pending = document.fonts.load(spec, text);
    } catch {
      return undefined;
    }
    void pending.then(() => { if (active) setReadyKey(key); }, () => undefined);
    return () => { active = false; };
  }, [enabled, key, spec, text]);
  return enabled && readyKey === key;
}
