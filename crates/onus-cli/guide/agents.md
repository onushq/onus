# The map for coding agents (MCP)

`onus mcp` gives a coding agent the codebase map as MCP tools: where things are, what depends on what, which tests cover what, who owns it, and what a change means before it is committed. Every answer is computed from the code, never generated, and carries the file and line it comes from.

It also lets the agent hand its work in the way Onus expects: run the tests in an environment, submit the change with that evidence, see its lane and the judge's verdict, and ask for more access with evidence instead of working around its token. Nothing it offers lets an agent approve itself.

**What helps agents today is `onus_check`.** We tested whether the map makes coding agents faster or more correct when they implement a feature ([evaluation](https://github.com/onushq/onus/blob/main/docs/evaluation/2026-10-07-map-for-agents.md)). It did not, measurably: agents found code with text search just as well and rarely called the navigation tools. They did call `onus_check` before finishing, and Phase 2 now focuses on making it precise. The navigation tools below are experimental.

## Connect an agent

Claude Code:

    claude mcp add onus -- onus mcp

Any MCP client that starts servers over stdio: run `onus mcp` in the repository (or pass `--repo <worktree>`). The map is the map of the worktree the server is started in, as it is on disk now, saved but not committed changes included.

Clients that connect over HTTP (streamable HTTP): run `onus mcp --http 127.0.0.1:8765` and point the client at `http://127.0.0.1:8765/`. Only requests naming a loopback host are answered, which keeps other machines and DNS rebinding out; `--allow-host <name>` adds a host for a network you trust. The tools describe your code and can run containers, and there is no authentication. Every tool answers the same over stdio and HTTP, and to every client.

## Tools

The check:

- `onus_check`: the changes in meaning between a commit (default `HEAD`) and the worktree now, as the same rows as a pull request report, preceded by a short checklist of what to verify before finishing: third-party APIs the repository has never used (with the pinned version), implementations and test doubles of a changed interface that were not updated, committed secrets and broken boundary rules. Run it before finishing a task.
- `onus_impact`: before a change, what breaks if a symbol, file or component is removed, renamed, changes its signature, gains a required member, or only changes behavior. One site per file (file, line, how it is used; for a new required member only the implementations, found in the source text), the tests that exercise it, and what static analysis cannot see.
- `onus_invariants`: the invariants onus.yaml declares for a symbol, file or component, or all of them: rules a change must keep.

Navigation (experimental):

- `onus_status`: what the map holds (components, files, symbols, edges, tests). Start here.
- `onus_find`: symbols by name words, ranked by how many words match in the name, then in fields and parameters, then in the file path (`format phone` finds `formatPhoneNumber`; a field name finds the types that declare it). Every answer also lists matching files.
- `onus_symbol`: one symbol: kind, location, contract shape, declared invariants, how many places use it.
- `onus_dependents`: what depends on a symbol, file or component, up to `depth` steps (imports, calls, type references). Use it before changing something.
- `onus_dependencies`: what a symbol, file or component depends on: code, external services, events, tables, config keys.
- `onus_tests_for`: the test files that exercise a symbol, file or component.
- `onus_owners`: owners and sensitivity labels.
- `onus_component`: a component's public surface, the components it uses and that use it, its external services and events.
- `onus_file`: a file's symbols, imports and importers.

Handing work in (leave these out with `onus mcp --no-actions`):

- `onus_lanes`: the lane policy: what merges on its own, what goes to the judge or a person, and the floors no policy can lower.
- `onus_env_create`, `onus_env_run`, `onus_envs`, `onus_env_destroy`: an environment built from a commit, and commands run in it. Each run is recorded as evidence (output, JUnit results, traces) under a run id. See `onus help environments`.
- `onus_evidence`: recorded runs, a run's manifest, or one of its artifacts.
- `onus_run_test`: a test command once in a throwaway container, to reproduce a failure; nothing is recorded.
- `onus_submit`: hands in a committed change with an intent, run ids as evidence and the ids of escalations granted during the task. Onus reports it, attaches the scope of the server's task token, classifies it, and keeps it in `.onus/submissions/` where people see it in `onus ui`. Returns the lane, every reason, and a submission id.
- `onus_judge`: the verifying judge on a submission: evidence re-run in fresh containers, intent, contracts, tests, held-out checks. The verdict is kept beside the submission.
- `onus_escalate`, `onus_escalation`: ask for rights the token lacks, with evidence, and check the answer (`wait` waits up to 50 seconds for it). A granted request returns the new token, the old rights plus exactly what was asked, and the server uses it from then on. The granted token names its request, so it proves the grant wherever the person's decision was logged. Where a request goes is in `onus help scopes`.
- `onus_blocked`: the safe exit. A test the agent did not write fails and cannot be fixed in scope, the spec contradicts itself or the code, or the task cannot be done as asked: the agent reports it with evidence (`kind` `broken-test`, `contradictory-spec` or `impossible-task`) and stops. It is filed like an escalation that asks for no rights, routed to the owners of the code involved, and listed even when the CLI is signed out. A person answers it; the agent reads the answer with `onus_escalation` (state `answered`).
- `onus_outcomes`: track records and the held-out backlog, read-only.

Onus Cloud (listed only when the CLI is signed in with `onus login`, `ONUS_CLOUD_TOKEN` or `ONUS_API_KEY`; see `onus help cloud`):

- `onus_cloud_env_create`, `onus_cloud_env_run`, `onus_cloud_envs`, `onus_cloud_env_destroy`: environments built and run on Onus Cloud's machines from a pushed commit, each run recorded by Onus Cloud.
- `onus_cloud_attach_evidence`: a recorded run to a pull request, for the judge.
- `onus_cloud_status`, `onus_cloud_pulls`, `onus_cloud_pull`: who the CLI is and what its plan allows; governed pull requests, their lanes and evidence.
- `onus_cloud_map`: the map tools' questions (`{"query": "dependents", "target": …}`) asked of the map Onus Cloud keeps at a commit or a pull request's head, answered here and filtered to the token's read scope.
- `onus_cloud_token`: a narrower Onus Cloud token for a sub-agent: only the scopes and repositories named, never more than the server's, ending within 24 hours.

A server signed in with a login never sends the login with an agent's call: it mints a scoped token for the worktree's repository on first use and uses that. The flags:

- `--key <file>`: the root private key, held by the server and never shown to the agent. With it, grants decided by policy, or by a person in Onus Cloud, are minted by the server. Needs `--token`.
- `--cloud-token <token>` (`ONUS_CLOUD_TOKEN`): a scoped Onus Cloud token to use as it is.
- `--cloud-repo <owner/name>` (`ONUS_CLOUD_REPO`): the repository in Onus Cloud, when the worktree's remote is not GitHub.
- `--cloud-ttl <duration>`: how long the token minted from a login lasts (default `8h`, at most `24h`).

The order an agent should follow is in the server's instructions: commit, run the tests with `onus_env_create` and `onus_env_run`, `onus_submit` with an intent and those runs, then `onus_judge`.

What agents cannot do through MCP: record outcomes, grant or deny escalations, mint or widen task tokens, widen an Onus Cloud token, or choose the scope a submission is checked against. With `--token`, submissions, environments (their hosts and secrets) and escalations use the server's task token, whatever the agent asks for.

Every call is checked at the door and logged. With `--token`, each tool first checks the token: its signature when the server starts, its expiry on every call, then the tool's own rule. A refused call says which right is missing and how to ask for it:

    denied: `net:host:evil.example` is not in the token of task `sms-alerts`.
    Your net rights: `net:host:sms.test.example`.
    To get it: onus_escalate with scopes ["net:host:evil.example"] and evidence (a failing test is strongest).

- Environments belong to the task that made them. Another task cannot run commands in one, destroy it, read its runs or see it in `onus_envs`; a run used as evidence must come from the task's own environments. An environment whose hosts or secrets the token no longer allows must be made again.
- `onus_run_test` takes no `setup` under a token: setup runs with open network, and the token decides the network. Use `onus_env_create`.
- `onus_submit` takes `escalations`: ids from `onus_escalate`. Each must be granted, for the task, with a token signed by the root key, minted for that request, that holds exactly what was asked; otherwise the submission is refused with the reason. `onus submit --escalation` and `onus ui` check the same.
- A grant is minted from the first block of a token, so it drops any narrowing (`onus token attenuate`). A server whose token was narrowed for a sub-agent does not take up such a grant, and does not hand it out; the person narrows the granted token the same way and restarts the server with it.
- A task plan can name the agent (`agent: {tool: claude-code, model: opus}`). The token then carries it, and submissions and log entries made under the token use it; what the agent declares (`agent_tool`, …) is ignored, and the answer says so.
- Hosted environments and runs (`onus_cloud_*`) are checked the same way, from the log of what the task made. A pull request or a list of them is this worktree's repository: `onus_cloud_pulls` without `repository` lists only its pull requests. An environment made without `hosts` has no network.
- A tool with no rule is closed to a token.

Every call, with or without a token, is written to the audit log: the tool, a digest and a shortened copy of its arguments, the token's task and id (never the token), the agent and where its identity came from, the decision (`allowed` or `denied`, then `done` or `failed` for tools that act) and the reason. The log is `.onus/audit.jsonl` in the worktree, which `onus ui` shows; the server adds it to the repository's own `.git/info/exclude`, so `git add -A` does not commit it. `onus mcp --audit <file>` puts it where the agent cannot write. When the log cannot be written, the call is refused. Without a token nothing is refused, and calls are logged as `local`. See `onus help scopes`.

Targets can be a symbol id (`billing:src/payments.ts#chargeCard`), a bare symbol name, a file path, a module id or a component id. Answers are JSON, sorted, and limited (default 50 items, `limit` up to 500); `truncated` says when there is more. Each answer also says which version of the map it came from and whether the map was rebuilt for it.

The same questions work from a shell, with the same answers:

    onus query status
    onus query find format phone
    onus query dependents UserPreferences --depth 2
    onus query tests-for services/billing/src/payments.ts
    onus query impact UserPreferences --change add-required-member
    onus query invariants
    onus query overview
    onus query check --base main

## Many agents, many worktrees

Agents often work in parallel, each in its own git worktree of one repository. Onus is built for that:

- **One server per repository.** Every `onus mcp` and `onus query` of a repository talks to one map server over a private Unix socket, and starts it if it is not running. Ten agents in a worktree share one map and one rebuild.
- **One map per worktree, kept current.** The server watches each worktree's files. A change marks the map stale; the next question rebuilds it once, however many agents are asking, and questions between changes are answered from memory in about a millisecond.
- **Facts shared across worktrees.** Parsing a file is most of the work, and worktrees share almost all of their files. The server keeps each file's facts by content, in memory and in the repository's git folder (`.git/onus/facts`), so a second worktree, a rebuild after an edit, or a restarted server parses only files whose content is new.
- **Checks reuse base commits.** `onus_check` keeps the extracted base commit and its map, so agents checking against the same commit pay for it once.

Measured on Twenty, an open-source TypeScript monorepo with 31,000 source files, 72,000 symbols and 272,000 edges (Apple M-series laptop):

| | Time |
|---|---|
| A question to a warm map (MCP tool call) | under 1 ms; `onus_find` about 30 ms |
| The same from `onus query` (a new process each time) | 20–50 ms |
| First question in a repository (full build) | 4.8 s |
| First question in a second worktree | 1.9 s |
| First question after an agent edits a file | 2.0 s |
| Eight agents asking at once after an edit | 1.7 s, one rebuild |
| `onus_check` against a commit already checked | 2.4 s |

The server exits after 30 minutes without questions. `onus query stats` shows its worktrees and cache use; `--no-server` builds the map in the calling process instead (the only mode on Windows).

## What it reads and runs

The same as every other command: the files of the worktree and, for `onus_check`, the base commit through `git archive`. Nothing from the repository is executed, the server only listens on a socket readable by your user, and no plugins are loaded.
