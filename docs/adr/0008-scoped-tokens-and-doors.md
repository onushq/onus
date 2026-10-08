# 0008. Scoped tokens, enforcing doors and evidence-based escalation

Status: Accepted (2026-10-08)

## Context

Phase 3 lets a coding agent work with exactly the access its task needs and makes it ask, with evidence, for more. The plan (PLAN.md, Phase 3) names the pieces: tokens minted from a task plan, doors that enforce them (a git gateway, a read-limited workspace, the MCP server), structured escalation with graded evidence, errors that teach, and an audit log. The gate is that red-team runs land zero writes outside the scope.

Three constraints shape the design:

- **The agent never holds the forge credential.** Whatever it can push, it pushes through a door that checks it.
- **Enforcement is in the door, not in the agent's goodwill.** A prompt-injected agent that tries to write elsewhere must be refused by code it cannot change.
- **Deterministic and offline.** Onus already runs without a service; tokens are verified with a public key and no network.

## Decision

1. **Biscuit tokens** (`biscuit-auth`, Apache-2.0). A token carries rights as facts, `right("write", "path", "services/notifications/**")`, `right("read", "path", …)`, `right("net", "host", …)`, `right("secret", …)`, a task id and an expiry check. Anyone holding a token can attenuate it (add checks that narrow it, for a sub-agent); nobody can widen it without the root key. Onus keeps the root private key in a file the operator controls (`onus token keygen`) and verifies with the public key alone.
2. **A task plan** (`plan.yaml`) lists the task, its write paths, and optional reads, hosts and secrets. `onus scope suggest` proposes reads from the map: the written components, what they use and what uses them, leaving out components labeled sensitive (they must be asked for). Writes are never widened by a suggestion.
3. **The git gateway** (`onus gateway serve`) is a git smart-HTTP server. The agent clones from it and pushes to it with its token as the HTTP password. The gateway:
   - serves a **read-limited mirror**: one commit whose tree holds only the readable paths of the real base commit, so files outside the read scope are not in the agent's objects or history at all;
   - receives pushes into that mirror and checks every changed path of every pushed commit against the write scope (writes must also be readable); a refused push gets an error that teaches ("`billing/invoices.ts` is outside your write scope; request `write:path:billing/invoices/**` with a failing test: `onus escalate …`");
   - **replays** accepted commits onto the real base commit (same trees for the changed paths, same messages and authors) and pushes the result to the real remote with the credential only the gateway holds.

   It uses `git http-backend` for the protocol and git's own hooks for the check, so Onus implements no wire protocol.
4. **The MCP server filters by read scope** when started with a token: answers drop symbols, files and sites outside it.
5. **Escalation** (`onus escalate`) is a structured request: kind (permission, broken test, contradictory spec, impossible task), the scope asked for, graded evidence (1 failing test, 2 trace, 3 map path, 4 draft diff, 5 rationale), and the blast radius computed from the map. `onus escalation decide` grants automatically when the requested paths are not labeled sensitive, the blast radius is small and the evidence is grade 1 and reproduces; otherwise it goes to a person (`onus escalation grant|deny`). A failing test is reproduced in a container with no network (`onus run-test`), the minimal test runner Phase 5 generalizes. A grant is a new token from the root key, with the original rights plus the narrow grant and the same expiry.
6. **The audit log** is append-only JSONL, one line per door decision (mint, attenuate, push accepted or refused, escalation, grant), each carrying the hash of the line before it; `onus audit verify` recomputes the chain.

## Consequences

- An agent's push can only change paths its token allows, and its clone holds only what it may read. Nothing in the agent's process can change that.
- Replaying commits onto the real base keeps the agent's history readable and the upstream history real; commit ids differ between the mirror and upstream, and the gateway records the mapping in the audit log.
- `git http-backend` must be available (it ships with git). Docker or Podman is needed only to reproduce failing tests.
- The gate (zero out-of-scope writes in red-team runs) is a test suite: prompt-injected pushes, path tricks (`..`, symlinks, renames out of scope, case variants) and token tampering, all refused.
