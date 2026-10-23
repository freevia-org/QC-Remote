#!/bin/sh
# CI has no reliable Finder session. Build a real installer without GUI cosmetics.
set -eu
app="$1"
image="$2"
test -d "$app/Contents/Resources/QC Device Broker.app"
mkdir -p "$(dirname "$image")"
stage=$(mktemp -d "${TMPDIR:-/tmp}/qc-macos-installer.XXXXXX")
trap 'rm -rf "$stage"' EXIT HUP INT TERM
mkdir "$stage/source"
ditto "$app" "$stage/source/QC Remote.app"
ditto "$app/Contents/Resources/QC Device Broker.app" "$stage/source/QC Device Broker.app"
ln -s /Applications "$stage/source/Applications"
# Hosted macOS occasionally keeps a temporary image busy while creating it.
# Retry only that transient error; publish the output only after verification.
attempt=1
while :; do
  if hdiutil create -ov -fs HFS+ -volname "QC Remote" -srcfolder "$stage/source" -format UDZO "$stage/installer.dmg" >"$stage/create.log" 2>&1; then
    cat "$stage/create.log"
    break
  fi
  cat "$stage/create.log" >&2
  if [ "$attempt" -ge 4 ] || ! grep -q 'Resource busy' "$stage/create.log"; then
    exit 1
  fi
  echo "Retrying busy disk image (attempt $((attempt + 1))/4)" >&2
  sleep "$((attempt * 2))"
  attempt=$((attempt + 1))
done
hdiutil verify "$stage/installer.dmg"
mv "$stage/installer.dmg" "$image"
