# 0009. Risk lanes and a verifying judge, rules first

Status: Accepted (2026-10-08)

## Context

Phase 4 decides what happens to a change after Onus has described it: merge it automatically, have a judge verify it, send it to a person, or block it. The plan (PLAN.md, Phase 4) asks for a change application (`change.submit`), a rules-first classifier configured in `onus.yaml` with hard floors, a judge that verifies before it judges taste and never sees the author's reasoning, GitHub integration through labels, checks and auto-merge, and defenses against gaming: hidden checks, random human audits, rotating judge configurations and outcome tracking.

Onus's standing rules apply: facts are computed, never generated; output is deterministic; under uncertainty, Onus is conservative. A lane is a fact about a change, so it must come from rules, not from a model.

## Decision

1. **A submission** (`onus submit`) is one JSON document an agent hands in with its change: the stated intent, Onus's semantic report of the change, the evidence it gathered (test runs from `onus run-test`), the scope it worked under (its token, verified), the escalations granted to it, and its setup (tool, model, configuration, team). Nothing in it is the author's reasoning.
2. **Lanes come from rules** (`onus classify`). Four lanes, from least to most scrutiny: `auto-merge`, `judge`, `human`, `blocked`. Rules can only move a change up:
   - **Hard floors.** Sensitive labels (`auth`, `payments`, `pii` and any label of medium or high sensitivity) go to `human`. Committed secrets, writes outside the token's scope, and weakened tests without an approval are `blocked`. Rules of the game (tests' assertions, CI, policies, labels, the judge's configuration, declared contracts, `onus.yaml`) go to `human`.
   - **Policy.** `onus.yaml` `lanes:` sets the default lane and rules matching rows by kind, subkind, label or component, plus how many changes an agent setup needs on record before it may use a lane below `judge`.
   - **Random audits.** A share of `auto-merge` changes (`audit-rate`) goes to `human`, chosen by a hash of the head commit, so the choice is deterministic and unpredictable to the author.
   The classification lists every rule that applied.
3. **The judge** (`onus judge`) verifies, in order, and stops at the first failure: re-runs the submission's test evidence in a fresh container; compares intent with effect (the report's intent check); checks contracts and boundary rules (breaking rows and rule violations); checks for weakened tests; runs held-out checks the author never saw (a command from `onus.yaml`, run in a container against the change, typically a test suite kept outside the agent's read scope). Taste comes last and weighs least: an optional external reviewer command, which may raise concerns that escalate to a person but can never approve. Onus itself calls no model. The verdict is `approve`, `reject` with structured reasons for the agent, or `escalate` to a person.
4. **GitHub.** The action takes `lanes: true` to classify and, with `apply-lane: true`, adds an `onus/lane:<lane>` label and fails the job for `blocked`. With `auto-merge: true`, an `auto-merge` lane enables GitHub's auto-merge on the pull request; required checks still apply.
5. **Outcomes** (`onus outcomes record|summary`) keep a JSONL record per change: agent setup, judge configuration, lane, verdict, and what happened (merged, reverted, incident). The summary gives each agent setup and judge configuration its record, which the classifier uses for new setups and people use to rotate judge configurations.

## Consequences

- Every lane decision is explainable and reproducible from the report, the policy and the submission.
- A model can be part of the judge only as the last, lowest-weight step, and only through a command the operator configures; it can escalate a change, never approve one.
- Held-out checks need a container engine and a suite the agent cannot read; the gateway's read scope (ADR 0008) is how to keep it hidden.
- The gate (a sampled human audit's miss rate under a threshold, and the share of changes in the human lane) is measured from outcome records once the lanes are in use.
