# Cross-compiles the quadcortex-android JNI cdylib (packages/rust/quadcortex-android,
# which wraps the shared `quadcortex` state engine) for the Android ABIs Gradle
# expects, and places the resulting .so files where app/build.gradle's
# `buildSharedQcRust` task looks for them:
#   apps/android/android/app/src/main/jniLibs/arm64-v8a/libquadcortex_android.so
#   apps/android/android/app/src/main/jniLibs/x86_64/libquadcortex_android.so
#
# Requires the same toolchain CI uses (.github/workflows/ci.yml) and the root
# README lists: Android NDK 27.2.12479018 (ANDROID_NDK_HOME pointed at it, or
# installed side by side under $ANDROID_HOME/ndk), cargo-ndk 4.1.2, and the
# aarch64-linux-android / x86_64-linux-android rustup targets. Install with:
#   sdkmanager "ndk;27.2.12479018"
#   cargo install cargo-ndk --version 4.1.2 --locked
#   rustup target add aarch64-linux-android x86_64-linux-android
# A different NDK or cargo-ndk is refused rather than used, so a local build
# cannot quietly differ from CI's. scripts/verify-manifest-pins.test.mjs keeps
# these versions equal to CI's and the README's.

param(
    [ValidateSet("debug", "release")]
    [string]$Profile = "debug"
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$requiredNdkVersion = "27.2.12479018"
$requiredCargoNdkVersion = "4.1.2"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo is not on PATH. Install the Rust toolchain (https://rustup.rs) before building the Android native library."
}
$cargoNdkInstall = "cargo install cargo-ndk --version $requiredCargoNdkVersion --locked"
if (-not (Get-Command cargo-ndk -ErrorAction SilentlyContinue)) {
    throw "cargo-ndk is not installed. Run '$cargoNdkInstall' before building the Android native library."
}
$cargoNdkVersionText = "$(& cargo ndk --version 2>&1)".Trim()
if ($cargoNdkVersionText -notmatch '(?:^|\s)v?(\d+\.\d+\.\d+)(?:\s|$)' -or $Matches[1] -ne $requiredCargoNdkVersion) {
    throw "cargo-ndk $requiredCargoNdkVersion is required (found '$cargoNdkVersionText'). Run '$cargoNdkInstall --force' before building the Android native library."
}

$ndkHome = $env:ANDROID_NDK_HOME
if (-not $ndkHome) {
    if (-not $env:ANDROID_HOME) {
        throw "Neither ANDROID_NDK_HOME nor ANDROID_HOME is set. Install the Android SDK/NDK and set ANDROID_HOME before building the Android native library."
    }
    # The pinned side-by-side directory, not whichever version sorts highest.
    $ndkHome = Join-Path (Join-Path $env:ANDROID_HOME "ndk") $requiredNdkVersion
    if (-not (Test-Path -LiteralPath $ndkHome -PathType Container)) {
        throw "Android NDK $requiredNdkVersion is not installed at '$ndkHome'. Run 'sdkmanager ""ndk;$requiredNdkVersion""' (or Android Studio > SDK Manager > SDK Tools > NDK, Show Package Details) before building the Android native library."
    }
}
if (-not (Test-Path -LiteralPath $ndkHome -PathType Container)) {
    throw "Android NDK directory '$ndkHome' does not exist."
}
# Every NDK records its own version in source.properties as Pkg.Revision.
$ndkProperties = Join-Path $ndkHome "source.properties"
$ndkRevision = if (Test-Path -LiteralPath $ndkProperties -PathType Leaf) {
    Get-Content -LiteralPath $ndkProperties | ForEach-Object {
        if ($_ -match '^\s*Pkg\.Revision\s*=\s*(\S+)') { $Matches[1] }
    } | Select-Object -First 1
}
if ($ndkRevision -ne $requiredNdkVersion) {
    throw "Android NDK $requiredNdkVersion is required, but '$ndkHome' is NDK '$ndkRevision'. Point ANDROID_NDK_HOME at an NDK $requiredNdkVersion install."
}
$env:ANDROID_NDK_HOME = $ndkHome

$requiredTargets = @("aarch64-linux-android", "x86_64-linux-android")
$installedTargets = @(& rustup target list --installed)
$missingTargets = $requiredTargets | Where-Object { $installedTargets -notcontains $_ }
if ($missingTargets.Count -gt 0) {
    throw "Missing rustup target(s) $($missingTargets -join ', '). Run 'rustup target add $($missingTargets -join ' ')' before building the Android native library."
}

$outputDir = Join-Path $repo "apps/android/android/app/src/main/jniLibs"
New-Item -ItemType Directory -Force -Path $outputDir | Out-Null

$cargoArgs = @("ndk", "-t", "arm64-v8a", "-t", "x86_64", "-o", $outputDir, "build", "--locked")
if ($Profile -eq "release") { $cargoArgs += "--release" }

Push-Location (Join-Path $repo "packages/rust/quadcortex-android")
try {
    Write-Output "Building quadcortex-android ($Profile) for arm64-v8a, x86_64 with cargo-ndk (NDK: $ndkHome)"
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "Android Rust build failed with exit code $LASTEXITCODE." }
} finally {
    Pop-Location
}

foreach ($abi in @("arm64-v8a", "x86_64")) {
    $expected = Join-Path $outputDir "$abi/libquadcortex_android.so"
    if (-not (Test-Path -LiteralPath $expected -PathType Leaf)) {
        throw "Expected native library was not produced: $expected"
    }
}

Write-Output "quadcortex-android native libraries written to $outputDir"
