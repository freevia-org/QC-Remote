#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
export MACOSX_DEPLOYMENT_TARGET=13.0
case "$(uname -m)" in
  arm64) arch=arm64; target=aarch64-apple-darwin ;;
  x86_64) arch=x86_64; target=x86_64-apple-darwin ;;
  *) echo 'Unsupported macOS architecture' >&2; exit 2 ;;
esac
build_target="${CARGO_TARGET_DIR:-$PWD/apps/windows/src-tauri/target}"
build_target="$(python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "$build_target")"
export CARGO_TARGET_DIR="$build_target"
npm run build --workspace @qc-remote/windows
cargo build --release --locked --manifest-path apps/windows/src-tauri/Cargo.toml \
  --features macos-app-store,desktop-broker,custom-protocol --bins
python3 scripts/build-macos-store-helpers.py --arch "$arch" --broker "$build_target/release/qc-device-broker"
# The Store config replaces the default resource object with an empty array
# and copies its exact helpers/legal resources through macOS.files instead.
# This prevents recursive config merging from retaining the direct helper.
cp "$build_target/release/qc-device-broker" "apps/windows/src-tauri/binaries/qc-device-broker-$target"
# Bundle assembly can retain resources from a previous flavor. Remove only
# this rebuildable output app, after checking its identity and refusing links.
python3 - "$build_target/release/bundle/macos/QC Remote.app" <<'PYAPP'
import pathlib, plistlib, shutil, sys
app = pathlib.Path(sys.argv[1])
if app.is_symlink():
    raise SystemExit('Refusing a symlink at the bundle output path')
if app.exists():
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    if info.get('CFBundleIdentifier') != 'org.freevia.qcremote':
        raise SystemExit('Refusing to replace an unrelated bundle output')
    shutil.rmtree(app)
PYAPP
cd apps/windows
npm exec -- tauri build \
  --config src-tauri/tauri.appstore.conf.json --bundles app
