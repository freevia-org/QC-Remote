/** Formats a rejected native relay call for the status notice, keeping the
 * plugin's message and its error code (USB_PERMISSION_REQUIRED, ...). */
export function relayFailureNotice(error: unknown, fallback = "Remote could not start."): string {
  const raw = error instanceof Error ? error.message : typeof error === "string" ? error : (error as { message?: unknown } | null)?.message;
  const message = typeof raw === "string" && raw ? raw : fallback;
  const code = (error as { code?: unknown } | null)?.code;
  return typeof code === "string" && code && !message.includes(code) ? `${message} (${code})` : message;
}

/** Starts the relay from a user action; a rejection becomes a notice instead
 * of an unhandled promise rejection. */
export async function startRelayReportingFailure(start: () => Promise<unknown>, notice: (text: string) => void): Promise<void> {
  try {
    await start();
  } catch (error) {
    notice(relayFailureNotice(error));
  }
}
