# 0003. Apache-2.0 license and DCO sign-off

Status: Accepted (2026-10-05)

## Context

Onus should be usable by any team with any agent, and companies will run it in their CI. We want contributing to be easy.

## Decision

License Onus under the Apache License 2.0. Contributors certify the Developer Certificate of Origin with a `Signed-off-by` line on each commit; there is no contributor license agreement.

## Consequences

- Permissive use, including commercial, with an explicit patent grant from contributors.
- A DCO check runs on every pull request.
- Dependencies must have compatible licenses; `cargo deny` enforces an allow list once the workspace exists.
