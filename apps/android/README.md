# QC Remote for Android

Capacitor/React composition root for the compact Quad Cortex device surface. It shares device behavior and CorOS-style UI packages with Windows while owning Android USB-host lifecycle, permissions, screen wake behavior, and the outbound relay adapter.

The app provides:

- the shared QC display, preset, Grid, routing, scene, parameter, footswitch, history, save, and performance workflows;
- direct Android USB host control through the shared Rust state engine;
- compact portrait and landscape layouts;
- USB status, diagnostics, reconnect, and safe session recovery;
- remote MCP browser pairing and access-tier control;

The Android app contains no built-in conversational interface or speech-recognition feature. Remote automation reaches the same generated, allowlisted MCP action surface and uses the same Read-only, Performance, Modify, or Full policy as Windows.

## Using the app

The full end-user guide is [`docs/USER-GUIDE.md`](../../docs/USER-GUIDE.md).
In short:

- **Install.** Download the signed APK (`qc-remote-<version>.apk`) from the [latest release](https://github.com/freevia-org/QC-Remote/releases/latest) and open it on the phone; the release page lists its SHA-256 checksum. The `.aab` file on the same page is the Android App Bundle prepared for Google Play, not an installable package. The Google Play listing is pending an internal-test upload, so its link may not open yet. QC Remote is early, pre-1.0 software, and only the latest release receives fixes.
- **Requirements.** Android 7.0 or newer (API level 24) and a phone that can act
  as a USB host (OTG). You also need a Quad Cortex, switched on, and a cable or
  adapter that connects it to the phone. The manifest does not make USB host
  mandatory for installation, but the app can only control a unit through it.
- **Connecting.** Plug the Quad Cortex in and open QC Remote. When the unit is
  attached, Android shows its own USB confirmation dialog for the app (the app
  declares a USB attach filter for the unit); allow it. Otherwise open the
  **USB** indicator at the top right and press **Connect**.
- **Permission prompts.** Only a deliberate press of **Connect** makes the app
  raise its own USB permission request, so plugging the unit in does not
  produce two prompts. If you deny it, the app reports "USB permission was
  denied."; press **Connect** to be asked again. If the unit is unplugged while
  a prompt is open, the app reports that and clears the request.
- **No unit attached.** The app opens on a **DEMO** device whose presets stay on
  the phone. Use the panel's **Connect Quad Cortex** or **Switch to Quad
  Cortex** button to return to hardware.
- **Indicators.** **USB** and **REMOTE** at the top right open their panels. The
  USB panel reads *Quad Cortex ready*, *synchronizing*, *writes paused*,
  *connection error* or *disconnected*; *writes paused* means a write could not
  be confirmed and the unit needs to be reconnected before further edits.
- **Remote access.** **REMOTE** pairs the phone with the relay through a browser
  sign-in with Google. Remote access starts only while the Quad Cortex is
  connected and USB access is granted, and it keeps one low-priority ongoing
  notification; on Android 13 or newer the app explains this and asks for
  notification permission first. The access selector in the REMOTE panel sets
  Read-only, Performance, Modify, or Full control.
- **Screen.** The **I/O** and **GIG** buttons open the I/O and Gig View screens, as
  does swiping down from the top edge or up from the bottom edge of the Grid.
  **About > Display** has **Keep screen awake while connected** (on by
  default).

## Run and build

The prerequisites (JDK 21, Android SDK 36, NDK 27.2.12479018, cargo-ndk and
the two Android Rust targets, and `pwsh`) are listed in the root
[README](../../README.md#prerequisites). From the repository root:

```powershell
npm install
npm run dev --workspace @qc-remote/android           # the web layer alone, in a browser
npm run android:sync --workspace @qc-remote/android  # build the web layer and sync it into the Gradle project
npm run android:open --workspace @qc-remote/android  # open the Gradle project in Android Studio
```

For a debug APK, from `apps/android/android`:

```sh
sh ./gradlew assembleDebug        # on Windows: .\gradlew.bat assembleDebug
```

On Windows, `npm run android:build:debug --workspace @qc-remote/android` runs
the same build through `scripts/build-android-debug.ps1`, which also locates a
compatible JDK and regenerates the theme-derived resources first. Release
signing is not part of this repository.

The native USB plugin remains the only Android owner of the device session. UI code uses the stable shared gateway transport and does not handle raw HID or protobuf frames.
