# Onus Cloud from the CLI

Onus Cloud is the hosted Onus: it analyses every pull request of the repositories you install it on, keeps the outcome records, runs the judge, and builds environments on its own machines. The CLI signs in to it as you, so you and your agents can use it from the terminal and through `onus mcp`.

## Signing in

    onus login                       # opens the browser; approve the code it shows
    onus login --org acme            # and act in the organisation acme
    onus login --no-browser          # print the link instead, for a machine without one

`onus login` shows a one-time code and opens Onus Cloud's sign-in page, where you compare the code and approve it (and pick the organisation the CLI acts in). The CLI then holds a token of its own in `~/.config/onus/cloud.json` (`$ONUS_CONFIG_DIR` or `$XDG_CONFIG_HOME` to move it), readable by you only. The token acts as you, in every organisation you belong to, with your role there; organisations that require single sign-on need you to have approved it after signing in with it. It lasts 90 days from its last use.

    onus cloud status                # who, which organisation, its plan and what it allows
    onus cloud use beta              # act in another organisation
    onus logout                      # the token is revoked and forgotten

The account page in Onus Cloud lists every signed-in CLI, and revokes any of them.

**In CI**, there is no one to approve a code: set `ONUS_API_KEY` to an organisation's API key (Settings, API keys; `write` scope to create environments and attach evidence). `ONUS_CLOUD_URL` and `ONUS_CLOUD_ORG` change the address and the organisation; `--org` does too, per command.

The CLI signs in with the first of: a scoped token in `ONUS_CLOUD_TOKEN`, an API key in `ONUS_API_KEY`, the login in `cloud.json`. With `ONUS_CLOUD_TOKEN` set, neither of the others is read.

## Narrow tokens for agents and scripts

A login acts as you, everywhere you can. An agent or a script needs much less. Mint it a scoped token: one organisation, some repositories, some scopes, a fixed end.

    onus cloud token create --repo acme/shop --scope pulls:read --scope env:run --ttl 2h --label sms-otp
    onus_tok_7Hc0…                      # stdout: the token alone
    onus: token for acme (acme/shop): pulls:read, env:run; ends 2026-10-12T19:40:00.000Z (2h). Revoke: onus cloud token revoke 3f2a…

    ONUS_CLOUD_TOKEN=onus_tok_7Hc0… claude …          # the agent's onus commands use it, and nothing else

| Scope | Opens |
|---|---|
| `env:read` | hosted environments, their runs and logs |
| `env:run` | create, run in and destroy hosted environments (and `env:read`) |
| `evidence:write` | attach evidence to a pull request |
| `pulls:read` | governed pull requests, their analyses and evidence |
| `map:read` | a repository and the maps Onus Cloud keeps of it |
| `escalation:write` | file, read and withdraw escalations |

- **Repositories.** `--repo` is repeatable; without it the token is for this folder's GitHub repository. `--all-repos` makes it for every repository.
- **How long.** `--ttl` is `90s`, `30m`, `2h` or `1d`: one hour by default, at most 24 hours. The end is fixed; using the token does not move it.
- **Narrowable, never widenable.** A scoped token can mint a narrower one, for a sub-agent: `ONUS_CLOUD_TOKEN=<parent> onus cloud token create --scope pulls:read --ttl 30m`. A child never has a scope, a repository or an end its parent does not. API keys cannot mint tokens.
- **Revoking.** `onus cloud token list` shows your logins and scoped tokens; `onus cloud token revoke <id>` revokes one and every token minted from it. `onus logout` revokes the login and everything minted from it.
- **Denials teach.** A call the token does not open answers with the scope it needs: `Denied: this token does not have env:read … (mint one with onus cloud token create --scope env:read, or ask with onus_escalate)`. A token that ended says to ask for a new one.

`onus cloud status` says how the CLI is signed in: `with a scoped token (sms-otp) in acme: pulls:read, env:run on acme/shop; ends in 1h52m`. A scoped token acts in its one organisation, so `onus cloud use` refuses it. Onus Cloud logs every call a token makes (Settings, Audit log, CLI and agent calls).

## Hosted environments

