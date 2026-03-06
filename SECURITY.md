<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# Security Policy

## Supported versions

OpenPortal is under active development. Only the most recent release receives security fixes. Older releases are not
backported. We recommend always running the latest version.

| Version | Supported |
|---|---|
| Latest release | Yes |
| Older releases | No |

Once we have a stable release (designated as `1.0.0`), we will maintain a security support policy for older versions, which will be documented here.

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security vulnerabilities.

Report security issues by email to:

**Christopher Woods** — Christopher.Woods@bristol.ac.uk

You can also use
[GitHub's private vulnerability reporting](https://github.com/isambard-sc/openportal/security/advisories/new)
if you prefer not to use email.

Please include as much of the following as you can:

- A description of the vulnerability and its potential impact
- The affected component(s) and version(s)
- Steps to reproduce or a proof-of-concept (if available)
- Any suggested mitigations

We aim to acknowledge reports within **2 working days** and to provide an
initial assessment within **5 working days**. We will keep you informed as we
work on a fix and will credit you in the release notes unless you ask to remain
anonymous.

## Security model

OpenPortal uses a distributed, no-god-key security architecture. Each pair of
agents shares independent 32-byte symmetric key pairs. Connections are
authenticated through a four-layer sequence: IP allowlist, cryptographic
handshake, zone verification, and name verification. All inter-agent
communication is double-encrypted using XChaCha20-Poly1305 with per-message
HKDF-SHA512 derived keys.

For a full description of the security model see
[docs/specifications/security-model.md](docs/specifications/security-model.md).

## Scope

Issues that are in scope include:

- Cryptographic weaknesses in the paddington wire protocol or key management
- Authentication or authorisation bypass between agents
- Privilege escalation within the agent hierarchy
- Command or instruction injection via the instruction parser
- HMAC bypass or replay attacks on the bridge HTTP API
- Information leakage between agents or zones
- Denial-of-service vulnerabilities in the core agent communication path

Issues that are **out of scope**:

- Vulnerabilities in third-party dependencies (please report these upstream;
  we will update dependencies promptly when fixes are available)
- Issues requiring physical access to the host running an agent
- Social engineering attacks
- Vulnerabilities in portal software that integrates with OpenPortal (e.g.
  Waldur) rather than in OpenPortal itself
