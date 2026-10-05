# Contributing to Onus

Thanks for helping. Onus is early, so the most useful contributions right now are design feedback on [PLAN.md](PLAN.md), real-world pull requests we can test semantic reports against, and, once Phase 1 code lands, adapters, extractors and test scenarios.

## Ways to contribute

- **Discuss a design question** in [Discussions](https://github.com/onushq/onus/discussions). Larger decisions get a short record in [docs/adr](docs/adr).
- **Report a bug or request a feature** with the issue forms.
- **Propose a new language adapter** with the "New language adapter" form, so we can agree on scope before code is written.
- **Send a pull request.** For anything bigger than a small fix, open an issue or discussion first.

Good first tasks are labeled [`good first issue`](https://github.com/onushq/onus/labels/good%20first%20issue).

## Development

Onus is a Cargo workspace on stable Rust. Once the workspace lands (milestone M0):

```sh
cargo build
cargo test                     # unit tests and snapshot tests
cargo insta review             # review changed map and report snapshots
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo deny check               # licenses and advisories
```

Map and report output is tested with snapshots (`insta`) against the scenarios in `fixtures/shop`. When a change alters a snapshot, review it with `cargo insta review` and explain the change in your pull request.

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
