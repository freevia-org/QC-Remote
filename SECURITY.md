# Security policy

QC Remote accepts responsible security reports for the prerelease source,
prerelease Windows and Android builds, and the relay Freevia EOOD operates at
`qcrc.freevia.org` (the Cloudflare Worker in `services/qc-remote-cloud`).

## How to report

Do not post suspected vulnerabilities, credentials, tokens, private device
data, or exploit details in a public issue or pull request. Report privately
by email to **security@freevia.org**.

Email is the only reporting channel this policy supports.

Include only what is needed to reproduce and assess the problem: the affected
component and version, the steps, and the impact. Do not include unrelated
user, device, account, or third-party proprietary data.

## What to expect

Receipt is acknowledged as resources permit; no fixed response time is
promised. During prerelease, fixes may be included in a later test build or
deployed directly to the relay. No public Windows or Android release is
currently supported.

## Scope

In scope: the Windows and Android apps, the device broker, the local MCP
endpoint and its bridge, and the relay's pairing, OAuth, and command routing.

Out of scope: Neural DSP firmware and products (report those to Neural DSP),
and third-party AI apps you connect to QC Remote (report those to their
publishers).