The same environments as `onus env` (`onus help environments`), built and run by Onus Cloud instead of your machine: no Docker needed, and every run is recorded by Onus Cloud rather than by the machine that asks.

    onus cloud env create                                   # this folder's repository at its pushed HEAD
    onus cloud env create --repo acme/shop --ref pull/42 --name sms
    onus cloud env run sms -- npm test
    onus cloud env run sms --timeout 3600 -- npx playwright test
    onus cloud env show sms                                 # its runs
    onus cloud env log sms <run id>                         # a run's whole output
    onus cloud env list --all
    onus cloud env destroy sms

- **What is built.** The repository's `environment:` in onus.yaml (or its devcontainer), read from the **default branch**, never from the commit under test; the commit itself must be pushed, since Onus Cloud fetches it from GitHub. `--ref` takes a branch, tag, commit or `pull/<n>`. `create` waits until it is ready (the first build of a lockfile runs the setup; later ones start from the warm image).
- **Network.** None, unless you name hosts: `--host registry.npmjs.org --host '*.example.com'`. The egress proxy allows exactly those. Setup has network, as with `onus env`. Nothing in an environment can reach Onus Cloud's own network or its cloud's metadata server.
- **Files.** Environments run under gVisor, where `/tmp` is in memory: a `seed` or `setup` that writes there is not kept when the judge rebuilds the environment to re-run a test. Write seed data under the repository or `/var`.
- **Secrets.** None. Hosted environments are for code and synthetic data.
- **Runs.** Each run is recorded like `onus env run`: the output, JUnit results and traces the environment declares, under a manifest. `run` waits and exits 0 when the command passed, 1 when it failed. Runs of one environment go one at a time.
- **Lifetime.** An environment unused for 30 minutes, or 8 hours old, is destroyed. Destroy it when you are done: its minutes count until then.
- **Plans.** The Free plan has 1 environment at a time and 300 environment minutes a month; Team has 5 at a time and 3,000 minutes, then bills the minutes beyond; Enterprise has no limits. `onus cloud status` shows what is left.

## Evidence the judge can trust

Attach a recorded run to a pull request:

    onus cloud evidence acme/shop#42 --run <run id>

Onus Cloud keeps it for the run's commit and, when that is the pull request's head, analyses the pull request again. The judge finds the run's manifest and artifacts beside the repository, checks the run against them (the commit, the command, the exit code), and re-runs it in a fresh container with the environment's setup and seed. Its verdict says how many runs matched their manifests.

    onus cloud pulls --attention                            # waiting for a person
    onus cloud pulls --repo acme/shop --lane judge
    onus cloud pull acme/shop#42                            # its lane, analyses and evidence

## The map Onus Cloud keeps

Onus Cloud keeps the map of every default-branch commit it maps (after each push to the default branch) and of the newest head of each pull request it analyses. A newer head of a pull request replaces the older one; a closed pull request keeps its last. Every map is the binary's own output, kept byte for byte, and so is its overview (`onus query overview`).

    GET /v1/orgs/<org>/repos/<owner/name>/map                        # the newest default-branch map
    GET /v1/orgs/<org>/repos/<owner/name>/map?commit=<40-hex sha>    # the map of that commit
    GET /v1/orgs/<org>/repos/<owner/name>/map/overview[?commit=…]    # its overview
    GET /v1/orgs/<org>/repos/<owner/name>/maps                       # the maps kept, newest first

The `x-onus-map-commit` header names the commit of the map sent. A commit Onus Cloud never mapped answers 404 `no_map`: ask a worktree at that commit with `onus query` instead. The web app's map explorer shows the overview: components, owners, labels, what each uses and what uses it.

`onus cloud map` asks it the same questions as `onus query`, with the same JSON. The CLI downloads the map and answers here; Onus Cloud only serves the file:

    onus cloud map status --repo acme/shop                          # the default branch's newest map
    onus cloud map dependents UserPreferences --commit 3f2a1c…      # a commit Onus Cloud mapped
    onus cloud map impact formatPhone --change change-signature --pull 42   # a pull request's head
    onus cloud map --download map.json --commit 3f2a1c…             # the map itself
    onus query --map map.json dependents UserPreferences --depth 2

The repository defaults to this folder's GitHub remote (else `ONUS_CLOUD_REPO`). `--commit` takes a full sha, or anything this folder's git resolves. Maps of a commit never change, so they are kept by sha in `~/.cache/onus/cloud-maps/` (`$ONUS_CACHE_DIR` or `$XDG_CACHE_HOME` to move it), and a second question makes no request. A map of another schema version is refused (update onus), and so is a map of another commit than the one asked for. `check` and `stats` need a worktree: use `onus query`.

