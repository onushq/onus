# Onus user guide

The guide ships inside the `onus` binary: `onus help <topic>` prints any of these pages in the terminal, offline. The source files live in [`crates/onus-cli/guide/`](../crates/onus-cli/guide/).

| Topic | What it covers |
| --- | --- |
| [getting-started](../crates/onus-cli/guide/getting-started.md) | Install Onus, build a map and read your first report |
| [commands](../crates/onus-cli/guide/commands.md) | Every command and flag, with examples and exit codes |
| [reports](../crates/onus-cli/guide/reports.md) | Reading a report: rows, ranking, evidence and the JSON |
| [changes](../crates/onus-cli/guide/changes.md) | Every kind and subkind of change Onus reports |
| [configuration](../crates/onus-cli/guide/configuration.md) | The `onus.yaml` reference: components, rules, labels, extractors |
| [intent](../crates/onus-cli/guide/intent.md) | Checking a change against its stated intent |
| [ci](../crates/onus-cli/guide/ci.md) | Running Onus on every pull request, and outcome records kept by CI |
| [agents](../crates/onus-cli/guide/agents.md) | The map for coding agents: `onus mcp`, its tools, many agents and worktrees |
| [scopes](../crates/onus-cli/guide/scopes.md) | Task tokens, the git gateway, escalation with evidence and the audit log |
| [lanes](../crates/onus-cli/guide/lanes.md) | Risk lanes, change submissions, the verifying judge and outcome records |
| [web-interface](../crates/onus-cli/guide/web-interface.md) | The web interface: onus ui opens all of it in your browser |
| [environments](../crates/onus-cli/guide/environments.md) | Environments built from a commit, the evidence store, and traces into the map |
| [plugins](../crates/onus-cli/guide/plugins.md) | New languages and frameworks, SCIP indexes, language servers, trusted mode |
| [how-it-works](../crates/onus-cli/guide/how-it-works.md) | How maps are built and compared |
| [troubleshooting](../crates/onus-cli/guide/troubleshooting.md) | Map confidence notes, common questions and current limits |

In the terminal:

```sh
onus help                  # commands and guide topics
onus help diff             # one command's flags and examples
onus help configuration    # a guide topic
```
