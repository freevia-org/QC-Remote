import { useEffect, useState, type ReactNode } from "react";
import { QCIosNativeBridge } from "../native-bridge";

export interface IPadDriverStatusProps {
  driverHardwareSupported: boolean;
  driverOsSupported: boolean;
  fullUsbDriverBundled: boolean;
  fullUsbReady: boolean;
  fullUsbRequirement: string;
  driverCompatibilityNote: string;
  children: ReactNode;
}

export function IPadDriverStatus({ driverHardwareSupported, driverOsSupported, fullUsbDriverBundled, fullUsbReady, fullUsbRequirement, driverCompatibilityNote, children }: IPadDriverStatusProps) {
  const [midiChannel, setMidiChannel] = useState(1);
  const [driverReady, setDriverReady] = useState(fullUsbReady);
  const [checkingDriver, setCheckingDriver] = useState(false);
  const [writesQuarantined, setWritesQuarantined] = useState(false);
  useEffect(() => {
    void QCIosNativeBridge.getMidiInputChannel().then(({ channel }) => setMidiChannel(channel)).catch(() => undefined);
  }, []);
  const refreshDriverReadiness = async () => {
    setCheckingDriver(true);
    try {
      const capabilities = await QCIosNativeBridge.platformCapabilities();
      setDriverReady(capabilities.fullUsbReady);
    } catch {
      setDriverReady(false);
    }
    try {
      const diagnostics = await QCIosNativeBridge.usbDriverDiagnostics();
      setWritesQuarantined(diagnostics.writesQuarantined === 1);
    } catch {
      // The user client has no diagnostics until the QC driver is open.
    } finally {
      setCheckingDriver(false);
    }
  };
  useEffect(() => {
    void refreshDriverReadiness();
    const timer = window.setInterval(() => {
      void QCIosNativeBridge.usbDriverDiagnostics()
        .then((diagnostics) => setWritesQuarantined(diagnostics.writesQuarantined === 1))
        .catch(() => undefined);
    }, 2_000);
    return () => window.clearInterval(timer);
  }, []);
  const changeMidiChannel = (channel: number) => {
    setMidiChannel(channel);
    void QCIosNativeBridge.setMidiInputChannel({ channel }).catch(() => undefined);
  };
  return <div className="ios-ipad-root">
    <aside className="ios-ipad-driver-notice" role="status">
      {writesQuarantined && <span className="ios-ipad-write-quarantine" role="alert"><strong>Direct USB writes are paused.</strong> A persistent QC operation may have an unknown result. Disconnect, reconnect, and wait for synchronization before trying again.</span>}
      <strong>Full USB control requires the QC Remote DriverKit driver.</strong>
      <span>{!driverHardwareSupported
        ? "This iPad model does not have an M-series chip. Use an M-series iPad to connect directly through USBDriverKit."
        : driverOsSupported
        ? "This iPadOS version supports DriverKit on M-series iPads. The iPad must have an M-series chip, and the driver must be signed with Apple-approved USB and app-to-driver entitlements, then enabled in Settings › Privacy & Security › Drivers."
        : "DriverKit requires iPadOS 16 or later on an M-series iPad. Update iPadOS and use a supported iPad for direct USB HID control."}</span>
      <span className="ios-ipad-driver-build-state">{driverReady ? "The USB driver service is available to this app." : !fullUsbDriverBundled ? `The driver is not bundled in this app build. ${fullUsbRequirement}.` : "The driver is bundled, but its USB service is unavailable. Connect the QC and check the DriverKit entitlement and Settings › Privacy & Security › Drivers."} Existing paired-host control remains available below.</span>
      <span className="ios-ipad-driver-compatibility">{driverCompatibilityNote}</span>
      <button type="button" className="ios-ipad-driver-refresh" disabled={checkingDriver} onClick={() => void refreshDriverReadiness()}>{checkingDriver ? "Checking DriverKit…" : "Check DriverKit availability"}</button>
    </aside>
    <section className="ios-ipad-midi-channel" aria-label="Quad Cortex MIDI channel">
      <label>Quad Cortex MIDI input channel<select value={midiChannel} onChange={(event) => changeMidiChannel(Number(event.target.value))}>{Array.from({ length: 16 }, (_, index) => <option key={index} value={index + 1}>Channel {index + 1}</option>)}</select></label>
    </section>
    {children}
  </div>;
}
