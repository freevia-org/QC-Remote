import { isBackupChecksumReportV1, type WorkspaceFileResult } from "@qc-remote/client";

/** Describe a completed save without treating an untrusted digest as authentication. */
export function formatNativeBackupSaveNotice(result: WorkspaceFileResult): string {
  if (result.cancelled) return "Device backup cancelled.";
  const saved = `Native Quad Cortex backup saved as ${result.name ?? "a local file"}.`;
  const report = result.backupChecksum;
  if (!report) return saved;
  if (!isBackupChecksumReportV1(report)) return `${saved} Checksum assessment unavailable.`;
  if (report.status === "verified" && report.reason === "local_match" && report.checkedLayout === "local_stamp_873_21") {
    return `${saved} Archive checksum matches; authenticity and restore compatibility are not verified.`;
  }
  if (report.status === "mismatch" && report.reason === "local_mismatch" && report.checkedLayout === "local_stamp_873_21") {
    return `${saved} Archive checksum differs under the observed layout; formatting differences or damaged data are possible.`;
  }
  return `${saved} Checksum assessment unavailable.`;
}
