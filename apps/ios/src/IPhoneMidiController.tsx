import { useCallback, useEffect, useState } from "react";
import { QCIosNativeBridge } from "../native-bridge";

type MidiDevice = { deviceId: number; name: string; manufacturer: string; connected: boolean };

export function IPhoneMidiController() {
  const [devices, setDevices] = useState<MidiDevice[]>([]);
  const [deviceId, setDeviceId] = useState<number>();
  const [connectedName, setConnectedName] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("Connect the Quad Cortex with a USB-C cable or supported camera adapter.");
  const [bankMsb, setBankMsb] = useState(0);
  const [bankLsb, setBankLsb] = useState(0);
  const [program, setProgram] = useState(0);
  const [channel, setChannel] = useState(1);
  const [expression1, setExpression1] = useState(0);
  const [expression2, setExpression2] = useState(0);

  useEffect(() => {
    void QCIosNativeBridge.getMidiInputChannel().then(({ channel: storedChannel }) => setChannel(storedChannel)).catch(() => undefined);
  }, []);

  const changeChannel = (nextChannel: number) => {
    setChannel(nextChannel);
    void QCIosNativeBridge.setMidiInputChannel({ channel: nextChannel }).catch(() => setMessage("Could not save the MIDI channel preference."));
  };

  const scan = useCallback(async () => {
    setBusy(true);
    try {
      const result = await QCIosNativeBridge.scanUsbMidi();
      setDevices(result.devices);
      setDeviceId((current) => result.devices.some((device) => device.deviceId === current) ? current : result.devices[0]?.deviceId);
      setMessage(result.devices.length ? "Quad Cortex USB-MIDI is available." : "No Quad Cortex USB-MIDI device found. Check the cable and MIDI-over-USB setting on the unit.");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "USB-MIDI scanning failed.");
    } finally { setBusy(false); }
  }, []);

  useEffect(() => { void scan(); }, [scan]);

  const connect = async () => {
    if (deviceId == null) return;
    setBusy(true);
    try {
      const result = await QCIosNativeBridge.connectUsbMidi({ deviceId });
      setConnectedName(result.name);
      setMessage(`Connected to ${result.name}. MIDI commands are sent without state readback.`);
    } catch (error) { setMessage(error instanceof Error ? error.message : "Could not connect to USB-MIDI."); }
    finally { setBusy(false); }
  };

  const disconnect = async () => {
    await QCIosNativeBridge.disconnectUsbMidi();
    setConnectedName("");
    setMessage("Disconnected from Quad Cortex USB-MIDI.");
  };

  const sendCc = async (controller: number, value: number) => {
    if (!connectedName || busy) return;
    setBusy(true);
    try {
      await QCIosNativeBridge.sendUsbMidiControlChange({ channel, controller, value });
      setMessage(`MIDI CC ${controller} sent on channel ${channel}. The iPhone cannot confirm the resulting QC state.`);
    } catch (error) { setMessage(error instanceof Error ? error.message : "MIDI command failed."); }
    finally { setBusy(false); }
  };

  const sendPreset = async () => {
    if (!connectedName || busy) return;
    setBusy(true);
    try {
      await QCIosNativeBridge.sendUsbMidiProgramChange({ channel, bankMsb, bankLsb, program });
      setMessage(`Preset selection sent on channel ${channel} (setlist ${bankLsb + 1}, bank ${bankMsb}, program ${program + 1}). The iPhone cannot confirm the resulting QC state.`);
    } catch (error) { setMessage(error instanceof Error ? error.message : "Preset selection failed."); }
    finally { setBusy(false); }
  };

  const connected = Boolean(connectedName);
  return <main className="ios-midi-page">
    <header className="ios-midi-header">
      <div className="ios-midi-eyebrow">QC REMOTE · iPHONE</div>
      <h1>Quad Cortex MIDI</h1>
      <p>Direct USB-MIDI controls for your Quad Cortex.</p>
    </header>

    <aside className="ios-midi-limitation" role="note">
      <strong>Direct USB on iPhone is MIDI-only.</strong>
      <span>Preset and scene selection, footswitches, expression pedals, mode changes, tap tempo, tuner, Gig View, and Looper X controls are available here. Choose Paired host above for full control, live state, and backups. QC account, cloud, and update traffic can be forwarded when the paired online host supports and enables internet forwarding.</span>
    </aside>

    <section className="ios-midi-card" aria-label="USB connection">
      <div className="ios-midi-card-title"><div><h2>USB connection</h2><p>{connected ? `Connected · ${connectedName}` : "Not connected"}</p></div><span className={connected ? "ios-midi-status is-connected" : "ios-midi-status"} /></div>
      {!connected ? <div className="ios-midi-connect-row">
        <select aria-label="Quad Cortex MIDI device" value={deviceId ?? ""} onChange={(event) => setDeviceId(Number(event.target.value))}>
          {devices.length === 0 && <option value="">No QC MIDI device found</option>}
          {devices.map((device) => <option key={device.deviceId} value={device.deviceId}>{device.name}</option>)}
        </select>
        <button className="ios-midi-secondary" disabled={busy} onClick={() => void scan()}>Scan</button>
        <button className="ios-midi-primary" disabled={busy || deviceId == null} onClick={() => void connect()}>Connect</button>
      </div> : <button className="ios-midi-secondary" disabled={busy} onClick={() => void disconnect()}>Disconnect</button>}
    </section>

    <section className="ios-midi-card" aria-label="MIDI channel">
      <div className="ios-midi-card-title"><div><h2>MIDI channel</h2><p>Match the Quad Cortex MIDI input channel</p></div></div>
      <label className="ios-midi-channel">Transmit on<select value={channel} onChange={(event) => changeChannel(Number(event.target.value))}>{Array.from({ length: 16 }, (_, index) => <option key={index} value={index + 1}>Channel {index + 1}</option>)}</select></label>
    </section>

    <section className="ios-midi-card" aria-label="Scene selection">
      <div className="ios-midi-card-title"><div><h2>Scenes</h2><p>Recall one of the eight scenes</p></div></div>
      <div className="ios-midi-grid ios-midi-scenes">{Array.from({ length: 8 }, (_, scene) => <button key={scene} disabled={!connected || busy} onClick={() => void sendCc(43, scene)}>{String.fromCharCode(65 + scene)}</button>)}</div>
    </section>

    <section className="ios-midi-card" aria-label="Footswitches">
      <div className="ios-midi-card-title"><div><h2>Footswitches</h2><p>Send a press to a QC footswitch</p></div></div>
      <div className="ios-midi-grid ios-midi-switches">{Array.from({ length: 8 }, (_, switchIndex) => <button key={switchIndex} disabled={!connected || busy} onClick={() => void sendCc(35 + switchIndex, 127)}>FS {switchIndex + 1}</button>)}</div>
    </section>

    <section className="ios-midi-card" aria-label="Expression pedals">
      <div className="ios-midi-card-title"><div><h2>Expression pedals</h2><p>Send heel-to-toe position (CC 1 and 2)</p></div></div>
      <label className="ios-midi-expression">Expression 1 <span>{expression1}</span><input aria-label="Expression pedal 1 position" type="range" min="0" max="127" value={expression1} disabled={!connected || busy} onChange={(event) => setExpression1(Number(event.target.value))} onPointerUp={() => void sendCc(1, expression1)} onKeyUp={() => void sendCc(1, expression1)} /></label>
      <label className="ios-midi-expression">Expression 2 <span>{expression2}</span><input aria-label="Expression pedal 2 position" type="range" min="0" max="127" value={expression2} disabled={!connected || busy} onChange={(event) => setExpression2(Number(event.target.value))} onPointerUp={() => void sendCc(2, expression2)} onKeyUp={() => void sendCc(2, expression2)} /></label>
      <p className="ios-midi-control-note">Position is sent when you release the slider. The iPhone cannot read back the QC’s expression position.</p>
    </section>

    <section className="ios-midi-card" aria-label="Mode selection">
      <div className="ios-midi-card-title"><div><h2>Modes</h2><p>Recall a configured mode slot (CC 47)</p></div></div>
      <div className="ios-midi-grid ios-midi-modes">
        <button disabled={!connected || busy} onClick={() => void sendCc(47, 0)}>Slot 1 · Preset</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(47, 1)}>Slot 2 · Scene</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(47, 2)}>Slot 3 · Stomp</button>
      </div>
      <p className="ios-midi-control-note">The QC uses the mode assigned to each slot; the names above are the factory defaults.</p>
    </section>

    <section className="ios-midi-card" aria-label="Preset selection">
      <div className="ios-midi-card-title"><div><h2>Preset</h2><p>Recall by setlist, bank, and program number</p></div></div>
      <div className="ios-midi-preset-fields">
        <label>Setlist<select value={bankLsb} onChange={(event) => setBankLsb(Number(event.target.value))}>{Array.from({ length: 13 }, (_, index) => <option key={index} value={index}>{index + 1}</option>)}</select></label>
        <label>Bank<select value={bankMsb} onChange={(event) => setBankMsb(Number(event.target.value))}><option value={0}>1</option><option value={1}>2</option></select></label>
        <label>Program<input type="number" min={1} max={128} value={program + 1} onChange={(event) => setProgram(Math.min(127, Math.max(0, Number(event.target.value) - 1)))} /></label>
        <button className="ios-midi-primary" disabled={!connected || busy} onClick={() => void sendPreset()}>Recall</button>
      </div>
    </section>

    <section className="ios-midi-card ios-midi-quick" aria-label="Quick controls">
      <div className="ios-midi-card-title"><div><h2>Quick controls</h2><p>Commands follow the Quad Cortex MIDI chart</p></div></div>
      <div className="ios-midi-grid ios-midi-quick-grid">
        <button disabled={!connected || busy} onClick={() => void sendCc(44, 127)}>Tap tempo</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(45, 127)}>Tuner on</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(45, 0)}>Tuner off</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(46, 127)}>Gig View</button>
      </div>
    </section>

    <section className="ios-midi-card" aria-label="MIDI settings">
      <div className="ios-midi-card-title"><div><h2>Duplicate preset messages</h2><p>Set whether repeated program changes are ignored (CC 62)</p></div></div>
      <div className="ios-midi-grid ios-midi-quick-grid">
        <button disabled={!connected || busy} onClick={() => void sendCc(62, 127)}>Ignore duplicates on</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(62, 0)}>Ignore duplicates off</button>
      </div>
      <p className="ios-midi-control-note">The QC does not report this setting to the iPhone over this MIDI connection.</p>
    </section>

    <section className="ios-midi-card" aria-label="Looper X MIDI controls">
      <div className="ios-midi-card-title"><div><h2>Looper X</h2><p>Requires a Looper X block in the current preset</p></div></div>
      <div className="ios-midi-grid ios-midi-looper-grid">
        <button disabled={!connected || busy} onClick={() => void sendCc(48, 0)}>Open looper</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(48, 127)}>Close looper</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(49, 127)}>Duplicate toggle</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(50, 127)}>One Shot toggle</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(51, 127)}>Half Speed toggle</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(52, 127)}>Punch In/Out</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(53, 127)}>Record/Overdub</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(54, 127)}>Play/Stop</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(55, 127)}>Reverse toggle</button>
        <button disabled={!connected || busy} onClick={() => void sendCc(56, 127)}>Undo/Redo</button>
      </div>
      <div className="ios-midi-looper-settings">
        <label>Duplicate mode<select disabled={!connected || busy} defaultValue="0" onChange={(event) => void sendCc(57, Number(event.target.value))}><option value="0">Free</option><option value="1">Sync</option></select></label>
        <label>Quantize<select disabled={!connected || busy} defaultValue="0" onChange={(event) => void sendCc(58, Number(event.target.value))}><option value="0">Off</option>{Array.from({ length: 8 }, (_, index) => <option key={index + 1} value={index + 1}>{index + 1} beats</option>)}<option value="9">16 beats</option></select></label>
        <label>MIDI clock<select disabled={!connected || busy} defaultValue="0" onChange={(event) => void sendCc(59, Number(event.target.value))}><option value="0">Stop</option><option value="1">Start</option></select></label>
        <label>Editor mode<select disabled={!connected || busy} defaultValue="0" onChange={(event) => void sendCc(60, Number(event.target.value))}><option value="0">Perform</option><option value="1">Parameters</option></select></label>
        <label>Routing mode<select disabled={!connected || busy} defaultValue="0" onChange={(event) => void sendCc(61, Number(event.target.value))}>
          {["Grid", "In 1", "In 2", "Ret 1", "Ret 2", "In 1/2", "Ret 1/2", "Out1", "Out2", "Out3", "Out4", "Out1/2", "Out3/4", "Multi"].map((name, index) => <option key={index} value={index}>{name}</option>)}
        </select></label>
      </div>
      <p className="ios-midi-control-note">Looper toggle state is not read back over MIDI. Select values are sent when changed and may not reflect the QC’s current setting.</p>
    </section>

    <p className="ios-midi-message" aria-live="polite">{message}</p>
    <p className="ios-midi-footnote">MIDI is one-way control. The Quad Cortex does not send editor state back through this iPhone connection.</p>
  </main>;
}
