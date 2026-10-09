# The web interface

`onus ui` opens everything Onus does for a repository in your browser: the map, reports, lanes and the judge, environments and evidence, tokens, escalations, the audit log and outcomes. It ships inside the binary; nothing is installed and nothing leaves your machine.

    onus ui                      # the repository containing the current directory
    onus ui --repo ../shop --port 0 --no-open

It prints an address such as `http://127.0.0.1:4387/#token=…` and opens it.

## What it shows

| Page | What you can do |
|---|---|
| Overview | Map totals, the component graph, uncommitted work, recent commits, what is set up |
| Map | The component graph, components, files, events and external services; find symbols; per component, symbol and file: owners, public surface, dependents, dependencies, tests, declared invariants, and the impact of a change |
| Changes | Check uncommitted work (what `onus_check` gives an agent), or report any two refs, with an intent; download Markdown or JSON |
| Lanes & judge | The lane policy; make a submission with evidence from the store, classify it and run the judge |
| Environments | Create environments, run commands, browse the evidence store and its artifacts, run a test once |
| Outcomes | Records, totals per agent setup and judge, the human-lane share and audit miss rate; record outcomes and incidents, find reverts |
| Tokens & scopes | Generate a root key, plan a task, suggest reads, mint, inspect, check and narrow tokens |
| Escalations | File requests with evidence; decide by policy, grant or deny |
| Audit log | Every decision, and whether the hash chain is intact |
| onus.yaml | What is declared, and what `onus init` would suggest |
| Guide | This guide |

The map follows the files on disk, so pages show your work as you edit.

## Where things are kept

- Outcome records: `.onus/outcomes.jsonl` (`--outcomes`)
- The audit log: `.onus/audit.jsonl` (`--audit`)
- Escalation requests: `.onus/escalations/` (`--escalations`)
- The root key pair: `~/.onus-keys` (`--keys`)
- Evidence: the repository's git directory, as for `onus env`

## Security

The server listens on 127.0.0.1 only and answers only requests addressed to a loopback name, so a web page elsewhere cannot reach it through DNS rebinding. Its API needs the session token from the address it printed; the token travels in the address's fragment, which browsers never send to a server, and the page keeps it for the tab only. Cross-origin requests are refused. Anyone who can run programs as you can already do what the interface does.

## Building from source

Release binaries include the interface. A build from source embeds it when `ui/build` exists:

    cd ui && npm ci && npm run build
    cargo build --release -p onus-cli

Without it, `onus ui` serves the API and a page that says how to add the interface. To work on the interface itself, run `ONUS_UI_TOKEN=dev onus ui --no-open` and, in `ui/`, `npm run dev`, then open `http://localhost:5173/#token=dev`.
