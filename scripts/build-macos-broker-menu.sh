#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
target="${1:-}"
if [ -z "$target" ]; then
  case "$(uname -m)" in
    arm64) target=aarch64-apple-darwin ;;
    x86_64) target=x86_64-apple-darwin ;;
    *) echo "Unsupported macOS architecture" >&2; exit 2 ;;
  esac
fi
bundle="$PWD/.artifacts/builds/QC Device Broker.app"
mkdir -p "$bundle/Contents/MacOS"
mkdir -p .artifacts/toolchains "$bundle/Contents/Resources/BrokerLED"
rustc --edition 2021 apps/windows/macos-broker/export-icons.rs \
  -o .artifacts/toolchains/export-broker-icons
.artifacts/toolchains/export-broker-icons "$bundle/Contents/Resources/BrokerLED"
cp apps/windows/macos-broker/Info.plist "$bundle/Contents/Info.plist"
xcrun swiftc -O -swift-version 5 -framework AppKit \
  apps/windows/macos-broker/main.swift -o "$bundle/Contents/MacOS/QCDeviceBroker"
cp "apps/windows/src-tauri/binaries/qc-device-broker-$target" "$bundle/Contents/MacOS/qc-device-broker"
codesign --force --sign - "$bundle/Contents/MacOS/qc-device-broker"
codesign --force --sign - "$bundle"
