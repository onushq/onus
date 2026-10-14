# Scoped tokens, the gateway and escalation

An agent should work with exactly the access its task needs, never hold your forge credential, and ask with evidence when it needs more. Onus does that with task tokens, doors that enforce them, structured escalation and an audit log. Nothing here depends on which agent you use.

## Task tokens

Create a root key once and keep the private half with whoever approves tasks:

    onus token keygen --out ~/.onus-keys        # root.key (private, 0600) and root.pub

Describe a task in a plan:

    task: sms-alerts
    writes: ["services/notifications/**"]
    reads: ["services/orders/events/**"]       # optional
    hosts: ["sms.test.example"]                 # optional: network for its environment
    secrets: ["SMS_TEST_KEY"]                   # optional
    refs: ["refs/heads/onus/sms-alerts/*"]      # default: refs/heads/onus/<task>/*
    ttl: 8h                                     # default 8h
    agent: {tool: claude-code, model: opus}     # optional: who does the task

`onus scope suggest --plan plan.yaml` proposes reads from the map: the components the task writes, what they use and what uses them. It leaves out components labeled sensitive (they must be asked for), and it never widens writes.

    onus token mint --plan plan.yaml --key ~/.onus-keys/root.key --audit audit.jsonl

A token is a [Biscuit](https://www.biscuitsec.org/): signed rights (`write:path:<glob>`, `read:path:<glob>`, `push:ref:<glob>`, `net:host:<host>`, `secret:<NAME>`), the task id, the agent when the plan names one (`tool`, `model`, `config`, `team`) and an expiry. Submissions and log entries made under a token that names an agent use that agent, whatever the agent says it is. Writing implies reading. Anyone holding a token can narrow it for a sub-agent, but nobody can widen it without the root key:

    onus token attenuate --token "$TOKEN" --key-public root.pub --only write:path:services/notifications/src/sms/**
    onus token inspect --token "$TOKEN" --key-public root.pub
    onus token check --token "$TOKEN" --key-public root.pub write:path:services/billing/a.ts   # exit 1 and why

Path globs use `*` (within a folder), `**` (any depth) and `?`, and are case-sensitive like git paths.

### Components and contracts instead of paths

A plan can name what the task may touch in the map's terms:

    task: sms-alerts
    writeComponents: [notifications.internal]   # its files that declare no public symbol
    readContracts: [UserPreferences]            # the file that declares the contract
    escalateBefore: ["contract:*"]              # contract files need a granted escalation to change

    onus token mint --plan plan.yaml --key root.key --repo .

Minting resolves them through the map into path rights (`writeComponents: [notifications]` means all of the component's files), so a token's paths do not drift while the task runs. `escalateBefore` adds `guard:path:` rights for the files that declare matching contracts (onus.yaml `contracts:`): the gateway refuses a push that touches one unless the token also holds `unlock:path:` for it, which only an escalation grant adds. The refusal says how to ask.

## The git gateway

The gateway holds the real repository and its credentials; agents clone from it and push to it with their token as the password:

    onus gateway serve --repo /srv/gateway/shop --remote origin --base main \
      --key-public root.pub --state /srv/gateway/state --listen 127.0.0.1:9419

    git clone http://agent:$TOKEN@127.0.0.1:9419/sms-alerts.git

- **Reads.** Each task gets a mirror whose single commit holds only the paths it may read. The other files are not in its objects or its history, so no git command can show them.
- **Writes.** Each push is checked by the mirror's hook, which verifies the token again: every changed path of every new commit must be writable, refs must be in the push scope, and symbolic links, submodules, merges, new root commits and paths that collide by case with one outside the scope are refused. A refusal says how to ask:

      onus: push refused for task `sms-alerts`:
        - `services/billing/src/refund.ts` is outside your write scope
      Your write scope: `write:path:services/notifications/**`
      To change it, ask with evidence (a failing test is the strongest):
        onus escalate --task sms-alerts --scope write:path:services/billing/src/** --evidence failing-test:<test file> --reason "…"

- **Landing.** Accepted commits are replayed onto the real base commit, same files, authors and messages, and pushed to the remote with the credential only the gateway holds. Everything the agent could not see is still in the real commits.

The gateway needs `git http-backend`, which ships with git. It answers on the address you give it; keep it on a loopback or private network.

## Read scope for the map

`onus mcp --token "$TOKEN" --key-public root.pub` answers only about files the task may read: a question about an unreadable symbol or file is refused, and lists leave out unreadable items (the answer says how many). Every other tool is checked against the token too, and every call is logged (`onus help agents`).

## Escalation

When a task needs more, it asks with a structured request:

    onus escalate --task sms-alerts --scope write:path:services/orders/src/** \
      --evidence failing-test:services/orders/src/ship.test.ts \
      --reason "the shipped event must carry the phone number" --out request.json

Evidence is graded: 1 a failing test, 2 a trace, 3 a path through the map, 4 a draft diff, 5 a rationale. Onus adds the blast radius (files that depend on the asked paths) and the sensitivity labels of the components involved: those of the asked paths and of the files the evidence names. It also adds the owners (from onus.yaml or CODEOWNERS), who a person-decision goes to. They are the people who own every component the asked paths reach. A path reaches a component by the component's paths (onus.yaml `path`, or its folder), so a file that does not exist yet counts. Asking for one more path can only remove an owner, never add one, so an agent cannot choose who may grant its request. There are no owners, and an admin decides, when no one owns it all, when part of an asked path is in no component (onus.yaml, CI workflows or a root package.json, say), or when a right is not a path (a ref, a host or a secret). Evidence adds no owner. A request that asks for nothing, such as a broken-test report, grants nothing, so it goes to the owners of the files its evidence names.

    onus escalation decide request.json --token "$TOKEN" --key-public root.pub --key root.key \
      --reproduce-at HEAD --image node:22 --setup "npm ci" --test-command "npx vitest run {test}" \
      --audit audit.jsonl

A request is granted automatically only when all of these hold:

- it asks for path rights;
- the paths are outside sensitive components;
- the blast radius is small (20 files by default, `--max-blast-radius`);
- a failing test was reproduced by the test runner.

Then the command prints a new token: the original rights plus exactly what was asked, for the rest of the task. The token names the request it grants, so it proves that grant and no other. It is minted from the first block of `--token`, so grant from the token the task was given, not from a narrowed one. Otherwise it exits with code 3 and the reasons, and a person decides:

    onus escalation grant request.json --token "$TOKEN" --key-public root.pub --key root.key --by @team-orders
    onus escalation deny request.json --by @team-orders --reason "use the events API instead"

`--id <id>` and `--scope <right>` (repeated) make `grant` mint only when the file still holds that request, asking for exactly those scopes. Use them when the person decided somewhere else, such as Onus Cloud, and the file sat where the agent can write.

Before running a failing test, Onus checks that its reference is a plain path in the repository (letters, digits and `._/@+-`, no `..`, not absolute), that the file exists at the commit, and that it is a test file (onus.yaml `tests.globs`). The test command must name the test (`{test}`); one that does not would run every test, so nothing is run. A request's failing tests get 15 minutes in all, pulling the image and the setup included. A test still running then is stopped and its containers removed, and one not started by then is not run. Until then the command, `onus ui`'s Decide by policy and `onus_escalate` deciding by policy wait for it. A reference that fails a check, or a test that is stopped or not started, is not run to the end. It counts as not reproduced and sends the request to a person, even when another failing test reproduced, and the reasons say why. Whether a test reproduced is only ever what this run saw: a request file that says so is not believed, and one changed after it was filed is refused. The test only has to fail at the agent's commit, so an agent could write one that always fails; that is why the policy grants only path rights with a small blast radius outside sensitive components.

### Where a request goes

`onus_escalate` (and `onus_blocked`, a report that the task is blocked) always keeps the request in `.onus/escalations/<id>.json`. Then, in this order:

1. **Signed in to Onus Cloud** (a login, a scoped token with `escalation:write`, or an API key): it is filed there, and the local policy does not run. Onus Cloud assesses it on the repository's default branch with `onus escalation assess`, so an agent cannot weaken it by editing onus.yaml on its branch, and a person decides there. `onus_escalation` asks where it stands and writes the decision into the audit log. On a grant, `onus mcp` mints the token itself with the root key it holds (`--key`): the old rights plus exactly the scopes of the request it kept. A request file changed after it was filed, or a grant of other scopes than the file asks for, mints nothing. Onus Cloud never mints, and never sees the task token or the root key. See `onus help cloud`.
2. **Not signed in, the server holds the root key and a trusted checkout** (`onus mcp --token … --key root.key --trusted-repo ../shop-main`): the policy above decides here, and mints on a grant. The blast radius, sensitive labels and owners come from the trusted checkout's map, and the test command from its onus.yaml (`tests.command`), run at the agent's commit. The trusted checkout is one whose files the agent cannot edit, such as a `git worktree` of the default branch outside the agent's, so the agent's commits resolve there. A git worktree shares git's config, hooks and refs with the agent's, and the server runs git and the test command in it, so keep those out of the agent's reach as well. Without it, the facts would come from the agent's own worktree, which the agent can edit, so the policy does not run.
3. **Otherwise:** the request waits in the folder for a person in `onus ui`, or `onus escalation grant`.

### Assessing without minting

`onus escalation assess` applies the same policy and mints nothing, so it needs neither a token nor a key. Hosted services use it, and it records nothing in the audit log, because assessing is not deciding:

    onus escalation assess request.json --repo <trusted checkout> --reproduce-at <commit>

The test command comes from the trusted checkout's onus.yaml (`tests.command`), and the image and setup from its `environment:` (or devcontainer), else `node:22`. An environment that is declared but cannot be used (a devcontainer that builds from a Dockerfile, say) is not replaced by `node:22`: the test is not run, and the reasons say why. The agent's commit is only the tree the test runs in, so the agent cannot choose what reproduces a failure. `--test-command`, `--image` and `--setup` override them. Without a test command, failing tests are not reproduced, and the reasons say so. It prints the request with its reproduced evidence (which keeps its id: what a run saw is not part of it), `decision` (`granted` or `needs-person`) and `reasons`, and exits 0 when policy would grant it, 3 when a person decides.

### The test runner

`onus run-test --repo . --reference HEAD --image node:22 -- npx vitest run path/to.test.ts` runs a command at a commit in a throwaway container: no network, no capabilities, 4 GB of memory, 2 CPUs. The commit's files are copied in, never mounted, so engines that run in a VM (colima, podman machine) work too. Onus runs repository code only here and in environments (`onus help environments`), and only when asked. An optional `--setup` command (installing dependencies) runs first in its own container, with network. It needs Docker or Podman.

## The audit log

Every decision is one JSON line: mints, mirrors, pushes accepted and refused, replays, escalations, grants and denials, and every call through a door. Each line carries the hash of the line before it, so a removed or edited line breaks the chain:

    onus audit /srv/gateway/state/audit.jsonl     # "N entries, chain intact", or where it breaks

What the doors log:

| Action | Door | Decisions |
|---|---|---|
| `mcp:<tool>` | `onus mcp`, every tool call | `allowed` or `denied`; then `done` or `failed` for tools that act |
| `adopt-token` | `onus mcp`, when it takes up a granted escalation's token (only one that keeps every right and narrowing of the token it holds) | `allowed` |
| `auth` | the gateway, a refused password (bad token, another task's, expired) | `refused` |
| `fetch` | the gateway, every clone and fetch | `allowed` |
| `push`, `mirror`, `replay` | the gateway | as before |

An `onus mcp` entry looks like this:

    {
      "seq": 42, "at": 1791500000,
      "actor": "sms-alerts",                        // the token's task, or "local" without one
      "action": "mcp:onus_env_run",
      "subject": "sms-1",                           // an environment, run, submission, target, path or pull request
      "decision": "allowed",
      "reason": "the token of task `sms-alerts` allows it",
      "details": {
        "door": "mcp",
        "argsDigest": "sha256:…",                   // of the arguments, keys sorted
        "args": { "name": "sms-1", "command": "npm test" },   // long strings and lists shortened
        "token": { "task": "sms-alerts", "id": "3f9a…", "expires": 1791528800, "attenuations": 0 },
        "agent": { "tool": "claude-code", "model": "opus", "config": "", "team": "", "from": "token" },
        "missing": ["net:host:evil.example"],       // denials
        "hidden": 3,                                // questions whose answer left items out
        "result": { "runId": "…", "exitCode": 0 }   // done
      },
      "prev": "…", "hash": "…"
    }

The token itself is never logged, only its id (the first 16 hex characters of its sha256). `details.result` keeps what later checks need: the hosted environments and runs a task made, which `onus_cloud_env_run`, `onus_cloud_env_destroy` and `onus_cloud_attach_evidence` are checked against. A broken chain refuses those calls until a person checks the log.

The chain is plain sha256: someone who can write the file can rewrite it consistently. Keep the log where agents cannot write (`onus mcp --audit`, the gateway's state folder).

The log can travel with the work. `onus_cloud_attach_evidence` with `doorLog: true`, or `onus cloud evidence <pull> --door-log --task <task>`, attaches the task's slice to the pull request in Onus Cloud: the entries from the task's first to its last, with whatever lies between, so the chain can be checked over it where it is read. `onus ci pr --audit` and `onus provenance --audit` check it and keep the task's entries in the change's provenance (`onus help provenance`). Through `onus mcp --token`, the door sends its inspection of the token it holds with the slice (`{task, rights, attenuations, expires}`), never the token. A slice shows only that nothing in it was changed or removed, not what came before it, nor who cut it. Other tasks' entries between the task's first and last go with it, so it is attached only to the pull request whose head in Onus Cloud is the worktree's commit.
