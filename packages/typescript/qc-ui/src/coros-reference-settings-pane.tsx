import { QcHeadphonesGlyph, QcIoIcon, QcLibraryIcon, QcScreenGlyph, QcSettingsIcon, QcUiIcon, QcWifiSignalGlyph, QcDirectoryIcon, type QcScreenGlyphName } from "./theme-icons";
import { COROS_REFERENCE_VERSION, COROS_REFERENCE_VERSION_LINE, DEVICE_NAME_FALLBACK, LINUX_KERNEL_BUILD, NEWS_QR_MODULES, THIRD_PARTY_LICENSES, UBOOT_BUILD, ZENCODER_FW_APP, ZENCODER_FW_BOOTLOADER, ZENJACK_FW_APP, ZENJACK_FW_BOOTLOADER, ZENWIRELESS_FW } from "./device-reference/coros-software-info";
import { BACKUPS_UTC_NOTE, DEVICE_COPY, DEVICE_UPDATES_RUNNING, HOLD_TIMING_BODY, LATENCY_COMPENSATION_BODY, MASTER_VOLUME_ASSIGNMENT_BODY, NEWS_BODY, POWER_BUTTON_SENSITIVITY_BODY, SCENE_BYPASS_ALWAYS_OVERWRITE, SCENE_BYPASS_BODY, SCENE_BYPASS_NON_STOMP_OVERWRITE, SEND_REPORT_BODY, SEND_REPORT_TITLE, STOMP_BYPASS_BODY, SWAP_TEMPO_TUNER_BODY } from "./device-reference/coros-screen-text";
import { QcSegmentBar } from "./qc-segment-bar";
import { QcPillToggle } from "./qc-pill-toggle";
import { QcRadioButton } from "./qc-radio-button";

export type ReferenceSettingsView = "settings-support" | "settings-wifi" | "settings-storage" | "settings-info" | "settings-diagnostics" | "settings-system-power" | "settings-system-volume" | "settings-update-idle" | "settings-system-reset" | "settings-device-scene-bypass" | "settings-device-stomp-bypass" | "settings-device-hold-timing" | "settings-device-swap-tempo-tuner" | "settings-device-gig-view" | "settings-device-latency" | "settings-support-report" | "settings-support-licenses" | "settings-account-backups" | "settings-restart-confirm";

function QcNewsQr({ className }: { className: string }) {
  return (
    <svg className={className} viewBox="-1.75 -1.75 28.75 28.75" aria-hidden="true" shapeRendering="crispEdges">
      <rect x="-1.75" y="-1.75" width="28.75" height="28.75" fill="white" />
      {NEWS_QR_MODULES.flatMap((row, y) => [...row].map((module, x) => (module === "1" ? <rect key={`${x}-${y}`} x={x} y={y} width="1" height="1" fill="black" /> : null)))}
    </svg>
  );
}

// Each settings row resolves to the registry name of the device asset it
// draws, so the rows whose word another family also claims - the System
// pane's updates/brightness/volume/storage, Account's cloud, Device's MIDI -
// reach `system-`, `account-` and `device-` names instead of a bare leaf.
const REFERENCE_SETTINGS_ROW_GLYPHS: Record<string, QcScreenGlyphName> = {
  support: "contact", power: "power-functions", "gig-access": "gig-view", bypass: "global-bypass",
  cloud: "account-cloud", midi: "device-midi", updates: "system-updates", brightness: "system-brightness",
  about: "about", device: "device", hold: "hold", info: "info", latency: "latency", licenses: "licenses",
  report: "report", diagnostics: "diagnostics", "scene-bypass": "scene-bypass", "stomp-bypass": "stomp-bypass",
  swap: "swap", wifi: "wifi", system: "system", user: "user", backup: "backup"
};

/** The Device pane's rows, shared by the live pane and the reference ones. */
export const SETTINGS_DEVICE_ROWS: ReadonlyArray<readonly [string, string]> = [
  ["bypass", "Global Bypass"],
  ["scene-bypass", "Scene Bypass Behavior"],
  ["stomp-bypass", "Stomp Mode Bypass"],
  ["hold", "Hold Timing"],
  ["swap", "Swap Tempo and Tuner"],
  ["gig-access", "Gig View Access"],
  ["latency", "Latency Compensation"],
  ["midi", "MIDI"],
];

// `volume` and `storage` are not in the table above and cannot be: the unit
// draws both in two inks, and a `currentColor` mask has one. They route to the
// components that draw those two controls at a value - see QcSettingsIcon.
export function ReferenceSettingsIcon({ kind }: { kind: string }) {
  if (kind === "headphones") return <QcHeadphonesGlyph />;
  if (kind === "volume" || kind === "storage") return <QcSettingsIcon kind={kind} />;
  return <QcScreenGlyph kind={REFERENCE_SETTINGS_ROW_GLYPHS[kind] ?? "factory"} />;
}

