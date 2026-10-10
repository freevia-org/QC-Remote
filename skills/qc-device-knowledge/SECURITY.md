# Security policy

QC Remote accepts responsible security reports for the prerelease source,
prerelease Windows and Android builds, and the relay Freevia EOOD operates at
`qcrc.freevia.org` (the Cloudflare Worker in `services/qc-remote-cloud`).

## How to report

Do not post suspected vulnerabilities, credentials, tokens, private device
data, or exploit details in a public issue or pull request. Report privately
by email to **security@freevia.org**, or through GitHub private vulnerability
reporting: open the repository's **Security** tab and choose **Report a
vulnerability**.

Include only what is needed to reproduce and assess the problem: the affected
component and version, the steps, and the impact. Do not include unrelated
user, device, account, or third-party proprietary data.

## What to expect

Receipt is acknowledged as resources permit; no fixed response time is
promised. During prerelease, fixes ship in the next release or are deployed
directly to the relay. Only the latest Windows and Android release published
on GitHub Releases or Google Play receives fixes; update before reporting a
problem in an older build.

## Scope

In scope: the Windows and Android apps, the device broker, the local MCP
endpoint and its bridge, and the relay's pairing, OAuth, and command routing.

Out of scope: Neural DSP firmware and products (report those to Neural DSP),
and third-party AI apps you connect to QC Remote (report those to their
publishers).
