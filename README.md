# Onus

**Code is not the product.** Agents now write code faster than anyone can read it. Onus is the layer every coding agent works through: it reads each change for what it means, routes it by risk, and approves it on evidence instead of on someone pretending to review 2,000 lines.

The idea behind Onus in one sentence: changes should be understood as shifts in meaning, scored by risk, and approved by evidence rather than by someone reading lines.

> **Status: pre-alpha, planning.** There is no code yet. The design lives in [PLAN.md](PLAN.md), and work starts with Phase 1. Watch the repository or join [Discussions](https://github.com/onushq/onus/discussions) to follow along.

## What it will do

Given a pull request, Onus describes it as a few changes in meaning instead of thousands of changed lines. For the manifesto's example, "Text customers when their order ships" (about 1,400 lines across 23 files), the report reads:

| Change | Kind | Why it matters |
| --- | --- | --- |
| `notifications` now calls an external SMS provider | New external dependency | Customer phone numbers leave the system |
| `UserPreferences` gains an optional `phoneVerified` field | Contract change, additive | Shared contract; existing callers unaffected |
| `notifications` subscribes to the `OrderShipped` event | New event consumer | Additive; `orders` itself is unchanged |
| About 1,400 lines of new code and tests inside `notifications` | Internal | No other component touched |

The first row is the one that deserves a person's attention, so it comes first.

## Roadmap

Each phase is useful on its own and moves on only when a measurable gate is passed.

1. **Semantic change reports** on today's pull requests, from a CLI and a GitHub Action. *(next)*
2. **The map as a service**: agents and people query components, contracts, owners and tests over MCP and the CLI.
3. **Scoped tokens and evidence-based escalation**, enforced at a git gateway and the MCP servers.
4. **Risk lanes and a verifying judge** that re-runs the evidence instead of giving an opinion.
5. **Evidence factory and production loop**: disposable environments, and outcomes that retrain the routing.

Details, decisions and acceptance criteria are in [PLAN.md](PLAN.md).

## Built with

Rust (one static binary, `onus`), tree-sitter for parsing, and the official Rust MCP SDK later on. TypeScript and JavaScript are the first languages Onus analyzes. Onus works with any agent that can run a CLI or speak MCP: Codex, Claude Code, or one you built yourself.

## Contributing

Onus is open source under the [Apache License 2.0](LICENSE) and welcomes contributions. Read [CONTRIBUTING.md](CONTRIBUTING.md) first; commits need a `Signed-off-by` line ([Developer Certificate of Origin](https://developercertificate.org/)). Please follow the [Code of Conduct](CODE_OF_CONDUCT.md), and report security issues privately as described in [SECURITY.md](SECURITY.md).

## Links

- Website: [onushq.com](https://onushq.com)
- Plan: [PLAN.md](PLAN.md)
- Decisions: [docs/adr](docs/adr)
