# 0013. Outcome records kept by CI on a branch of the repository

Status: Accepted (2026-10-09)

## Context

Lanes earn trust from track records: an agent setup may auto-merge only after enough merged changes and no recent incident, and the miss rate of human audits says whether the automatic lanes can be trusted. Until now those records were typed by hand with `onus outcomes record`, so in practice they were empty, and the lanes that depend on them never opened. The facts are already in git and on the pull request: who made it, its lane, whether it merged, whether it was reverted.

## Decision

1. **CI records outcomes.** `onus ci pr` classifies each pull request and appends what it knew to `pulls.jsonl`; `onus ci closed` turns the latest of those into an outcome when the pull request closes; `onus ci push` records reverts on the default branch. The GitHub Action runs them with `records: true`.
2. **The records live on a branch of the repository**, `onus/records`, as append-only JSON Lines files. No service, database or token beyond the workflow's own. Concurrent runs merge by taking every line from both sides and retry the push.
3. **People add what CI cannot see with `/onus` comments**: `approve <row>`, `audit ok|miss`, `incident <note> involved: …`. Only owners, members and collaborators may record; the comment's author is kept with the record.
4. **The agent setup is read from git and GitHub**, in a fixed order: an `Onus-Agent:` trailer, a branch prefix, a bot author, a `Co-authored-by` trailer, else the person. No model is asked.
5. An incident counts against its agent setup even after the change is reverted.

## Consequences

- Track records accumulate from the first day the action runs, so auto-merge eligibility is earned from real history rather than declared.
- Agents still cannot write their own record (ADR 0012): the records are written by CI from events and by people with write access.
- Pull requests from forks get read-only tokens and are classified but not recorded.
- Agent detection by branch or bot name is a convention; a setup that wants its model and configuration counted separately adds the `Onus-Agent:` trailer.
