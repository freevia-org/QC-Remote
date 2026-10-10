// Device-derived reference data, kept apart from the screen code so the
// project LICENSE can carve it out by whole file: the software information,
// diagnostics readouts, third-party licence list and News QR code CorOS shows
// on its Settings screens. Not licensed under MIT OR Apache-2.0; see
// LICENSES/LicenseRef-QC-Remote-Reference.txt.

export const USB_STATISTICS: Array<[string, string, string?]> = [
  ["USBConnected", "Yes"],
  ["IN rd/wr idx", "0/0"],
  ["OUT rd/wr idx", "0/0"],
  ["UI to DSP HID count (tx/rx)", "468580/468580"],
  ["USBAudioInRdWrDistanceAverage", "0", "is-red"],
  ["USBAudioOutRdWrDistanceAverage", "0", "is-red"],
  ["USBAudioRxFrame", "0"],
  ["USBAudioTxFrame", "0"],
  ["USBAudioTxFrameSkipped", "0"],
  ["USBAudioTxFrameAborted", "0"],
  ["USBAudioOUTGapsDetected", "0"],
  ["USBAudioOUTGapResets", "0"],
  ["USBAudioOUTBufferResets", "0"],
  ["USBAudioPlusOneSampleCount", "0", "is-green"],
  ["USBAudioMinusOneSampleCount", "0", "is-blue"],
  ["USBAudioStreamingEPEnabled", "0"],
  ["USBAudioStreamingEPDisabled", "0"],
  ["USBMidiInCount", "18"],
  ["USBMidiOutCount", "0"],
  ["HID driver sent report count", "468580"],
  ["HID driver received report count", "3777"],
];

export const FOOTSWITCH_STATISTICS: Array<[string, string, string]> = [
  ["A", "(p: 2, r: 2)", "(l: 0, r: 0)"],
  ["B", "(p: 0, r: 0)", "(l: 0, r: 0)"],
  ["C", "(p: 0, r: 0)", "(l: 0, r: 0)"],
  ["D", "(p: 0, r: 0)", "(l: 0, r: 0)"],
  ["DOWN", "(p: 0, r: 0)", "(l: 0, r: 0)"],
  ["E", "(p: 0, r: 0)", "(l: 0, r: 0)"],
  ["F", "(p: 0, r: 0)", "(l: 0, r: 0)"],
  ["G", "(p: 0, r: 0)", "(l: 0, r: 0)"],
  ["H", "(p: 0, r: 0)", "(l: 0, r: 0)"],
  ["TEMPO", "(p: 0, r: 0)", "(l: 0, r: 0)"],
  ["UP", "(p: 0, r: 0)", "(l: 0, r: 0)"],
];

/** Five columns; CorOS clips SOC2 ARM the same way, leaving four visible. */
export const DSP_CORES = ["Core 1", "Core 2", "Core 3", "Core 4", "SOC2 ARM"];

export const DSP_DIAGNOSTICS: Array<[string, string[]]> = [
  ["Drp Msg:", ["0", "0", "0", "0", "-1"]],
  ["FP exc:", ["0", "0", "0", "0", "-1"]],
  // Cores 1 and 3 as the device's own screen reads them.
  ["CPU Av/Pk(%):", ["3.3/3.5/1.7", "32/33/0.58", "4.8/5/1.3", "1.3/1.5/0", "-1"]],
  ["EMDMA (%):", ["0", "6.65", "0", "0", "-1"]],
  ["Heap(%):", ["DM:0.177/PM:0/SD:0.00687", "DM:3.3/PM:6.74/SD:0.208", "DM:0.135/PM:0/SD:0.00687", "DM:0.135/PM:0/SD:0.00687", "-1"]],
];

export const DSP_SOC_COUNTERS = [
  ["SOC1.1", "53846"],
  ["SOC1.2", "21451"],
  ["SOC2.0", "0"],
];

/** Every licence CorOS lists; the screen shows the first six of them. */
export const THIRD_PARTY_LICENSES = ["binutils license", "busybox license", "bzip2 license", "collectd license", "cramfs license", "e2fsprogs license", "elfutils license", "expat license", "f2fstools license", "fontconfig license", "freetype license", "harfbuzz license", "hostapd license", "iw license", "jpeg-turbo license", "libarchive license", "libcurl license", "libevent license", "libffi license", "libnl license", "libpcap license", "libpng license", "libsigc license", "libsigsegv license", "libtirpc license", "libungif license", "libunwind license", "linux license", "lrzsz license", "lz4 license", "lzip license", "lzo license", "lzop license", "memtester license", "mtd license", "ncurses license", "ntp license", "openssh license", "openssl license", "oprofile license", "pcutils license", "popt license", "pppd license", "protobuf license", "qt license", "readline license", "rsync license", "squashfs license", "tslib license", "uboot-tools license", "util-linux license", "wpa_supplicant license", "zlib license", "ADVobfuscator license", "pugixml license"];

/* Device Updates uses a compact, 25-module web-link mark. Keep the geometry as
   data instead of introducing a second raster asset: it remains resolution
   independent on both hosts and gives every renderer one canonical source. */
export const NEWS_QR_MODULES = ["1111111011110010101111111", "1000001010010110001000001", "1011101001000111101011101", "1011101000011110101011101", "1011101010011110001011101", "1000001011101100101000001", "1111111010101010101111111", "0000000011000011000000000", "1110011011011001011110011", "0110100000001101101101011", "0000001111101001101101101", "0001000100111010110111000", "0001011011101001101100001", "0101100001100101111100011", "1101101101010101101001101", "0010010010110010110111000", "1110101100110010111110010", "0000000011011100100010001", "1111111000110000101010001", "1000001011011011100010001", "1011101000110001111110010", "1011101001111101010010110", "1011101010110001100111011", "1000001010101000100110000", "1111111010011000110001001"] as const;

export const DEVICE_NAME_FALLBACK = "Neural DSP Quad Cortex";

export const COROS_REFERENCE_VERSION = "4.1.0";

export const COROS_REFERENCE_VERSION_LINE = "CorOS: 4.1.0";

export const LINUX_KERNEL_BUILD = [
  "Linux buildroot 4.0.0-ADI-1.3.0 #1 PREEMPT Tue",
  "Aug 18 01:26:58 EEST 2026 armv7l (none)"
] as const;

export const UBOOT_BUILD = [
  "U-Boot 2015.01 ADI-1.3.0 (Sep 30 2021 -",
  "01:01:44)"
] as const;

export const ZENJACK_FW_APP = "d14e";

export const ZENJACK_FW_BOOTLOADER = "b113";

export const ZENCODER_FW_APP = "d111";

export const ZENCODER_FW_BOOTLOADER = "b103";

export const ZENWIRELESS_FW = "cf9daede4300aaae664fc527cede12ae";
