# Security policy

Onus sits between coding agents and codebases, so we take security reports seriously.

## Reporting a vulnerability

Please **do not open a public issue** for a security problem.

Report it privately through GitHub: go to the repository's **Security** tab and choose **Report a vulnerability** ([direct link](https://github.com/onushq/onus/security/advisories/new)). Include what you found, how to reproduce it, and the impact you expect.

We will acknowledge the report within 3 working days, keep you informed while we fix it, and credit you in the advisory unless you prefer otherwise.

## Supported versions

Onus is pre-alpha and has no releases yet. Once releases begin, security fixes go into the latest minor release.

## Scope

In scope: the `onus` CLI and its crates, the GitHub Action in `action/`, and release artifacts. Examples that matter especially for Onus:

- Analyzing a repository causes code from that repository to run.
- A pull request can make a report state something false, or hide a change, to slip past review.
- A scoped token can be widened, or a door can be passed without one (from Phase 3).
