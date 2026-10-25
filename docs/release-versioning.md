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
workspace lock entries together. Android's integer `versionCode` and the iOS
App target's build number each advance once when their marketing version
changes. Repeating the command with the same version changes neither counter.
The excluded DriverKit prototype and library/protocol package versions retain
their independent versions. Linux derives its package version from the shared
desktop Tauri configuration.

Store approval and installed versions can still differ temporarily during a
rollout. A source bump does not cancel an existing Apple submission, replace a
published artifact, or imply that every store has approved the next release.
