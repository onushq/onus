# 0018. Held-out suites outside the repository, chosen from the map, and a rotating judge

Status: Accepted (2026-10-13)

## Context

ADR 0009 asks the judge to run "held-out checks the author never saw" and to stop at the first failure, and names rotating judge configurations as a defense against gaming. The manifesto adds that the map should point the judge at the combinations worth testing, because tests that combine features are the ones an author's own suite rarely has.

None of that held. The held-out check was one command in `onus.yaml`, which the agent can read in its own worktree, and `onus_lanes` handed it to agents verbatim. The same command ran for every change, and the map was never asked. Every step ran even after a failure, and every evidence run was re-run. There was one configuration, so nothing rotated.

## Decision

1. **A suite lives outside the repository.** A held-out suite is a directory holding `held-out.yaml` (`schemas/onus-held-out.schema.json`) and the files its checks need. It is given with `--held-out DIR` or `ONUS_HELD_OUT` to `onus judge`, `onus ci pr`, `onus ui` and `onus mcp`. The binary refuses a suite whose canonical path is inside the repository's worktree.
2. **Only the test container sees it.** Each check runs at the head commit with no network, the tree at `/work` and the suite copied in at `/held-out`. A setup step runs with network in its own container, without the suite.
3. **The map chooses.** The judge re-derives the changed paths with git, builds the head map (only when a check has `covers`), and works out the change's reach: touched components, row subjects, and pairs of interacting components (a code dependency, a file import, or one publishes an event the other consumes) with one side touched. File imports count so that a change which breaks the symbols between two components still pairs them. A check is chosen by its best reason: `combination`, `subject`, `touched`, `always`, `neighbour`. File order breaks ties, and a limit cuts the list. Without the map, the rows alone decide.
4. **The judge stops at the first failure.** Within evidence, the first failing run stops the step; within held-out, the first failing check. Every later step is `skipped`, `not reached: <step> failed`, and no container starts after it.
5. **The author sees counts.** The judgment gains `heldOut: {suite, selected, ran, passed}` and nothing else about the suite. What each check did goes to a separate report for people: `held-out.json` from `onus ci pr`, `onus judge --show-held-out`, and `onus ui`. `onus_lanes` says only that held-out checks are configured.
6. **Configurations rotate.** `lanes.judges` lists named configurations, each with an optional `taste` reviewer and `heldOutLimit`. The judge picks one from a hash of the head commit and the suite's full hash, and records `rotation` in the judgment. Its id is `<name>@<hash>`, so outcome records track each configuration by name with no format change. A person can force one (`--judge-config`); the agent door never forwards that choice. The agent door also hides which configuration judged: `onus_judge` drops `rotation` and gives the policy hash as the judge id, the same for every configuration, so an agent cannot amend its commit until a lenient configuration comes up. The kept judgment has both, for people. Without `judges`, the judge id is the policy hash, byte for byte as before.
7. **`lanes.heldOut` keeps working** as an untagged check with id `lanes.heldOut`, and `onus judge` notes on stderr that agents can read it.

## Consequences

- A suite is only as hidden as the place it is kept. On a machine where an agent has a shell, that is the machine's file permissions. A CI secret, a private repository cloned with its own token, or Onus Cloud keeps it out of reach.
- Without a suite, the rotation is predictable from the head commit. The suite's hash is what makes it unpredictable to an author who cannot read the suite.
- The rotation stops an author from aiming at a configuration in advance, not from shopping after the fact. An author that sees the configuration can amend and judge again until a lenient one comes up. `onus_judge` hides it, but `lane.md` from `onus ci pr` names it on the pull request, and an agent with a shell can read the judgment and the held-out report `onus ui` and `onus mcp` keep in the worktree's `.onus/submissions/`. Outcome records per configuration are the check that remains: a lenient one shows in its reverts, incidents and audit misses.
- An engine error in the held-out step can name the suite's image or where it is kept, so the author reads only that the checks could not run; the error goes to the people-only report.
- A rejection names one failing step, so an agent fixes one thing at a time. The static checklist (`onus_check`) is how it sees everything before submitting.
- The judge builds one head map per judgment when a check has `covers`. Large repositories may later reuse a cached map keyed by the head commit.
- In the GitHub Action, `judges` and `taste` are read from the pull request's worktree, so the author can edit them until a person approves the `onus.yaml` change, which the hard floor already sends to a person. Reading the policy from the base commit would close that.
- Onus Cloud stores suites and hands them to `onus ci pr --held-out`, but never selects, ranks, hashes or rotates: the binary computes every fact.
