# Security policy

GSwitch handles password-equivalent Codex credential documents locally. Please
report security issues privately and never include real credentials, API keys,
auth JSON, reset-credit IDs, screenshots containing them, or raw provider
responses in a public Issue, pull request, or discussion.

## Reporting a vulnerability

Use [GitHub private vulnerability reporting for GSwitch](https://github.com/ginbing/GSwitch/security/advisories/new).
If that link is unavailable, do not open a public report; contact the project
maintainer through a private GitHub channel and include only a sanitized
description.

Include:

- the affected GSwitch version or commit;
- the operating system and installed Codex version, if relevant;
- a minimal, sanitized reproduction and expected versus observed behavior; and
- the potential impact and any suggested mitigation.

We will acknowledge a valid report privately, investigate it, and coordinate a
fix and disclosure timing with the reporter. Security fixes are developed on
`main`; supported released versions will be recorded with their release notes.

## Scope

Relevant reports include accidental credential exposure, unsafe credential-file
mutation, bypass of process or recovery safeguards, unauthorized use of reset
credits, and packaged-app permission or update flaws.

The durable technical guarantees and security boundaries are owned by
[docs/security.md](docs/security.md). This policy is only the reporting path;
it does not add a second security design or promise support for unreleased
versions.
