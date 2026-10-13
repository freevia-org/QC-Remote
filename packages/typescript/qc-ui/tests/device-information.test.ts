import assert from "node:assert/strict";
import { readFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import type { DeviceIdentity } from "@qc-remote/client";
import { qcDeviceInformation } from "../src/device-information.ts";
import { bundleModule } from "./support/display-page.ts";

const FULL: DeviceIdentity = {
  serial: "SER-1",
  customName: "Studio QC",
  macAddress: "aa:bb:cc:dd:ee:ff",
  corOsVersion: "9.8.7",
  linuxKernelVersion: "Linux test-kernel 1.2.3",
  ubootVersion: "U-Boot test 4.5.6",
  appFwVersion: "zj-app-1",
  bootloaderFwVersion: "zj-boot-2",
  zencoderFwApp: "zc-app-3",
  zencoderFwBootloader: "zc-boot-4",
  zenwirelessFwVersion: "zw-5"
};

test("every field the unit reports lands in its own row", () => {
  assert.deepEqual(qcDeviceInformation(FULL), {
    serial: "SER-1", deviceName: "Studio QC", macAddress: "aa:bb:cc:dd:ee:ff", corOsVersion: "9.8.7",
    linuxKernel: "Linux test-kernel 1.2.3", uBoot: "U-Boot test 4.5.6",
    zenjackApp: "zj-app-1", zenjackBootloader: "zj-boot-2", zencoderApp: "zc-app-3", zencoderBootloader: "zc-boot-4", zenwireless: "zw-5"
  });
});

test("the application firmware build is the Zenjack FW app row and never the CorOS version", () => {
  // The unit's version reply has no CorOS field; the build it does carry (observed "d14e" on a unit running CorOS 4.1.0) is the Zenjack app firmware.
  const information = qcDeviceInformation({ serial: "S", appFwVersion: "d14e" });
  assert.equal(information.zenjackApp, "d14e");
  assert.equal(information.corOsVersion, undefined, "the old mapping showed d14e as the CorOS version");
});

test("a field the unit did not report stays unreported; blank text is not a value; the app's own name fills a missing device name", () => {
  const information = qcDeviceInformation({ serial: "S", customName: "  ", macAddress: "" }, "Rig");
  assert.equal(information.deviceName, "Rig");
  assert.equal(information.macAddress, undefined);
  assert.equal(information.linuxKernel, undefined);
  assert.deepEqual(Object.values(qcDeviceInformation(undefined)).filter((value) => value !== undefined), []);
});

type Renderer = { render(information: object): Promise<string> };
const workDirectory = mkdtempSync(join(tmpdir(), "qc-device-information-"));
const entry = `
import { createElement } from "react";
import { prerenderToNodeStream } from "react-dom/static";
import { QcDeviceInformationDisplay } from ${JSON.stringify(join(fileURLToPath(new URL("../src/", import.meta.url)), "device-information-display.tsx"))};
import "@qc-remote/theme/theme.css";
export async function render(information) {
  const { prelude } = await prerenderToNodeStream(createElement(QcDeviceInformationDisplay, { deviceInformation: information, onClose() {} }));
  const chunks = [];
  for await (const chunk of prelude) chunks.push(Buffer.from(chunk));
  return Buffer.concat(chunks).toString("utf8").replaceAll("<!-- -->", "");
}
`;
const { module } = await bundleModule<Renderer>(workDirectory, "device-information", entry).catch((error) => { rmSync(workDirectory, { recursive: true, force: true }); throw error; });
test.after(() => rmSync(workDirectory, { recursive: true, force: true }));

const row = (markup: string, label: string) => markup.match(new RegExp(`<b>${label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}</b><i>([^<]*)</i>`))?.[1];

test("the screen shows the unit's real value in every row, with no redaction and nothing marked as a reference", async () => {
  const markup = await module.render(qcDeviceInformation(FULL));
  const rows: [string, string][] = [
    ["Serial number:", "SER-1"], ["Device name:", "Studio QC"], ["MAC address:", "aa:bb:cc:dd:ee:ff"], ["CorOS:", "9.8.7"],
    ["Linux kernel:", "Linux test-kernel 1.2.3"], ["U-Boot:", "U-Boot test 4.5.6"], ["Zenjack FW app:", "zj-app-1"],
    ["Zenjack FW bootloader:", "zj-boot-2"], ["Zencoder FW app:", "zc-app-3"], ["Zencoder FW bootloader:", "zc-boot-4"], ["Zenwireless FW:", "zw-5"]
  ];
  for (const [label, value] of rows) assert.equal(row(markup, label), value, label);
  assert.ok(!markup.includes("(reference)"));
  assert.ok(!markup.includes("Not reported"));
});

test("a field the unit did not report says so: CorOS shows the documented 4.1.0 as a reference, Zenjack keeps its own row", async () => {
  // What the gateway reports today: serial, name and the application firmware build.
  const markup = await module.render(qcDeviceInformation({ serial: "SER-1", customName: "Studio QC", appFwVersion: "d14e" }));
  assert.equal(row(markup, "CorOS:"), "4.1.0 (reference)", "CorOS is the documented version, marked, never the Zenjack build");
  assert.equal(row(markup, "Zenjack FW app:"), "d14e", "the Zenjack value is live and unmarked");
  assert.equal(row(markup, "MAC address:"), "Not reported by the unit");
  assert.match(row(markup, "Linux kernel:") ?? "", /\(reference\)$/);
  assert.equal(row(markup, "Serial number:"), "SER-1");
});

test("no host or window blanks a field of the Device Information screen", () => {
  const read = (relative: string) => readFileSync(fileURLToPath(new URL(relative, import.meta.url)), "utf8");
  const windowSource = read("../../../../apps/windows/src/app-window.tsx");
  assert.doesNotMatch(windowSource, /serial: ""|corOsVersion: ""/, "the Windows window shows the live values, not blanks");
  assert.match(windowSource, /device\.information \?\? \{ deviceName: device\.deviceName \}/);
  const settings = read("../src/device-settings-screen.tsx");
  assert.doesNotMatch(settings, /DeviceInformationDetail|corOsVersion: workflow\.identity\?\.appFwVersion/, "Device Settings uses the same pane and mapping");
  const views = read("../src/use-qc-device-views.ts");
  assert.match(views, /gateway\.identity\(\)[\s\S]*?owner\.apply\(\(\) => setDeviceInformation\(\{ key: owner\.key, value: qcDeviceInformation\(identity, snapshot\.deviceName\)/,
    "the captured identity read is session-scoped before mapping its original device name");
});
