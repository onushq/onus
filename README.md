# Onus

**Code is not the product.** Agents now write code faster than anyone can read it. Onus is the layer every coding agent works through: it reads each change for what it means, routes it by risk, and approves it on evidence instead of on someone pretending to review 2,000 lines.

The idea behind Onus in one sentence: changes should be understood as shifts in meaning, scored by risk, and approved by evidence rather than by someone reading lines.

> **Status: pre-alpha.** The first version of Phase 1 works: `onus` maps a TypeScript or JavaScript monorepo and reports a change as a few changes in meaning. Release binaries and the GitHub Action come next. The design lives in [PLAN.md](PLAN.md). Watch the repository or join [Discussions](https://github.com/onushq/onus/discussions) to follow along.

## What it does

Given a pull request, Onus describes it as a few changes in meaning instead of thousands of changed lines. For the manifesto's example, "Text customers when their order ships" (1,408 lines across 23 files in [`fixtures/shop`](fixtures/shop)), the report reads:

> **Onus · 4 meaning-level changes · 1 needs attention · 1,408 lines in 23 files**
>
> |   | Change | Kind | Why it matters |
> |---|---|---|---|
> | ● | `notifications` now calls an external SMS provider (Acme SMS) | New external dependency | Customer phone numbers leave the system via Acme SMS, a new vendor; needs a person |
> | ○ | `UserPreferences` gains an optional `phoneVerified` field | Contract change, additive | Shared contract used in 4 files across 2 components; existing callers unaffected; declared invariant: phone numbers are stored in E.164 |
> | ○ | `notifications` subscribes to the `OrderShipped` event | New event consumer | Additive; `orders` is unchanged |
> | ○ | 1,402 lines of new code and tests inside `notifications` | Internal | Stays inside `notifications`, which `billing` depends on; adds 69 test cases; reads new config `ACME_SMS_API_KEY` and `ACME_SMS_SENDER_ID`; … |

The first row is the one that deserves a person's attention, so it comes first. Every row is computed from the code by deterministic analysis and fixed templates, never generated, and each one links to the files and lines behind it. Onus only reads files: it never installs or runs the code it analyzes.

## Quick start

Onus is a Rust workspace. With [Rust](https://rustup.rs) installed:

```sh
git clone https://github.com/onushq/onus && cd onus
cargo build --release                     # the binary is target/release/onus

# The map of a repository: components, contracts, relationships, tests
./target/release/onus map fixtures/shop/base
./target/release/onus map fixtures/shop/base --json

# Report a change between two git refs of your repository
./target/release/onus report --repo path/to/repo --base main --head HEAD

# Or between two directories; try the manifesto's SMS example
cp -R fixtures/shop/base /tmp/shop-head
cp -R fixtures/shop/scenarios/s1-sms-alerts/services /tmp/shop-head/
./target/release/onus diff fixtures/shop/base /tmp/shop-head
```

Useful flags: `--format json` for the full machine-readable report, `--intent <file>` to check the change against a stated intent (a YAML file or a pull request body with an `onus-intent` block), and `--fail-on rule-violation,secrets` to exit with code 2 when a boundary rule is broken or a secret is committed. `onus init` writes a starter [`onus.yaml`](fixtures/shop/base/onus.yaml) inferred from your workspaces and `CODEOWNERS`, and `onus schema --out schemas` writes the JSON Schemas of every format.

The [user guide](docs/guide.md) covers every command, how to read a report, every kind of change, the `onus.yaml` reference, the intent check and running Onus in CI. It also ships in the binary: run `onus help` for the list of topics and `onus help <topic>` to read one.

## Roadmap

Each phase is useful on its own and moves on only when a measurable gate is passed.

1. **Semantic change reports** on today's pull requests, from a CLI and a GitHub Action. *(in progress: the CLI works; the Action is next)*
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
- User guide: [docs/guide.md](docs/guide.md)
- Decisions: [docs/adr](docs/adr)
