# Application versions

QC Remote uses one public application version for Windows, macOS, Linux,
Android, and iOS. Starting with 1.0.1, release notes and platform downloads use
the same x.y.z version. Earlier releases used independent desktop, Android,
and iOS counters; those published artifacts are historical and stay unchanged.

From the public repository root, prepare the next release with:

```sh
node scripts/app-version.mjs 1.0.2
npm run versions:check
```

The command updates all app manifests, native versions, the macOS broker, and
workspace lock entries together. Android's required integer `versionCode` and
Apple's required `CFBundleVersion` are derived from that canonical tuple rather
than maintained as independent counters. Google Play uses
`major * 1,000,000 + minor * 10,000 + build` as its integer `versionCode`; this
preserves tuple ordering when `minor < 100` and `build < 10,000`. Apple uses
the canonical three-part string for both marketing and build versions. The
Mac App Store helper bundle follows the same version. The excluded DriverKit
prototype and library/protocol package versions retain their independent
versions. Linux derives its package version from the shared desktop Tauri
configuration.

For example, `1.0.6` is the only manually chosen application version; its
Google Play `versionCode` is `1,000,006`, and Apple build metadata is also
`1.0.6`. Do not hand-edit or publish a store-specific counter. The version
check rejects any mismatch between public versions and derived store fields.

Store approval and installed versions can still differ temporarily during a
rollout. A source bump does not cancel an existing Apple submission, replace a
published artifact, or imply that every store has approved the next release.
Every rebuilt or replacement artifact must use a new canonical version; do not
replace bits under an already uploaded version.
