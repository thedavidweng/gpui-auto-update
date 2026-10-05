# Security policy

`gpui-auto-update` downloads and installs code, so it is part of its users'
software supply chain. We treat vulnerabilities in it as high priority.

## Reporting a vulnerability

**Please do not report security vulnerabilities in public issues, discussions,
or pull requests.**

Report them privately through GitHub's private vulnerability reporting:

<https://github.com/thedavidweng/gpui-auto-update/security/advisories/new>

Please include:

- the affected crate(s), version or commit, and platform;
- a description of the issue and its impact (for example: signature
  verification bypass, path traversal during extraction, privilege escalation,
  installing over an installation the updater does not own);
- steps or a proof of concept to reproduce it;
- any suggested fix.

Never include real private signing keys in a report. Use disposable keys for
reproductions.

## What to expect

- We aim to acknowledge reports within 5 business days.
- We will keep you informed while we investigate and prepare a fix.
- We will coordinate a disclosure date with you and credit you in the advisory
  unless you prefer to stay anonymous.

## Supported versions

The project is pre-release. Until 1.0, only the latest published version and
the `main` branch receive security fixes.

## Scope

In scope: the crates in this repository, the `gpui-auto-update` CLI, and the
release tooling and documentation it generates. Vulnerabilities in Sparkle
itself should be reported to the
[Sparkle project](https://github.com/sparkle-project/Sparkle/security).