## Escalations and the safe exit

An agent that needs more than its task token allows asks with evidence (`onus help scopes`). An agent that is blocked (a test it did not write fails, the spec contradicts itself, the task cannot be done as asked) says so with evidence and stops. When the CLI is signed in, both go to Onus Cloud, where a person sees them, decides, and is notified:

    onus cloud escalate --task sms-otp --scope 'write:path:services/billing/src/**' \
      --evidence failing-test:services/billing/src/a.test.ts --reason "…" \
      --token "$TOKEN" --key-public root.pub --key root.key [--pull 42] [--wait 600]
    onus cloud blocked --kind broken-test --evidence failing-test:services/orders/src/eta.test.ts \
      --reason "the test expects UTC; the fixture is local time" --task sms-otp
    onus cloud escalation <id> [--wait 600]       # where it stands
    onus cloud escalation list [--waiting]
    onus cloud escalation withdraw <id>

- **The facts are the binary's.** The request is built here, with its blast radius, sensitive labels and owners from the map, and kept in `.onus/escalations/<id>.json`. Onus Cloud then works them out again on the repository's **default branch** (`onus escalate`, `onus escalation assess`), so a branch that edits onus.yaml cannot weaken its own request. Low-risk requests with a reproduced failing test are granted by policy; the rest wait for an owner of the code involved, or an admin.
- **The token stays here.** Onus Cloud is sent what the task token holds (task, rights, end), never the token. On a grant, the door that holds the root key (`--key`, or `onus mcp --key`) mints the new token itself: the old rights plus exactly the scopes of the request it kept, never anything the answer names. An answer about another request is refused. Without the key, the answer says who granted it and the command the operator runs (`onus escalation grant .onus/escalations/<id>.json --by "Ada (Onus Cloud)" …`); the token is picked up once it is kept beside the request.
- **The audit log.** Filing writes `sent-to-onus-cloud`; a person's grant, denial or answer is written as theirs (`Ada (Onus Cloud)`), and `onus ui` shows which requests went to Onus Cloud.
- **Exit codes.** 0 when granted (the token alone on stdout) or answered, 1 when denied, withdrawn, expired or failed, 3 while it is being checked or waits for a person.

A blocked report asks for no rights. The person answers it (the agent sees `answered`, with the text) or denies it (the task should stop).

## For agents

When the CLI is signed in (with `onus login`, `ONUS_CLOUD_TOKEN` or `ONUS_API_KEY`), `onus mcp` adds tools for Onus Cloud:

- `onus_cloud_status`: who the CLI is, the organisation, the token's scopes, what the plan allows.
- `onus_cloud_env_create`, `onus_cloud_env_run`, `onus_cloud_envs`, `onus_cloud_env_destroy`: hosted environments; the repository and commit default to the agent's worktree and its pushed HEAD.
- `onus_cloud_attach_evidence`: a recorded run to a pull request, for the judge.
- `onus_cloud_pulls`, `onus_cloud_pull`: governed pull requests, their lanes and evidence, read-only.
- `onus_cloud_map`: a question to the map Onus Cloud keeps, at a commit or a pull request's head, answered by the CLI; under a task token, filtered to what the task may read, as the map tools are.
- `onus_cloud_token`: a narrower token for a sub-agent, never wider than the server's.

`onus_escalate`, `onus_escalation` and `onus_blocked` go to Onus Cloud by themselves when the CLI is signed in (see above).

**The login never reaches the agent's calls.** Signed in with `onus login`, `onus mcp` mints a scoped token from the login on the first Onus Cloud call and uses it for every call after: the organisation the CLI acts in, the worktree's repository, all six scopes, 8 hours (`--cloud-ttl`, at most 24h), labelled with the task of `--token`. It mints a new one shortly before that one ends. The worktree's repository is its GitHub remote; name it with `--cloud-repo owner/name` (or `ONUS_CLOUD_REPO`) when there is none. With `--cloud-token` (or `ONUS_CLOUD_TOKEN`) it uses that token as it is, and with `ONUS_API_KEY` the key.

Signed out, the tools are not listed. Nothing an agent does through Onus Cloud approves a change.
