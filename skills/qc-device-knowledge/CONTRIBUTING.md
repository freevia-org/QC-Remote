# Contributing to QC Remote

## Inbound contribution policy: Developer Certificate of Origin 1.1

QC Remote uses the **Developer Certificate of Origin, version 1.1** (DCO 1.1)
as its inbound-contribution policy. There is no Contributor License Agreement
to sign and no copyright assignment: you keep your copyright, and you certify
the origin of what you send.

Inbound equals outbound. Unless you state otherwise in writing, every
contribution you intentionally submit for inclusion in QC Remote is licensed
under the same terms as the project itself — `MIT OR Apache-2.0`, at the
recipient's option. See [`LICENSE`](LICENSE) for the grant and for the material
that grant deliberately does not cover.

### How to certify

Sign off every commit. Git writes the required trailer for you:

```bash
git commit -s -m "Your message"
```

That appends the Developer Certificate of Origin sign-off from your local Git
identity.
Use a name you are prepared to stand behind. A sign-off is a statement about
the origin of the work, so pseudonymous or anonymous sign-offs are not
accepted. To fix a commit you forgot to sign, `git commit --amend -s`; for a
series, `git rebase --signoff <base>`.

Every commit in a pull request from an outside contributor must carry the
trailer. The `DCO sign-off` CI job fails a pull request whose commits lack a
`Signed-off-by:` trailer matching the commit author's name and email. Merge
commits are not exempt, because a conflict resolution exists only in the merge:
if you merge the base branch into yours, sign the merge (`git merge --signoff`)
or rebase instead. GitHub's web "Update branch" button creates a merge commit
without a sign-off and will fail the check, so rebase or merge locally with
`--signoff`.

Maintainers are exempt: the CI job is skipped for pull requests whose author
GitHub reports as the repository owner, an organisation member or a
collaborator (`OWNER`, `MEMBER` or `COLLABORATOR`), and for pull requests opened
by a bot account (such as Dependabot). Everyone else, including contributors
with earlier merged work and first-time contributors, must sign off. This relies
on the pull request author as reported by GitHub, not on commit contents.
Anyone later added as a collaborator becomes exempt from then on, and
membership that GitHub does not report as `MEMBER` is checked rather than
exempted. A commit pushed by someone else onto a maintainer's or bot's pull
request branch needs write access to that branch, which is accepted. The job is
defined in `.github/workflows/dco.yml` and runs from the base branch, so a pull
request cannot change or skip it, nor add an exemption for itself. It can only
check that the trailer is there and names the author; whether the name is a
real one is for the maintainers to judge. The requirement applies from the first
outside contribution onward and is not retroactive: the commits the publisher,
Freevia EOOD, made before that, including the initial public release, carry no
sign-off and are released by the copyright holder directly.

### What you are certifying

The text below is the Developer Certificate of Origin 1.1, reproduced verbatim
from <https://developercertificate.org/>.

```
Developer Certificate of Origin
Version 1.1

Copyright (C) 2004, 2006 The Linux Foundation and its contributors.
1 Letterman Drive
Suite D4700
San Francisco, CA, 94129

Everyone is permitted to copy and distribute verbatim copies of this
license document, but changing it is not allowed.


Developer's Certificate of Origin 1.1

By making a contribution to this project, I certify that:

(a) The contribution was created in whole or in part by me and I
    have the right to submit it under the open source license
    indicated in the file; or

(b) The contribution is based upon previous work that, to the best
    of my knowledge, is covered under an appropriate open source
    license and I have the right under that license to submit that
    work with modifications, whether created in whole or in part
    by me, under the same open source license (unless I am
    permitted to submit under a different license), as indicated
    in the file; or

(c) The contribution was provided directly to me by some other
    person who certified (a), (b) or (c) and I have not modified
    it.

(d) I understand and agree that this project and the contribution
    are public and that a record of the contribution (including all
    personal information I submit with it, including my sign-off) is
    maintained indefinitely and may be redistributed consistent with
    this project or the open source license(s) involved.
```

Note (d): your name and email address become a permanent part of the public Git
history and cannot be removed later.

## What you must not send

Do not attach or include material you are not authorized to share, including:

- credentials, tokens, serial numbers, or personal data — yours or anyone
  else's;
- device or manual screenshots, copied product artwork, or other Neural DSP
  material;
- proprietary presets, captures, or recordings;
- copyrighted third-party media or code under terms incompatible with
  `MIT OR Apache-2.0`;
- confidential research material or non-redistributable technical evidence.

QC Remote brand assets — the project name, wordmark, and application icon — are
all rights reserved and are not covered by the project license. A fork must use
its own identity. See [`TRADEMARKS.md`](TRADEMARKS.md).

Opening an issue to report a defect transfers nothing and requires no sign-off;
the same "do not send protected material" rule applies.

## Before you open a pull request

- Make sure `.github/workflows/ci.yml` passes for your change.
- Keep the change scoped, and say in the message what you measured.
- Third-party components keep their own licenses and notices; see
  [`THIRD_PARTY-NOTICES.md`](THIRD_PARTY-NOTICES.md).

## Security

Do not report vulnerabilities through public issues or pull requests. Follow
[`SECURITY.md`](SECURITY.md).
