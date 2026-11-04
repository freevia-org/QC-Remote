# Cross-platform release versions

QC Remote uses one canonical release identity in `release-version.json`:

```text
MAJOR.MINOR.CODE+BUILD
```

For example, `1.0.6+198` means customer-facing version `1.0.6`, release code `6`, and artifact build `198`.

- `MAJOR.MINOR.CODE` is the same customer-facing version on every platform.
- `BUILD` is a monotonically increasing per-app artifact number. A coordinated release uses the same value on Android, iOS, macOS, Windows, and Linux. Every rebuilt or replacement artifact gets a higher value, even when the customer-facing version is unchanged. Never reuse an uploaded version/build pair.
- Increment `CODE` for a new coordinated feature/fix release and increment `BUILD` as well. Store-specific integer counters are derived; operators do not choose them independently.
- Existing public releases and reviews remain unchanged until a replacement has passed its platform gates.

## Platform mapping

- **Google Play:** `versionName = MAJOR.MINOR.CODE`. Derive `versionCode` as `MAJOR × 100,000,000 + MINOR × 1,000,000 + CODE × 1,000 + BUILD`. Supported bounds are major ≤ 20, minor ≤ 99, code ≤ 999, and build ≤ 999. The maximum tuple maps to 2,099,999,999, below Play's 2.1 billion limit. Revisit the encoding before any component reaches its bound.
- **Apple App Store:** `CFBundleShortVersionString = MAJOR.MINOR.CODE` and `CFBundleVersion = BUILD`. iOS and macOS in the same coordinated release use the same build ordinal.
- **Microsoft Store:** use `MAJOR.MINOR.CODE.BUILD` for package versions supporting four numeric components. For EXE/MSI submissions, derive installer product/file version and the submission manifest from the same tuple.
- **Direct desktop packages:** show `MAJOR.MINOR.CODE`; include `BUILD` in native package metadata, artifact names, and provenance receipts.

The app version generator applies and checks this mapping:

```sh
node scripts/app-version.mjs 1.0.7+199
npm run versions:check
```

The checked-in source target `1.0.6+198` is the initial migration from the previous three-part scheme, based on the recorded maximum Apple build 197. It is a source candidate only; it does not change an uploaded artifact or store submission. Recheck the current store/build records before generating release artifacts.

A source version bump does not cancel an existing review, replace a published artifact, or mean that every store has approved the next release. A build is ready to upload only after its source commit, package identity, signer, platform qualification, and SHA-256 receipt are recorded.
