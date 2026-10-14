# Architecture Decision Records

Short records of decisions that are significant or hard to reverse. Propose a new one as a pull request: copy the format of an existing record, number it next, and set its status to "Proposed".

| ADR | Decision | Status |
| --- | --- | --- |
| [0001](0001-rust-for-engine-and-cli.md) | Rust for the engine and the CLI | Accepted |
| [0002](0002-tree-sitter-for-typescript.md) | tree-sitter for analyzing TypeScript | Accepted |
| [0003](0003-apache-2-and-dco.md) | Apache-2.0 license and DCO sign-off | Accepted |
| [0004](0004-map-and-report-format-additions.md) | Additions to the map and report formats | Proposed |
| [0005](0005-phase-1-diff-behavior.md) | Phase 1 semantic diff behavior | Proposed |
| [0006](0006-pluggable-map-building.md) | Pluggable map building, language-author tooling and trusted mode | Proposed |
| [0007](0007-map-server-for-agents.md) | An in-memory map server for agents, without a graph database | Proposed |
| [0008](0008-scoped-tokens-and-doors.md) | Scoped tokens, enforcing doors and evidence-based escalation | Accepted |
| [0009](0009-risk-lanes-and-the-judge.md) | Risk lanes and a verifying judge, rules first | Accepted |
| [0010](0010-evidence-factory.md) | Environments, the evidence store, traces, the production loop and contract-level scopes | Accepted |
| [0011](0011-web-interface.md) | A web interface shipped in the binary | Accepted |
| [0012](0012-agents-act-through-mcp.md) | Agents hand work in through MCP, and cannot approve themselves | Accepted |
| [0013](0013-outcome-records-from-ci.md) | Outcome records kept by CI on a branch of the repository | Accepted |
| [0014](0014-built-in-risk-inputs.md) | Built-in risk inputs that only raise a lane | Accepted |
| [0015](0015-learned-lanes-only-raise.md) | A learned model that only raises a lane, read from the base commit | Accepted |
| [0016](0016-production-signals-and-rollout.md) | Production signals in the records, a rollout plan per lane, and decisions Onus makes but never executes | Accepted |
| [0017](0017-coverage-into-the-map.md) | Coverage into the map: changed lines tests ran, and how each test link is known | Accepted |
| [0018](0018-held-out-suites-and-judge-rotation.md) | Held-out suites outside the repository, chosen from the map, and a rotating judge | Accepted |
| [0019](0019-behavior-diff.md) | A behavior diff: the tests that reach a change, run at the base and at the head | Accepted |
