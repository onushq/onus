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

`onus scope suggest --plan plan.yaml` proposes reads from the map: the components the task writes, what they use and what uses them. It leaves out components labeled sensitive (they must be asked for), and it never widens writes.

    onus token mint --plan plan.yaml --key ~/.onus-keys/root.key --audit audit.jsonl

A token is a [Biscuit](https://www.biscuitsec.org/): signed rights (`write:path:<glob>`, `read:path:<glob>`, `push:ref:<glob>`, `net:host:<host>`, `secret:<NAME>`), the task id and an expiry. Writing implies reading. Anyone holding a token can narrow it for a sub-agent, but nobody can widen it without the root key:

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

`onus mcp --token "$TOKEN" --key-public root.pub` answers only about files the task may read: a question about an unreadable symbol or file is refused, and lists leave out unreadable items (the answer says how many).

## Escalation

When a task needs more, it asks with a structured request:

    onus escalate --task sms-alerts --scope write:path:services/orders/src/** \
      --evidence failing-test:services/orders/src/ship.test.ts \
      --reason "the shipped event must carry the phone number" --out request.json

Evidence is graded: 1 a failing test, 2 a trace, 3 a path through the map, 4 a draft diff, 5 a rationale. Onus adds the blast radius (files that depend on the asked paths) and the sensitivity labels of the components involved.

    onus escalation decide request.json --token "$TOKEN" --key-public root.pub --key root.key \
      --reproduce-at HEAD --image node:22 --setup "npm ci" --test-command "npx vitest run {test}" \
      --audit audit.jsonl

A request is granted automatically only when all of these hold:

- it asks for path rights;
- the paths are outside sensitive components;
- the blast radius is small (20 files by default, `--max-blast-radius`);
- a failing test was reproduced by the test runner.

Then the command prints a new token: the original rights plus exactly what was asked, for the rest of the task. Otherwise it exits with code 3 and the reasons, and a person decides:

    onus escalation grant request.json --token "$TOKEN" --key-public root.pub --key root.key --by @team-orders
    onus escalation deny request.json --by @team-orders --reason "use the events API instead"

### The test runner

`onus run-test --repo . --reference HEAD --image node:22 -- npx vitest run path/to.test.ts` runs a command at a commit in a throwaway container: no network, no capabilities, 4 GB of memory, 2 CPUs. The commit's files are copied in, never mounted, so engines that run in a VM (colima, podman machine) work too. Onus runs repository code only here and in environments (`onus help environments`), and only when asked. An optional `--setup` command (installing dependencies) runs first in its own container, with network. It needs Docker or Podman.

## The audit log

Every decision is one JSON line: mints, mirrors, pushes accepted and refused, replays, escalations, grants and denials. Each line carries the hash of the line before it, so a removed or edited line breaks the chain:

    onus audit /srv/gateway/state/audit.jsonl     # "N entries, chain intact", or where it breaks
