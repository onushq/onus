# Getting started

Onus turns a change to a codebase into a short report of changes in meaning: new external services, contract changes, new relationships between components, weakened tests, broken rules. It reads TypeScript and JavaScript today.

Onus only reads files. It never installs dependencies or runs the code it analyzes, so it is safe to run on pull requests from forks.

## Install

With Homebrew, on macOS or Linux:

    brew install onushq/tap/onus

Or, without Homebrew, the installer downloads the binary for your system from the latest GitHub release, checks its SHA-256 and puts it in ~/.local/bin:

    curl -fsSL https://onushq.com/install.sh | sh

Set ONUS_VERSION=v0.1.0 to pin a version and ONUS_INSTALL_DIR to install elsewhere. Archives for every platform, including Windows, are at https://github.com/onushq/onus/releases.

Or build from source with Rust (https://rustup.rs):

    git clone https://github.com/onushq/onus && cd onus
    cargo build --release          # the binary is target/release/onus

Or install the binary into ~/.cargo/bin:

    cargo install --git https://github.com/onushq/onus onus-cli --locked

Check it works:

    onus --version

## Your first map

The map is Onus's picture of a repository: components, the contracts they expose, how they depend on each other, events, data access, external services and tests.

    onus map path/to/repo           # a summary
    onus map path/to/repo --json    # the whole map

The summary lists each component with its kind, public symbols, labels and owners, the external services found, and anything Onus could not resolve.

## Your first report

Compare two git refs of a repository:

    onus report --repo path/to/repo --base main --head my-branch

Or two directories:

    onus diff old-checkout new-checkout

Try it on the fixture that ships with Onus, the manifesto's "text customers when their order ships" change:

    cp -R fixtures/shop/base /tmp/shop-head
    cp -R fixtures/shop/scenarios/s1-sms-alerts/services /tmp/shop-head/
    onus diff fixtures/shop/base /tmp/shop-head

You get four rows instead of 1,408 changed lines. `onus help reports` explains how to read them.

## Tell Onus about your repository

Onus works without configuration: it finds components from npm, pnpm or yarn workspaces (or folders under services/, packages/ and apps/) and owners from CODEOWNERS. A small `onus.yaml` makes reports sharper: sensitivity labels, boundary rules, event patterns and SDKs Onus does not know.

    onus init                       # writes a starter onus.yaml

Review it, uncomment the labels it suggests, and commit it. See `onus help configuration`.

## Next

- `onus help reports`: reading a report
- `onus help changes`: every kind of change Onus reports
- `onus help intent`: checking a change against what it says it does
- `onus help ci`: running Onus on every pull request
