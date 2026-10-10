import { useCallback, useState } from "react";

export type CommandJournal<T> = {
  undoEntry?: T;
  redoEntry?: T;
  record: (entry: T) => void;
  clear: () => void;
  markUndone: (entry: T) => void;
  markRedone: (entry: T) => void;
};

/** Shared one-step command journal for device UIs with verified undo/redo. */
export function useCommandJournal<T>(): CommandJournal<T> {
  const [entries, setEntries] = useState<{undoEntry?: T; redoEntry?: T}>({});
  const record = useCallback((entry: T) => {
    setEntries({undoEntry: entry});
  }, []);
  const clear = useCallback(() => {
    setEntries({});
  }, []);
  // Completion belongs to the entry that started the operation. A newer
  // performance edit or a cleared preset history must survive a late result.
  const markUndone = useCallback((entry: T) => {
    setEntries((current) => current.undoEntry === entry ? {redoEntry: entry} : current);
  }, []);
  const markRedone = useCallback((entry: T) => {
    setEntries((current) => current.redoEntry === entry ? {undoEntry: entry} : current);
  }, []);
  return { ...entries, record, clear, markUndone, markRedone };
}
