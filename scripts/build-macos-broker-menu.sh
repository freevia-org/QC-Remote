#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
# The Rust target the bundle is for: the first argument, else
# QC_MACOS_TARGET (CI cross-builds x86_64 on Apple Silicon through
# `npm run tauri:build:mac`), else the host architecture.
target="${1:-${QC_MACOS_TARGET:-}}"
if [ -z "$target" ]; then
  case "$(uname -m)" in
    arm64) target=aarch64-apple-darwin ;;
    x86_64) target=x86_64-apple-darwin ;;
    *) echo "Unsupported macOS architecture" >&2; exit 2 ;;
  esac
fi
case "$target" in
  aarch64-apple-darwin) swift_arch=arm64 ;;
  x86_64-apple-darwin) swift_arch=x86_64 ;;
  *) echo "Unsupported macOS target: $target" >&2; exit 2 ;;
esac
# Match LSMinimumSystemVersion in macos-broker/Info.plist. Without -target,
# swiftc builds for the host architecture and the host's macOS version.
swift_target="$swift_arch-apple-macos13.0"
bundle="$PWD/.artifacts/builds/QC Device Broker.app"
mkdir -p "$bundle/Contents/MacOS"
mkdir -p .artifacts/toolchains "$bundle/Contents/Resources/BrokerLED"
rustc --edition 2021 apps/windows/macos-broker/export-icons.rs \
  -o .artifacts/toolchains/export-broker-icons
.artifacts/toolchains/export-broker-icons "$bundle/Contents/Resources/BrokerLED"
cp apps/windows/macos-broker/Info.plist "$bundle/Contents/Info.plist"
xcrun swiftc -O -swift-version 5 -target "$swift_target" -framework AppKit \
  apps/windows/macos-broker/main.swift -o "$bundle/Contents/MacOS/QCDeviceBroker"
cp "apps/windows/src-tauri/binaries/qc-device-broker-$target" "$bundle/Contents/MacOS/qc-device-broker"
codesign --force --sign - "$bundle/Contents/MacOS/qc-device-broker"
codesign --force --sign - "$bundle"
