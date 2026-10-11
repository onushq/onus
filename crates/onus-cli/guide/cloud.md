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

## For agents

When the CLI is signed in (or `ONUS_API_KEY` is set), `onus mcp` adds tools for Onus Cloud:

- `onus_cloud_status`: who the CLI is, the organisation, what the plan allows.
- `onus_cloud_env_create`, `onus_cloud_env_run`, `onus_cloud_envs`, `onus_cloud_env_destroy`: hosted environments; the repository and commit default to the agent's worktree and its pushed HEAD.
- `onus_cloud_attach_evidence`: a recorded run to a pull request, for the judge.
- `onus_cloud_pulls`, `onus_cloud_pull`: governed pull requests, their lanes and evidence, read-only.

Signed out, the tools are not listed. They act as the person who signed the CLI in, so give agents a CLI signed in by someone whose role fits what the agents should do; nothing they do through Onus Cloud approves a change.
