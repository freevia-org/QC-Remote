import { createElement } from "react";

/** Opens the authenticated web flow; account deletion is confirmed there. */
export function RemoteAccountDeletionLink({ onOpen, disabled = false }: { onOpen: () => void; disabled?: boolean }) {
  return createElement("section", { "aria-label": "Remote account deletion" },
    createElement("button", { type: "button", disabled, onClick: onOpen }, "Delete remote account"),
    createElement("p", null, "Sign in with Google on the website and confirm deletion there. Deleting your remote account removes its paired devices, linked AI apps, usage records and pending pairings."));
}
