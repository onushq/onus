# 0015. A learned model that only raises a lane

Status: Accepted (2026-10-13)

## Context

ADR 0009 says a lane is a fact about a change, so it "must come from rules, not from a model". The manifesto asks for more once outcome data exists: "Rules first, models later. Add a learned model once you have outcome data: which changes caused incidents, rollbacks or reverts." It also sets the limit: "Rules are a floor. A model can move a change up a lane, never below the lane a rule assigns." Outcome records said what happened to a change, but nothing about the change itself, so nothing could be learned from them.

## Decision

This amends ADR 0009: a lane may also come from a model, under these conditions.

1. **Features are recorded with the change.** Every classification computes a fixed, named set of numbers from the report, the submission and the outcome history (feature set 1). CI keeps them on the pull record, and `onus ci closed` copies them onto the outcome. Reverts, incidents, audits and `/onus caught` carry them on.
2. **Labels are computed from the records.** A change is a problem when it had an incident, was reverted or rolled back, an audit found a miss, or a person with write access said `/onus caught`. It is clean when it merged and stayed clean for 14 days (`--settle-days`), measured at the latest record's time, not the clock. Everything else is left out.
3. **The model is deterministic and committed.** `onus lanes train` fits a class-balanced, L2-regularised logistic regression by full-batch gradient descent from zero, with no randomness. The same records, in any line order, give the same file, byte for byte. A person reviews and commits the file and turns it on with `lanes.learned` in onus.yaml. The model file is a rule of the game (`lanes-model-changed`), so a change to it goes to a person.
4. **It is read from the base commit.** A pull request cannot delete or retrain the model to avoid a raise: the classification uses the base commit's file. `onus ci pr --model FILE` hands in a model from elsewhere (Onus Cloud trains one per organisation); it still needs `lanes.learned`.
5. **It only raises.** It runs after the hard floors and before the record and the audit. At or above `lanes.learned.judge` (default 0.6) a lower lane goes to the judge; at or above `lanes.learned.human` (0.85) to a person. It never lowers a lane, never blocks, and never removes a rule's reason. A raise is listed with `source: "model"`, the risk, the thresholds, the top three factors in plain words, the model's id and how many outcomes trained it.
6. **A missing, stale or unreadable model is never an error.** The lane then comes from rules only, and the classification says why.

## Consequences

- Rules stay the floor. Turning the model off, or deleting it on the default branch, gives exactly the rules-only lanes.
- Outcomes are shaped by the lanes changes got: a change a person reviewed was less likely to go wrong. The model learns what went wrong despite the lanes. That is the right target for raising a lane, but it is not a causal estimate of risk.
- Class balancing makes the risk a relative score, not a calibrated probability. Thresholds are tuned with the held-out fifth that `onus lanes train` prints.
- Adding a fact to the features (line coverage, say) is a new feature set. Models trained on the old set are reported as stale and stay inert until retrained.
- Training stays a person's act in the CLI. A hosted service may schedule it, but the binary computes the features, labels, model and raise.