function StorageKindIcon({ kind }: { kind: "presets" | "captures" | "irs" }) {
  if (kind === "presets") return <QcDirectoryIcon kind="grid" />;
  if (kind === "captures") return <QcLibraryIcon kind="capture-header" />;
  return <QcLibraryIcon kind="impulse-response" />;
}

export interface CorOsDeviceInformation {
  serial?: string;
  deviceName?: string;
  corOsVersion?: string;
  macAddress?: string;
}

/**
 * The detail pane of every reference Settings view, drawn once for the fixture
 * frames and for the live Settings screen's rows that have no gateway call
 * (Connection, Updates, Power Functions, Factory Reset, About and Contact, Send
 * Report, Diagnostics, 3rd Party Licenses, Backups): the unit's own pane,
 * inert. The caller supplies the section element that carries the view class
 * the stylesheets key on.
 */
export function CorOsReferenceSettingsPane({ view, deviceInformation, stateUnknown = false }: { view: ReferenceSettingsView; deviceInformation?: CorOsDeviceInformation; /** Draw a pane whose state the gateway cannot read without the reference frame's own selection (Power Functions' sensitivity). */ stateUnknown?: boolean }) {
  const liveInformation = view === "settings-info" && deviceInformation !== undefined;
  return (
    <>
    {view === "settings-support" && (
      <>
        {/* The unit's Support pane is a publisher card over a contact
            paragraph, and this screen keeps that shape while saying who
            publishes QC Remote instead of reproducing the manufacturer's
            own publisher information and contact endpoints - see
            TRADEMARKS.md. The substitution keeps the device's own
            layout: four card lines on a 23px pitch and four contact
            lines beneath them, so the stand-in text is written to those
            four-line shapes rather than to whatever length the sentence
            happened to reach. */}
        <h1>About Us</h1>
        <div className="support-company">
          <span>
            <strong>QC Remote</strong>
            <br />
            Independent app
            <br />
            Not affiliated
            <br />
            Publisher website
          </span>
          <b>QC Remote</b>
        </div>
        <hr />
        <div className="support-contact">
          <span>
            For help with QC Remote, use the app
            <br />
            on the connected computer.
            <br />
            <br />
            Support for the device itself remains with
            <br />
            the hardware vendor.
          </span>
          <QcNewsQr className="support-news-qr" />
        </div>
      </>
    )}
    {view === "settings-diagnostics" && (
      <div className="reference-list">
        {["DSP Diagnostics", "Footswitch Statistics", "USB Statistics"].map((label) => (
          <button key={label}>
            {label}
            <span>
              <QcUiIcon kind="disclosure-right" />
            </span>
          </button>
        ))}
      </div>
    )}
    {view === "settings-storage" && (
      <>
        <h1>Device Storage</h1>
        <div className="storage-captured">
          {[
            ["presets", "My Presets", "270/3072", 9],
            ["captures", "My Captures", "65/2048", 3],
            ["irs", "My Impulse Responses", "0/2048", 0],
          ].map(([kind, label, value, amount]) => (
            <div key={String(label)}>
              <span>
                <StorageKindIcon kind={kind as "presets" | "captures" | "irs"} />
                <strong>{label}</strong>
                <i>
                  <QcUiIcon kind="disclosure-right" />
                </i>
                <em>{value}</em>
              </span>
              <b>
                <i style={{ width: `${amount}%` }} />
              </b>
            </div>
          ))}
        </div>
      </>
    )}
    {view === "settings-system-power" && (
      <>
        <h1>Power Functions</h1>
        <h2>Power Button Sensitivity</h2>
        <p>
          {POWER_BUTTON_SENSITIVITY_BODY[0]}
          <br />
          {POWER_BUTTON_SENSITIVITY_BODY[1]}
          <br />
          {POWER_BUTTON_SENSITIVITY_BODY[2]}
        </p>
        <div className="power-scale">
          {["Off", "Low", "Medium", "High"].map((label) => (
            <span key={label} className={label === "High" && !stateUnknown ? "is-active" : ""}>
              {label}
            </span>
          ))}
        </div>
        <QcSegmentBar className="power-bar" segments={4} filled={stateUnknown ? 0 : 4} height={22} label="Power consumption" />
        <hr />
        <button className="power-restart">RESTART</button>
      </>
    )}
    {view === "settings-system-volume" && (
      <>
        <h1>Master Volume Knob Assignment</h1>
        <p className="volume-lead">{MASTER_VOLUME_ASSIGNMENT_BODY}</p>
        <hr />
        <div className="volume-assignments">
          {["OUT 1/2", "OUT 3/4", "SEND 1/2"].map((label) => (
            <span key={label}>
              <em>{label}</em>
              <i>
                <QcUiIcon kind="check" />
              </i>
            </span>
          ))}
          <span>
            <em className="is-headphones" aria-label="Headphones">
              <QcIoIcon kind="headphones-symbol" />
            </em>
            <i>
              <QcUiIcon kind="check" />
            </i>
          </span>
        </div>
      </>
    )}
    {view === "settings-info" && (
      <>
        <h1>Device information</h1>
        <div className="information-table">
          <span>
            <b>Serial number:</b>
            <i>{deviceInformation?.serial ?? ""}</i>
          </span>
          <span>
            <b>Device name:</b>
            <i>{deviceInformation?.deviceName ?? DEVICE_NAME_FALLBACK}</i>
          </span>
          <span>
            <b>MAC address:</b>
            <i>{deviceInformation?.macAddress ?? (liveInformation ? "Not exposed" : "")}</i>
          </span>
        </div>
        <hr />
        <h1>Software information</h1>
        <div className="information-table">
          <span>
            <b>CorOS:</b>
            <i>{deviceInformation?.corOsVersion ?? COROS_REFERENCE_VERSION}</i>
          </span>
          {!liveInformation && <>
          <span>
            <b>Linux kernel:</b>
            <i>
              {LINUX_KERNEL_BUILD[0]}
              <br />
              {LINUX_KERNEL_BUILD[1]}
            </i>
          </span>
          <span>
            <b>U-Boot:</b>
            <i>
              {UBOOT_BUILD[0]}<br />
              {UBOOT_BUILD[1]}
            </i>
          </span>
          <span>
            <b>Zenjack FW app:</b>
            <i>{ZENJACK_FW_APP}</i>
          </span>
          <span>
            <b>Zenjack FW bootloader:</b>
            <i>{ZENJACK_FW_BOOTLOADER}</i>
          </span>
          <span>
            <b>Zencoder FW app:</b>
            <i>{ZENCODER_FW_APP}</i>
          </span>
          <span>
            <b>Zencoder FW bootloader:</b>
            <i>{ZENCODER_FW_BOOTLOADER}</i>
          </span>
          <span>
            <b>Zenwireless FW:</b>
            <i>{ZENWIRELESS_FW}</i>
          </span>
          </>}
        </div>
      </>
    )}
    {["settings-device-stomp-bypass", "settings-device-swap-tempo-tuner", "settings-device-gig-view", "settings-device-latency"].includes(view) &&
      (() => {
        const pane = (
          {
            "settings-device-stomp-bypass": {
              title: "Stomp Mode Bypass Assignment",
              body: [STOMP_BYPASS_BODY],
              label: "AUTO-ASSIGN",
              options: ["Enabled", "Disabled (Factory Default)"],
              active: 1,
            },
            "settings-device-swap-tempo-tuner": {
              title: "Swap Tempo and Tuner Access",
              body: [SWAP_TEMPO_TUNER_BODY],
              label: "SWAP TEMPO AND TUNER",
              options: ["Yes", "No (Factory Default)"],
              active: 1,
            },
            "settings-device-gig-view": {
              title: "Gig View Footswitch Access",
              body: DEVICE_COPY.gigView,
              options: ["On", "Off (Factory Default)"],
              active: 1,
            },
            "settings-device-latency": {
              title: "Dynamic Latency Compensation",
              body: [LATENCY_COMPENSATION_BODY],
              options: ["Enabled (Factory Default)", "Disabled"],
              active: 0,
            },
          } as Record<
            string,
            {
              title: string;
              body: readonly (string | readonly [string, string])[];
              label?: string;
              options: string[];
              active: number;
            }
          >
        )[view];
        return (
          <>
            <h1>{pane.title}</h1>
            {pane.body.map((text, index) => (
              <p key={index} className={`settings-body is-${index}`}>
                {typeof text === "string" ? (
                  text
                ) : (
                  <>
                    {text[0]}
                    <QcUiIcon kind="down" />
                    {text[1]}
                  </>
                )}
              </p>
            ))}
            {pane.label && <span className="settings-field-label">{pane.label}</span>}
            <div className="settings-toggle">
              <QcPillToggle index={pane.active} />
              {pane.options.map((option, index) => (
                <span key={option} className={index === pane.active ? "is-active" : ""}>
                  {option}
                </span>
              ))}
            </div>
          </>
        );
      })()}
    {view === "settings-device-scene-bypass" && (
      <>
        <h1>Scene Bypass State Behavior</h1>
        {/* The device breaks this lead after "state", after "the", and
            sets "active Scene." alone. No container width can be told to
            reproduce that: the same box has to keep the swap-tempo-tuner
            pane's first line whole, so any edge narrow enough to push
            "of" down takes that pane's last word with it. The break is
            stated, the way every other pane in this fixture states the
            device's. */}
        <p className="settings-body is-0">
          {SCENE_BYPASS_BODY[0]}
          <br />
          {SCENE_BYPASS_BODY[1]}
          <br />
          {SCENE_BYPASS_BODY[2]}
        </p>
        <div className="scene-bypass-options">
          {[
            [SCENE_BYPASS_ALWAYS_OVERWRITE, true],
            [SCENE_BYPASS_NON_STOMP_OVERWRITE, false],
            [DEVICE_COPY.sceneBypassThird, false],
          ].map(([text, active], index) => (
            <section key={index}>
              <p>{text}</p>
              <QcRadioButton selected={Boolean(active)} plate />
            </section>
          ))}
        </div>
      </>
    )}
    {view === "settings-support-report" && (
      <>
        <h1>{SEND_REPORT_TITLE}</h1>
        <p className="settings-body is-0">{SEND_REPORT_BODY}</p>
        <button className="report-send">SEND REPORT</button>
      </>
    )}
    {view === "settings-support-licenses" && (
      <>
        <h1>3rd party Licenses</h1>
        <div className="licenses-list">
          {THIRD_PARTY_LICENSES.map((name) => (
            <button key={name}>
              {name}
              <i />
            </button>
          ))}
        </div>
        <span className="licenses-scrollbar" />
      </>
    )}
    {view === "settings-account-backups" && (
      <>
        <h1>
          Cloud Backups <em>3/5</em>
        </h1>
        <span className="backups-utc">{BACKUPS_UTC_NOTE}</span>
        <div className="backups-list">
          {[0, 1, 2].map((index) => (
            <section key={index}>
              <i />
            </section>
          ))}
        </div>
        <button className="backups-new">NEW CLOUD BACKUP</button>
      </>
    )}
    {view === "settings-device-hold-timing" && (
      <>
        <h1>Hold Timing</h1>
        <p className="settings-body is-0">{HOLD_TIMING_BODY}</p>
        <div className="hold-scale">
          {["500ms", "600ms", "700ms", "800ms", "900ms", "1000ms"].map((label, index) => (
            <span key={label} className={index === 3 ? "is-active" : ""}>
              {label}
            </span>
          ))}
        </div>
        <QcSegmentBar className="hold-bar" segments={6} filled={4} label="Hold timing" />
      </>
    )}
    {view === "settings-system-reset" && (
      <>
        <h1>Choose a Recovery option</h1>
        <p className="reset-lead">
          <strong>{DEVICE_COPY.resetSettings.slice(0, 14)}</strong>
          {DEVICE_COPY.resetSettings.slice(14)}
        </p>
        <button className="reset-settings">RESET SETTINGS</button>
        <p className="reset-factory-lead">
          <strong>{DEVICE_COPY.factoryReset.slice(0, 13)}</strong>
          {DEVICE_COPY.factoryReset.slice(13)}
        </p>
        <button className="reset-factory">FACTORY RESET</button>
      </>
    )}
    {view === "settings-update-idle" && (
      <>
        <h1>Device Updates</h1>
        <p className="update-version">
          {DEVICE_UPDATES_RUNNING}
          <br />
          <strong>{deviceInformation?.corOsVersion ? `CorOS: ${deviceInformation.corOsVersion}` : COROS_REFERENCE_VERSION_LINE}</strong>
        </p>
        <hr />
        <button className="update-check">CHECK FOR UPDATES</button>
        <div className="update-news">
          <h2>News</h2>
          <p>
            {NEWS_BODY[0]}
            <br />
            {NEWS_BODY[1]}
            <br />
            {NEWS_BODY[2]}
          </p>
          <QcNewsQr className="update-news-qr" />
        </div>
      </>
    )}
    {view === "settings-wifi" && (
      <>
        <header className="wifi-header">
          <h1>Internet Connected</h1>
          <button>Domain Settings</button>
          <button>Internet Check</button>
        </header>
        <div className="wifi-network">
          <span>
            <QcScreenGlyph kind="wifi" />
          </span>
          {/* The network name is deliberately withheld: nothing is drawn
              here. */}
          <em>Weak connection</em>
          <i>
            <QcDirectoryIcon kind="trash" />
          </i>
          <strong><QcWifiSignalGlyph strength={2} /></strong>
        </div>
        <div className="wifi-secondary">
          <span>
            <QcScreenGlyph kind="wifi" />
          </span>
          <strong><QcWifiSignalGlyph strength={4} /></strong>
        </div>
        <button className="wifi-reset">RESET WI-FI SETTINGS</button>
      </>
    )}
    </>
  );
}
