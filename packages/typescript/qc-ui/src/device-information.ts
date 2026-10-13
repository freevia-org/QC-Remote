import type { DeviceIdentity } from "@qc-remote/client";
import type { CorOsDeviceInformation } from "./coros-reference-settings-pane.tsx";
import { reconcileCorOsVersion, type CorOsVersionSightings } from "./cor-os-version.ts";

const present = (value: string | undefined): string | undefined => {
  const trimmed = value?.trim();
  return trimmed ? trimmed : undefined;
};

/**
 * The Device Information screen's data, from what the unit reported. The one
 * mapping every host and window uses, so a field can never land in the wrong
 * row: the version reply's application firmware build is the Zenjack FW app
 * row, never the CorOS row. A field the unit did not report stays undefined
 * and the screen says so. The CorOS row is the host's value or the one read from
 * the unit's own Device Information screen, whichever was obtained later
 * (reconcileCorOsVersion).
 */
export function qcDeviceInformation(identity: DeviceIdentity | undefined, fallbackDeviceName?: string, sightings?: CorOsVersionSightings): CorOsDeviceInformation {
  return {
    serial: present(identity?.serial),
    deviceName: present(identity?.customName) ?? present(fallbackDeviceName),
    macAddress: present(identity?.macAddress),
    corOsVersion: reconcileCorOsVersion(present(identity?.corOsVersion), sightings),
    linuxKernel: present(identity?.linuxKernelVersion),
    uBoot: present(identity?.ubootVersion),
    zenjackApp: present(identity?.appFwVersion),
    zenjackBootloader: present(identity?.bootloaderFwVersion),
    zencoderApp: present(identity?.zencoderFwApp),
    zencoderBootloader: present(identity?.zencoderFwBootloader),
    zenwireless: present(identity?.zenwirelessFwVersion)
  };
}
