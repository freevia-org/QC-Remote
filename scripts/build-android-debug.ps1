# Produces an installable debug APK for the Android app: resolves a
# compatible JDK, regenerates the theme-derived resources and branding
# bitmaps, cross-compiles the shared Rust native library, then runs the
# actual Gradle assembly. This is the debug-only counterpart of the (not
# included in this repo) signed release pipeline -- no signing-identity
# verification, dependency-graph provenance, or legal-asset gating here,
# just a real `assembleDebug`.
#
# Usage: powershell -File scripts/build-android-debug.ps1 [-SkipRustBuild]
#
# -SkipRustBuild skips the explicit Rust cross-compile step (useful if the
# .so files were already built and Gradle's own buildSharedQcRust task will
# pick them up from cache); it does NOT skip Gradle re-verifying that the
# libraries exist, since preBuild still depends on that task.

param(
    [switch]$SkipRustBuild
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$androidAppRoot = Join-Path $repoRoot "apps\android"

# JDK 21 only, as in CI (.github/workflows/ci.yml): the generated
# apps/android/android/app/capacitor.build.gradle compiles Java 21 source and
# target, which JDK 17 cannot, and JDK 25 and newer are too new for the
# current Android Gradle Plugin. scripts/verify-manifest-pins.test.mjs keeps
# this equal to the CI and Gradle values.
$requiredJdkMajor = "21"

function Test-JavaHome([string]$candidate) {
    if (-not $candidate) { return $false }
    $java = Join-Path $candidate "bin\java.exe"
    if (-not (Test-Path -LiteralPath $java)) { return $false }
    $version = (Get-Item -LiteralPath $java).VersionInfo.ProductVersion
    return $version -match "^$requiredJdkMajor\."
}

$javaCandidates = @(
    $env:JAVA_HOME,
    "$env:ProgramFiles\Android\Android Studio\jbr"
)
$javaCandidates += Get-ChildItem "$env:ProgramFiles\Microsoft" -Directory -Filter "jdk-21*" -ErrorAction SilentlyContinue | ForEach-Object FullName
$javaHome = $javaCandidates | Where-Object { Test-JavaHome $_ } | Select-Object -First 1

if (-not $javaHome) {
    throw "Android builds require JDK $requiredJdkMajor (Gradle compiles Java $requiredJdkMajor, and JDK 25+ is too new for the current Android Gradle Plugin). Install Microsoft.OpenJDK.21 or Android Studio, then rerun this command."
}

Write-Output "Using JAVA_HOME: $javaHome"
$previousJavaHome = $env:JAVA_HOME
$previousPath = $env:Path
try {
$env:JAVA_HOME = $javaHome
$env:Path = "$(Join-Path $javaHome 'bin');$env:Path"

if (-not $SkipRustBuild) {
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $repoRoot "scripts\build-android-rust.ps1") -Profile release
    if ($LASTEXITCODE -ne 0) { throw "Android native (Rust) build failed with exit code $LASTEXITCODE." }
}

Push-Location $androidAppRoot
try {
    npm run branding:generate
    if ($LASTEXITCODE -ne 0) { throw "Android branding generation failed with exit code $LASTEXITCODE." }

    npm run android:sync
    if ($LASTEXITCODE -ne 0) { throw "Capacitor sync failed with exit code $LASTEXITCODE." }

    & node (Join-Path $PSScriptRoot "run-owned-gradle.mjs") (Join-Path $androidAppRoot "android\gradlew.bat") -p ".\android" assembleDebug
    if ($LASTEXITCODE -ne 0) { throw "Gradle assembleDebug failed with exit code $LASTEXITCODE." }
}
finally {
    Pop-Location
}

$builtApkPath = Join-Path $androidAppRoot "android\app\build\outputs\apk\debug\app-debug.apk"
if (-not (Test-Path -LiteralPath $builtApkPath -PathType Leaf)) {
    throw "Gradle reported success but the expected APK is missing: $builtApkPath"
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
$apkArchive = [System.IO.Compression.ZipFile]::OpenRead($builtApkPath)
try {
    $packagedEntries = @($apkArchive.Entries | ForEach-Object FullName)
    foreach ($nativeLibrary in @("lib/arm64-v8a/libquadcortex_android.so", "lib/x86_64/libquadcortex_android.so")) {
        if ($nativeLibrary -notin $packagedEntries) {
            throw "Debug APK is missing the shared Rust runtime for $nativeLibrary. Native library packaging is broken."
        }
    }
}
finally {
    $apkArchive.Dispose()
}

Write-Output "Debug APK built and verified: $builtApkPath"
}
finally {
    $env:JAVA_HOME = $previousJavaHome
    $env:Path = $previousPath
}
