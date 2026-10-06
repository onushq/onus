# Contributing to Onus

Thanks for helping. Onus is early, so the most useful contributions right now are design feedback on [PLAN.md](PLAN.md), real-world pull requests we can test semantic reports against, adapters, extractors and test scenarios.

## Ways to contribute

- **Discuss a design question** in [Discussions](https://github.com/onushq/onus/discussions). Larger decisions get a short record in [docs/adr](docs/adr).
- **Report a bug or request a feature** with the issue forms.
- **Propose a new language adapter** with the "New language adapter" form, so we can agree on scope before code is written.
- **Send a pull request.** For anything bigger than a small fix, open an issue or discussion first.

Good first tasks are labeled [`good first issue`](https://github.com/onushq/onus/labels/good%20first%20issue).

## Development

Onus is a Cargo workspace on stable Rust; `rust-toolchain.toml` pins the version, and `rustup` installs it on first use. The crates follow the pipeline: `onus-core` (types, ids, ranking, schemas, provider and plugin interfaces), `onus-lang-ts` (the TypeScript adapter), `onus-lang-scip` (SCIP import), `onus-lang-lsp` (the LSP bridge), `onus-map` (map building and plugins), `onus-diff` (the semantic diff), `onus-report` (renderers) and `onus-cli` (the `onus` binary). `onus-plugin-svelte` adds Svelte through the plugin protocol, `onus-plugin-example` is a reference plugin, and `onus-testkit` holds test doubles such as a fake language server. Framework packs live in `crates/onus-lang-ts/packs/`.

```sh
cargo build
cargo test                     # unit tests and snapshot tests
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo run -p onus-cli -- map fixtures/shop/base
```

Two checks need extra tools, installed once:

```sh
cargo install cargo-insta cargo-deny
cargo insta review             # review changed map and report snapshots
cargo deny check               # licenses and advisories
```

Map and report output is tested with snapshots (`insta`) against the golden scenarios in `fixtures/shop`: `base/` is a small monorepo and each `scenarios/<id>/` folder is an overlay of changed and added files, with a `deleted.txt` for removed files and an optional `intent.yaml`. When a change alters a snapshot, review it with `cargo insta review` and explain the change in your pull request. A snapshot is a claim about the code: a wrong fact in it is a bug, even if the test passes.

The JSON Schemas under `schemas/` are generated from `onus-core`; a test fails when they are stale. Regenerate them with:

```sh
cargo run -p onus-cli -- schema --out schemas
```

The user guide lives in `crates/onus-cli/guide/` and is compiled into the binary (`onus help <topic>`); `docs/guide.md` indexes it. When you change behavior a user can see, update the guide in the same pull request. Tests check that it still lists the built-in registry, the defaults and every kind of change the scenarios produce.

To measure performance on a generated workspace of about 200,000 lines:

```sh
cargo test --release -p onus-cli --test perf -- --ignored --nocapture
```

## Pull requests

- Keep each pull request focused on one change, and describe what it does and why.
- Add or update tests. A new change kind or extractor needs a fixture scenario that exercises it.
- Make sure `fmt`, `clippy`, tests and `cargo deny` pass. CI runs them on Linux, macOS and Windows.
- We squash-merge, so the pull request title becomes the commit message. Write it in the imperative ("Add Prisma write detection").

## Sign your commits (DCO)

Every commit must carry a `Signed-off-by` line certifying the [Developer Certificate of Origin](https://developercertificate.org/): that you wrote the change or have the right to submit it under the project's license.

```sh
git commit -s -m "Add Prisma write detection"
```

To sign off commits you already made: `git rebase --signoff main`. A check on every pull request verifies the sign-off. There is no separate contributor license agreement.

## License

By contributing, you agree that your contributions are licensed under the [Apache License 2.0](LICENSE).

## Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). Report unacceptable behavior to conduct@onushq.com.
