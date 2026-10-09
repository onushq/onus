# 0011. A web interface shipped in the binary

Status: Accepted (2026-10-09)

## Context

Everything Onus does is reachable from the command line, the GitHub Action and MCP. People reviewing a change, setting up lanes or deciding an escalation want to look around: follow a component to its dependents, open the file behind a row, compare two refs, read what an environment recorded. Doing that through JSON on a terminal is slow.

## Decision

1. **`onus ui`** serves a web interface for one repository, with the whole of Onus behind it: the map, reports, lanes and the judge, environments and evidence, tokens, escalations, the audit log and outcomes.
2. **A static app embedded in the binary.** The interface is a SvelteKit app in `ui/` (Svelte 5, Tailwind CSS and shadcn-svelte components, themed with the Onus colors), built to static files (one page, no server rendering) and embedded at compile time by `crates/onus-cli/build.rs`. Release builds require it (`ONUS_REQUIRE_UI`); a build from source without it still works and says how to add it. Nothing is downloaded at run time; fonts are bundled.
3. **A JSON API over the same code as the commands.** `POST /api/<name>` calls the functions the commands call (submissions, classification, the judge, minting, escalation decisions, environments), and map questions go to the same in-process map server as `onus mcp`, which follows the files on disk. The interface computes nothing itself, so what it shows is what the commands print.
4. **Local and per session.** The server binds 127.0.0.1, answers only requests whose `Host` is a loopback name (against DNS rebinding), refuses cross-origin requests, and serves the API only with a random session token. The token is in the opened address's fragment, which browsers do not send to servers, and the page keeps it for the tab. Responses carry a content security policy that allows only the app's own files. Text that quotes the repository is never rendered as HTML.
5. **State lives in files.** Outcome records, the audit log and escalation requests go to `.onus/` in the repository unless flags say otherwise, so the interface and the commands work on the same records.

## Consequences

- The binary grows by the app's size (about 1.2 MB, fonts included).
- Building the release needs Node for one job; Rust-only builds and tests do not.
- The interface is a view and a set of buttons over existing commands: new features land in the commands first, and the interface follows.
