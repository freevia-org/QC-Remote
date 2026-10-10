## What and why

<!-- One or two sentences: what changes, and what problem it solves. -->

## How it was checked

<!-- The commands you ran (cargo test, npm test, ...) and, for device-facing
changes, the Quad Cortex firmware version you tested against. Say so if you
could not test on hardware. -->

## Checklist

QC Remote accepts contributions under the Developer Certificate of Origin 1.1.
Read [`CONTRIBUTING.md`](../CONTRIBUTING.md) before submitting.

- [ ] Every commit is signed off (`git commit -s`), certifying the DCO 1.1.
- [ ] I own or am authorized to submit everything in this pull request, and it
      may be licensed under the project's `MIT OR Apache-2.0` terms.
- [ ] It contains no credentials, personal data, serial numbers, device/manual
      screenshots, copyrighted media, proprietary presets/captures, or copied
      Neural DSP or other third-party artwork.
- [ ] CI (`.github/workflows/ci.yml`) passes.
- [ ] Generated files (`generated_*.rs`, `generated-*.ts`, `Generated*.java`)
      are not edited by hand; contract changes are proposed against
      `contracts/public/`.
