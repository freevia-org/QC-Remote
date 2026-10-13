# QC Remote on macOS

## Install and move the app

Move `QC Remote.app` directly into `/Applications` or `~/Applications` before
enabling background login startup. The login item points to the broker inside
this app bundle, so the app must stay at that location. Foreground use and
sidecar lookup still work from another valid app-bundle location; QC Remote
refuses to register a persistent login item from a mounted disk image,
Downloads, or another staging folder. The Settings error explains how to move
the app and relaunch it; after relaunch, enable **Show menu bar item and run
broker in the background** again.

Before moving or deleting an app that has background mode enabled, disable
**Show menu bar item and run broker in the background** in Settings and wait
until Settings reports that the background broker has stopped. Disconnect the
Quad Cortex and close connected MCP clients first. Disabling background mode
removes the login item and lets the broker stop through its normal idle
shutdown.

If the GUI is already gone, stop the LaunchAgent and remove its owned plist
from Terminal. First disconnect the Quad Cortex and its clients; `bootout`
stops a broker process owned by the loaded LaunchAgent. A broker that was
started separately by the GUI follows its normal idle shutdown after the
setting is disabled:

```sh
agent="$HOME/Library/LaunchAgents/org.freevia.qcremote.resident.plist"
job="gui/$(id -u)/org.freevia.qcremote.resident"
if [ -e "$agent" ] || [ -L "$agent" ]; then
  if [ -L "$agent" ] || [ ! -f "$agent" ] || \
    [ "$(stat -f '%u' "$agent")" != "$(id -u)" ] || \
    ! grep -Fq 'QC Remote managed LaunchAgent v1' "$agent"; then
    echo "Refusing to remove an unowned LaunchAgent: $agent" >&2
    exit 1
  fi
  if launchctl print "$job" >/dev/null 2>&1; then
    launchctl bootout "$job" || exit 1
  fi
  rm "$agent"
fi
```

The commands verify that the plist is a regular file owned by your account and
contains QC Remote's managed marker before removing it. They do not delete the
application-support directory, saved preferences, credentials, or other user
data.
