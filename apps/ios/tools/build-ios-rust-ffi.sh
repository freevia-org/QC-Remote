#!/bin/sh
set -eu

if [ -z "${SRCROOT:-}" ] || [ -z "${DERIVED_FILE_DIR:-}" ]; then
  echo "Xcode must provide SRCROOT and DERIVED_FILE_DIR." >&2
  exit 2
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust Cargo is required to build the shared QC iOS runtime." >&2
  exit 2
fi

manifest="${SRCROOT}/../../native-bridge/rust/Cargo.toml"
target_dir="${DERIVED_FILE_DIR}/QCRustTarget"
output="${DERIVED_FILE_DIR}/libquadcortex_ios_ffi.a"
profile="debug"
profile_args=""
if [ "${CONFIGURATION:-Debug}" = "Release" ]; then
  profile="release"
  profile_args="--release"
fi

case "${PLATFORM_NAME:-}" in
  iphoneos) target="aarch64-apple-ios" ;;
  iphonesimulator)
    case "${CURRENT_ARCH:-arm64}" in
      arm64) target="aarch64-apple-ios-sim" ;;
      x86_64) target="x86_64-apple-ios" ;;
      *) echo "Unsupported iOS simulator architecture: ${CURRENT_ARCH}" >&2; exit 2 ;;
    esac
    ;;
  *) echo "Unsupported Apple platform for QC Rust runtime: ${PLATFORM_NAME}" >&2; exit 2 ;;
esac

if command -v rustup >/dev/null 2>&1; then
  if ! rustup target list --installed | grep -Fxq "${target}"; then
    echo "Install the Rust target before building QC Remote: rustup target add ${target}" >&2
    exit 2
  fi
else
  echo "Install Rust via rustup and add the target ${target} before building QC Remote." >&2
  exit 2
fi

# Keep Cargo artifacts under Xcode's disposable DerivedData intermediates.
# shellcheck disable=SC2086
CARGO_TARGET_DIR="${target_dir}" cargo build --locked --manifest-path "${manifest}" \
  --package quadcortex-ios-ffi --target "${target}" ${profile_args}

library="${target_dir}/${target}/${profile}/libquadcortex_ios_ffi.a"
if [ ! -f "${library}" ]; then
  echo "Cargo did not produce the expected iOS static library: ${library}" >&2
  exit 1
fi
cp "${library}" "${output}"
