import assert from "node:assert/strict";
import { readFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import type { DeviceIdentity } from "@qc-remote/client";
import { qcDeviceInformation } from "../src/device-information.ts";
import { corOsVersionFromTree, isCorOsVersion, reconcileCorOsVersion } from "../src/cor-os-version.ts";
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

test("the CorOS row shows the unit's version without the reference mark once either source has stated it", async () => {
  const identity = { serial: "SER-1", customName: "Studio QC", appFwVersion: "d14e" };
  const fromScreen = await module.render(qcDeviceInformation(identity, undefined, { screen: { value: "4.1.0", at: 1 } }));
  assert.equal(row(fromScreen, "CorOS:"), "4.1.0", "read from the unit's Device Information screen");
  const fromHost = await module.render(qcDeviceInformation({ ...identity, corOsVersion: "4.1.0" }));
  assert.equal(row(fromHost, "CorOS:"), "4.1.0", "stated by the host");
  assert.ok(!fromScreen.includes("CorOS: 4.1.0 (reference)"));
  // Not a version: still the marked reference value, never the text.
  const junk = await module.render(qcDeviceInformation(identity, undefined, { screen: { value: "XXXXXXXX", at: 1 } }));
  assert.equal(row(junk, "CorOS:"), "4.1.0 (reference)");
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

/** The shape of the unit's tree while Settings > Support > Device Information is open; every identifying value is a placeholder. */
const DEVICE_INFORMATION_TREE = readFileSync(new URL("./support/device-information-unit-tree.txt", import.meta.url), "utf8");

/** A Device Information subtree with the given text nodes, each under its own GraphicsItem. */
const screen = (...texts: string[]) =>
  ["zenUI::RootGraphicsItem", "  zenUI::SettingsDialog", "    zenUI::DeviceInformation", ...texts.flatMap((text) => ["      zenUI::GraphicsItem", `        text : '${text}'`])].join("\n");

test("the version is the text node right after the CorOS label under zenUI::DeviceInformation", () => {
  assert.equal(corOsVersionFromTree(DEVICE_INFORMATION_TREE), "4.1.0");
  assert.equal(corOsVersionFromTree(DEVICE_INFORMATION_TREE.replace(/\n/g, "\r\n")), "4.1.0", "CRLF trees read the same");
  assert.equal(corOsVersionFromTree(screen("CorOS:", " 12.30.1 ")), "12.30.1");
  assert.equal(corOsVersionFromTree(screen("CorOS:", "4.1.0.2")), "4.1.0.2");
});

test("any other shape of tree yields no version", () => {
  const shapes: Record<string, string> = {
    "no Device Information screen": DEVICE_INFORMATION_TREE.replace(/zenUI::DeviceInformation/, "zenUI::SomethingElse"),
    "a class that only starts like it": DEVICE_INFORMATION_TREE.replace(/zenUI::DeviceInformation/, "zenUI::DeviceInformationExtra"),
    "the label outside the screen": ["zenUI::RootGraphicsItem", "  zenUI::DeviceInformation", "    zenUI::GraphicsItem", "      text : 'Serial number:'", "  zenUI::GraphicsItem", "    text : 'CorOS:'", "    text : '4.1.0'"].join("\n"),
    "no CorOS label": screen("Software information", "Serial number:", "XXXXXXXX", "4.1.0"),
    "a label with nothing after it": screen("Serial number:", "XXXXXXXX", "CorOS:"),
    "a label then the serial": screen("CorOS:", "XXXXXXXX", "4.1.0"),
    "a label then a mac address": screen("CorOS:", "XX:XX:XX:XX:XX:XX", "4.1.0"),
    "a lowercase label": screen("coros:", "4.1.0"),
    "a label with extra text": screen("CorOS: 4.1.0"),
    "the label's own text as the value": screen("CorOS:", "CorOS:", "4.1.0"),
    "an empty tree": ""
  };
  for (const [name, tree] of Object.entries(shapes)) assert.equal(corOsVersionFromTree(tree), undefined, name);
  // A text value that runs over several lines is not a value, and does not let a later node stand in for it.
  assert.equal(corOsVersionFromTree(["zenUI::DeviceInformation", "  zenUI::GraphicsItem", "    text : 'CorOS:'", "  zenUI::GraphicsItem", "    text : 'XXXX", "    4.1.0'", "  zenUI::GraphicsItem", "    text : '4.1.0'"].join("\n")), undefined);
});

test("only a dotted version of two to four short numeric parts counts", () => {
  for (const value of ["", " ", "d14e", "XXXXXXXX", "4", "4.", ".4", "4..1", "4.1.", "4.1.0-beta", "v4.1.0", "1.2.3.4.5", "4.1 0", "1234.1", "4.1.0 x", "٤.١.٠", "4,1,0", "0x4.1"]) {
    assert.equal(corOsVersionFromTree(screen("CorOS:", value)), undefined, JSON.stringify(value));
  }
  assert.ok(isCorOsVersion("4.1.0") && isCorOsVersion("0.0") && isCorOsVersion("123.456.789.012"));
  assert.ok(!isCorOsVersion(undefined) && !isCorOsVersion("") && !isCorOsVersion("4") && !isCorOsVersion("1.2.3.4.5") && !isCorOsVersion("XXXXXXXX"));
});

test("the serial number and the other identifying values never reach the result", () => {
  const serial = "XXXXXXXX";
  const version = corOsVersionFromTree(DEVICE_INFORMATION_TREE);
  assert.equal(version, "4.1.0");
  assert.ok(!JSON.stringify(version).includes(serial));
  // Whatever the shape, what comes back is a version number or nothing: never a node that merely follows another.
  for (const tree of [DEVICE_INFORMATION_TREE, screen("Serial number:", serial, "CorOS:", serial), screen("CorOS:", serial), screen(serial, "CorOS:", "4.1.0")]) {
    const found = corOsVersionFromTree(tree);
    assert.ok(found === undefined || /^[0-9.]+$/.test(found), String(found));
  }
  // And the Device Information mapping carries it in the CorOS row only.
  const information = qcDeviceInformation({ serial: "SYN-0001" }, undefined, { screen: { value: version!, at: 1 } });
  assert.equal(information.corOsVersion, "4.1.0");
  assert.ok(!Object.values(information).some((value) => value === serial || value?.includes(serial)));
});

test("the later of the host's value and the screen's wins; one source alone is used; neither means no value", () => {
  assert.equal(reconcileCorOsVersion(undefined), undefined);
  assert.equal(reconcileCorOsVersion("4.1.0"), "4.1.0");
  assert.equal(reconcileCorOsVersion(undefined, { screen: { value: "4.1.0", at: 5 } }), "4.1.0");
  // They agree.
  assert.equal(reconcileCorOsVersion("4.1.0", { identityReadAt: 10, screen: { value: "4.1.0", at: 20 } }), "4.1.0");
  // They differ: the screen was seen after the identity read, so the screen wins ...
  assert.equal(reconcileCorOsVersion("4.1.0", { identityReadAt: 10, screen: { value: "4.2.0", at: 20 } }), "4.2.0");
  // ... the identity was read after the screen was last seen, so the host wins ...
  assert.equal(reconcileCorOsVersion("4.2.0", { identityReadAt: 30, screen: { value: "4.1.0", at: 20 } }), "4.2.0");
  // ... and on a tie the protocol source does.
  assert.equal(reconcileCorOsVersion("4.2.0", { identityReadAt: 20, screen: { value: "4.1.0", at: 20 } }), "4.2.0");
  // A value that is not a version from either source is ignored.
  assert.equal(reconcileCorOsVersion("d14e", { screen: { value: "XXXXXXXX", at: 1 } }), undefined);
  assert.equal(reconcileCorOsVersion("4.1.0", { identityReadAt: 1, screen: { value: "XXXXXXXX", at: 9 } }), "4.1.0");
});
