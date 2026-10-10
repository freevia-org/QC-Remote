/** The scopes this relay grants, and what each one means to someone who owns
 * the instrument.
 *
 * One list. The authorization server's `scopesSupported`, the protected-resource
 * metadata's `scopes_supported`, the filter applied to what a client asks for,
 * and the consent screen all read it, so none of them can advertise a
 * capability another does not honour. `qc:pair` used to be grantable and was
 * checked nowhere; a scope a person can consent to and no code enforces is a
 * promise the system does not keep. */

/** Standard OAuth refresh, not a device capability. Listed so clients that
 * support refresh tokens keep their grant; it broadens no QC operation. */
export const OFFLINE_ACCESS = "offline_access";

export const QC_SCOPES = ["qc:read", "qc:control", OFFLINE_ACCESS] as const;

export type QcScope = (typeof QC_SCOPES)[number];

export const SCOPE_DESCRIPTIONS: Record<QcScope, string> = {
  "qc:read": "Read your Quad Cortex: its status, the preset that is loaded, and your library listings.",
  "qc:control": "Change your Quad Cortex: performance controls, preset edits, and saving, overwriting or deleting presets, setlists and captures.",
  [OFFLINE_ACCESS]: "Stay connected without signing in again."
};

/** The scopes that actually gate a device operation. `offline_access` does not,
 * so it is not one of them. */
export const DEVICE_SCOPES = QC_SCOPES.filter((scope) => scope !== OFFLINE_ACCESS);

export function isQcScope(value: unknown): value is QcScope {
  return typeof value === "string" && (QC_SCOPES as readonly string[]).includes(value);
}
