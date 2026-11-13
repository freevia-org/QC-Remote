import type { AndroidUsbDiagnostics } from "./native-services.ts";

/** Fields that distinguish failure causes. Every one is a counter, an id, a timing or an event
 * name: the native side never puts a preset, a device name or a payload in any of them. */
const SCALAR_FIELDS = [
  "phase", "sessionEpoch", "connected", "synchronized", "messagesReceived", "messagesSent",
  "rawReportsReceived", "decodeErrors", "expectedWriteStalls", "hidWriteTimeouts", "terminalHidWriteFailures",
  "emptyReads", "malformedReports", "lastMessageType", "readAttempts", "negativeReads",
  "interfaceId", "inputEndpointAddress", "inputMaxPacketSize", "reportBytes",
  "lastHidWriteResult", "lastHidWriteDurationMs", "maxHidWriteDurationMs", "lastHidWriteIncludedReportId",
  "readerRequestCount", "readerWaiting", "lastReaderError", "writesQuarantined", "writeQuarantineReason",
  "lastError", "lastStartupFailure", "lastStartupFailureSessionEpoch"
] as const;

export const USB_REPORT_FLIGHT_ENTRIES = 60;

/** A compact, payload-free block appended to the human sentence by "Copy diagnostics". */
export function formatUsbDiagnosticsReport(android: Partial<AndroidUsbDiagnostics> & Record<string, unknown>): string {
  const report: Record<string, unknown> = {};
  for (const field of SCALAR_FIELDS) {
    const value = android[field];
    if (value !== undefined && value !== null) report[field] = value;
  }
  if (android.messagesReceivedByType) report.messagesReceivedByType = android.messagesReceivedByType;
  if (android.messagesSentByType) report.messagesSentByType = android.messagesSentByType;
  const entries = android.flightRecorder?.entries;
  if (Array.isArray(entries)) {
    report.flight = entries.slice(-USB_REPORT_FLIGHT_ENTRIES).map(({ atUnixMs, event, messageType, reportCount }) =>
      ({ atUnixMs, event, ...(messageType !== undefined ? { messageType } : {}), ...(reportCount !== undefined ? { reportCount } : {}) }));
  }
  return `USB diagnostics data: ${JSON.stringify(report)}`;
}
