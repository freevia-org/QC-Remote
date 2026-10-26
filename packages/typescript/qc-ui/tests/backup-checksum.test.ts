import assert from "node:assert/strict";
import test from "node:test";
import { isBackupChecksumReportV1, type BackupChecksumReportV1, type WorkspaceFileResult } from "@qc-remote/client";
import { formatNativeBackupSaveNotice } from "../src/backup-checksum.ts";

const report = (status: BackupChecksumReportV1["status"]): BackupChecksumReportV1 => ({
  schemaVersion: 1, algorithm: "sha256", status,
  checkedLayout: status === "unknown_layout" ? null : "local_stamp_873_21",
  reason: status === "verified" ? "local_match" : status === "mismatch" ? "local_mismatch" : "alternate_whole_payload",
  message: status === "verified" ? "Archive checksum matches the supplied checksum under the observed local layout."
    : status === "mismatch" ? "Archive checksum does not match the observed local layout; formatting differences or damaged data are possible."
      : "Checksum assessment is unavailable for this backup layout.",
  authenticityVerified: false, stampIntegrityVerified: false,
  metadataIntegrityVerified: false, restoreCompatibilityVerified: false
});

test("saved backup notices describe every status without using untrusted explanatory text", () => {
  for (const status of ["verified", "mismatch", "unknown_layout"] as const) {
    const result: WorkspaceFileResult = { cancelled: false, name: "Public fixture.json", backupChecksum: report(status) };
    const before = JSON.stringify(result);
    const notice = formatNativeBackupSaveNotice(result);
    assert.match(notice, /backup saved as Public fixture.json/);
    assert.match(notice, status === "verified" ? /matches; authenticity and restore compatibility are not verified/
      : status === "mismatch" ? /formatting differences or damaged data are possible/ : /assessment unavailable/);
    assert.equal(JSON.stringify(result), before);
    const untrusted = { ...result, backupChecksum: { ...result.backupChecksum, message: "private-payload marker" } } as unknown as WorkspaceFileResult;
    const untrustedNotice = formatNativeBackupSaveNotice(untrusted);
    assert.match(untrustedNotice, /saved as Public fixture.json.*assessment unavailable/);
    assert.doesNotMatch(untrustedNotice, /private-payload/);
  }
});

test("incoming report validation requires the exact schema and all false assurance fields", () => {
  for (const status of ["verified", "mismatch", "unknown_layout"] as const) {
    const valid = report(status);
    assert.equal(isBackupChecksumReportV1(valid), true);
    for (const assurance of ["authenticityVerified", "stampIntegrityVerified", "metadataIntegrityVerified", "restoreCompatibilityVerified"]) {
      assert.equal(isBackupChecksumReportV1({ ...valid, [assurance]: true }), false, assurance);
      assert.equal(isBackupChecksumReportV1({ ...valid, [assurance]: "false" }), false, assurance);
    }
    for (const key of Object.keys(valid)) {
      const incomplete = { ...valid } as Record<string, unknown>;
      delete incomplete[key];
      assert.equal(isBackupChecksumReportV1(incomplete), false, `missing ${key}`);
    }
    assert.equal(isBackupChecksumReportV1({ ...valid, payload: "private-data" }), false);
  }
  for (const value of [null, true, "verified", [], {}, { ...report("verified"), reason: "__proto__" }]) {
    assert.equal(isBackupChecksumReportV1(value), false);
  }
  assert.equal(isBackupChecksumReportV1(new Proxy({}, { get() { throw new Error("unexpected getter"); } })), false);
});

test("old hosts and cancellation retain existing notices while malformed reports cannot imply verification", () => {
  assert.equal(formatNativeBackupSaveNotice({ cancelled: true, backupChecksum: report("verified") }), "Device backup cancelled.");
  assert.equal(formatNativeBackupSaveNotice({ cancelled: false, name: "Old.json" }), "Native Quad Cortex backup saved as Old.json.");
  for (const changed of [{ schemaVersion: 2 }, { algorithm: "unknown" }, { authenticityVerified: true }, { reason: "local_mismatch" }, { checkedLayout: null }]) {
    const result = { cancelled: false, name: "Saved.json", backupChecksum: { ...report("verified"), ...changed } } as unknown as WorkspaceFileResult;
    assert.match(formatNativeBackupSaveNotice(result), /saved as Saved.json.*assessment unavailable/);
  }
});
