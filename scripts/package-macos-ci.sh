#!/bin/sh
# CI has no reliable Finder session. Build a real installer without GUI cosmetics.
set -eu
app="$1"
image="$2"
test -d "$app/Contents/Resources/QC Device Broker.app"
mkdir -p "$(dirname "$image")"
stage=$(mktemp -d "${TMPDIR:-/tmp}/qc-macos-installer.XXXXXX")
trap 'rm -rf "$stage"' EXIT HUP INT TERM
ditto "$app" "$stage/QC Remote.app"
ditto "$app/Contents/Resources/QC Device Broker.app" "$stage/QC Device Broker.app"
ln -s /Applications "$stage/Applications"
hdiutil create -volname "QC Remote" -srcfolder "$stage" -format UDZO "$image"
hdiutil verify "$image"
