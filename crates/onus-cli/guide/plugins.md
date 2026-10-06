# Plugins: new languages, frameworks and compiler-backed facts

Onus builds a map with *providers*:

- **discovery** providers find components (built in: `onus.yaml`, Nx projects, npm/pnpm/yarn workspaces, folders);
- **language** providers turn source files into symbols, shapes, references and tests (built in: TypeScript and JavaScript on tree-sitter);
- **fact** providers add facts to the map built so far (SCIP indexes, framework plugins).

Plugins add providers without changing Onus. They are listed in a *plugins file* that you pass with `--plugins <file>` (or `ONUS_PLUGINS`). The plugins file belongs to whoever runs Onus: Onus never reads plugins from the analyzed repository, so a pull request cannot make CI run new code.

## Framework packs: no code at all

Most framework knowledge is a pattern in the code: a decorator, a method on a known client, a call with a literal argument. A *pack* is a YAML file of tree-sitter queries and what a match means. Packs never run code, so they may be listed in the analyzed repository's `onus.yaml` (`extractors.packs`) as well as in the plugins file (`packs`). Like the rest of `onus.yaml`, packs are read from the base version of a change: a pull request cannot edit a pack to hide what it adds, and editing one is reported as a rules-of-the-game change.

    pack: nestjs
    rules:
      - id: route
        query: |
          ((decorator (call_expression
             function: (identifier) @method
             arguments: (arguments . (string) @path)))
           (#match? @method "^(Get|Post|Put|Patch|Delete)$"))
        emit: { route: "{method|upper} {path}" }
      - id: on-event
        query: |
          ((decorator (call_expression
             function: (identifier) @fn
             arguments: (arguments . (_) @name)))
           (#eq? @fn "OnEvent"))
        emit: { event: consumes, name: "@name" }
      - id: cache-write
        query: |
          ((call_expression function: (member_expression
             object: (identifier) @c property: (property_identifier) @op))
           (#eq? @c "redis") (#eq? @op "set"))
        emit: { edge: writes, to: "db-table:redis-cache" }

Each rule has an `id`, a tree-sitter `query` over the TypeScript grammar (the same query runs on TSX and JavaScript), and one `emit`:

| Emit | Means |
|---|---|
| `event: publishes` or `consumes`, `name: "@capture"` | An event, named by the capture; literals, constants and enum members are resolved |
| `data: reads` or `writes`, `table: "<template>"` | A table read or written |
| `config: "<template>"` | A configuration key read |
| `host: "@capture"` | An outbound call to the host of the captured literal URL |
| `route: "<template>"` | An HTTP route served here: a public contract, so an added or removed route is a contract change |
| `edge: <kind>`, `to: "<template>"` | Any other relationship (`calls`, `reads`, `writes`, `publishes`, `consumes`, `calls-external`, `reads-config`, ...) |
| `diagnostic: <kind>`, `message: "<text>"` | A map confidence note |

Templates replace `{capture}` with the captured text (a string literal without its quotes) and accept `{capture|upper}` and `{capture|lower}`. Facts are attributed to the innermost declaration containing the match. Query predicates (`#eq?`, `#match?`, `#any-of?`) work as in tree-sitter.

Onus's own framework knowledge ships as packs too: Prisma reads and writes, `process.env` reads, `fetch`/`axios`/`got`/`ky` hosts, and the event patterns from `onus.yaml` (`crates/onus-lang-ts/packs/` in the repository). An invalid pack is a configuration error that names the pack and rule.

## The plugins file

    plugins:
      # A plugin speaking the Onus protocol: finds Go modules as components.
      - name: go-modules
        kind: discovery
        command: [onus-plugin-example]

      # A framework plugin: Express routes become contracts.
      - name: express-routes
        kind: facts
        command: [onus-plugin-example]

      # A SCIP indexer, run in the tree; it must write {out}/index.scip.
      - name: rust
        kind: scip
        command: [rust-analyzer, scip, ".", --output, "{out}/index.scip"]

      # A language server, driven over LSP.
      - name: python
        kind: lsp
        command: [pyright-langserver, --stdio]
        files: ["**/*.py"]
        language_id: python

    packs: [packs/nestjs.yaml]   # framework packs, relative to this file

    sandbox:
      preset: auto        # auto | bwrap | sandbox-exec | container | none
      # command: [docker, run, --rm, -i, --network, none, -v, "{root}:{root}:ro", -v, "{out}:{out}", -w, "{root}", my-image]

Each plugin has:

- `name`: recorded in the map's `builtWith.providers`;
- `kind`: `discovery`, `language`, `facts`, `scip` or `lsp`;
- `command`: the program and its arguments. `{root}` becomes the tree's root and `{out}` a fresh, writable temporary directory;
- `files` (language and lsp): globs of the files it handles;
- `language_id` (lsp): the LSP language id of those files;
- `runs_repo_code`: whether the tool may run code from the repository. Default: true for `scip` and `lsp`, false otherwise;
- `timeout_seconds`: default 300.

The format is in schemas/onus-plugins.schema.json.

## Trusted mode and the sandbox

Language servers and indexers are the most precise source of facts, because they are built on each language's own compiler. They also tend to run code from the repository: rust-analyzer runs build scripts and procedural macros, gopls runs `go list`, tsserver loads plugins named in tsconfig, Gradle and Maven run build scripts. Most of them also need dependencies installed.

So a plugin with `runs_repo_code: true` runs only with `--trusted`, and only inside a sandbox with no network:

- `auto`: bubblewrap (`bwrap`) on Linux, `sandbox-exec` on macOS;
- `bwrap` or `sandbox-exec`: that one;
- `container`: Docker or Podman (`runtime`) running `image`, with no network, the tree read-only and `{out}` writable; the tools must be in the image. With Docker Desktop or Colima, the tree and the temporary folder (`TMPDIR`) must be in a shared folder. This is the option on Windows;
- a custom `command` prefix;
- `none`: no sandbox; such plugins then also need `--allow-unsandboxed`.

In every case the tree is read-only and the network is unreachable; only `{out}` and the system's temporary folders are writable.

Without `--trusted` they are skipped, and both the map and the report say so ("provider `rust` skipped: ... runs only with --trusted"). Use trusted mode where the code is already trusted, for example on the main branch, and keep pull requests from forks on the default providers, which only read files.

Importing an index that was produced elsewhere runs nothing and needs no trusted mode:

    onus map . --scip index.scip
    onus diff old new --base-scip old.scip --head-scip new.scip

## SCIP indexes

SCIP indexers index a whole repository in one pass:

| Language | Indexer | Command |
|---|---|---|
| Rust | rust-analyzer | `rust-analyzer scip . --output {out}/index.scip` |
| TypeScript, JavaScript | scip-typescript | `scip-typescript index --output {out}/index.scip` |
| Python | scip-python | `scip-python index . --output {out}/index.scip` |
| Java, Scala, Kotlin | scip-java | `scip-java index --output {out}/index.scip` |
| Go | scip-go | `scip-go --output {out}/index.scip` |

Flags change between releases; check each indexer's documentation for the current ones.

What an index adds:

- for files another provider analyzed: references that provider missed, such as a method call through a variable, and confirmation of the ones it found (confidence `compiler`);
- for files no provider handles: their symbols, with the signature as shape, and their references. A symbol used from another component is public.

## Language servers

For languages without an indexer, the LSP bridge starts the server, opens each file it handles, waits until the server has finished loading the project (it follows the server's progress reports), and asks for document symbols and, where the server supports it, the call hierarchy. Symbols become map symbols (their detail, usually the signature, is the shape); outgoing calls become `calls` edges with confidence `compiler`. Language servers answer one request at a time, so this is slower than an index on large repositories. Tried with rust-analyzer (`rust-analyzer`, language id `rust`) and pyright (`pyright-langserver --stdio`, language id `python`).

## Writing a plugin

A plugin is any program. Onus starts its command, writes one JSON request to its stdin and reads one JSON response from its stdout; anything on stderr is shown if the plugin fails. The protocol is versioned and published as schemas/plugin-protocol.schema.json.

A request:

    {
      "protocol": 1,
      "kind": "facts",
      "workspace": {
        "root": "/abs/path/to/tree",
        "files": [{ "path": "services/api/src/app.ts", "component": "api", "isTest": false }],
        "components": [{ "id": "api", "kind": "service", "roots": ["services/api/**"], ... }],
        "packages": {},
        "extractors": { ... }
      },
      "map": { "files": [...], "symbols": [...], "edges": [...], "tests": [...], "diagnostics": [...] }
    }

- `discovery` requests list every file and ask for `components`.
- `language` requests list only the files the plugin handles that no earlier provider claimed (the built-in TypeScript adapter comes first); any file under `root` can still be read.
- `facts` requests include the map built so far.

A response:

    {
      "protocol": 1,
      "version": "my-plugin 1.2.0",
      "components": [],
      "map": { "symbols": [...], "edges": [...] }
    }

Everything in `map` uses the map's own types (schemas/codebase-map.schema.json). Ids follow the map's rules: `<component>:<path inside the component>#<name>` for symbols, and `event:`, `db-table:`, `config-key:`, `external:`, `npm:` for shared nodes.

Onus treats plugin output as untrusted input. It drops any path outside the tree, any file the tree does not contain, and any id of an unknown component, notes what it dropped (`plugin-output-dropped`), and sorts everything, so a report stays deterministic as long as the plugin is.

The reference plugin, `crates/onus-plugin-example` in the repository, implements discovery (Go modules) and facts (Express-style HTTP routes, which become public contracts, so added or removed routes show up in reports) in about 150 lines.

## Order and precedence

1. Discovery: `onus.yaml` components, when declared, are final. Otherwise built-in discovery runs first and discovery plugins add components; for each file, the most specific component wins.
2. Languages: the built-in TypeScript adapter, then language plugins, then language servers, in the order listed. Each file belongs to the first provider that handles it.
3. Facts: imported indexes (`--scip`), then SCIP indexer plugins, then fact plugins, in the order listed. Each sees what came before.

When two providers report the same edge, its sites are combined and it keeps the more trustworthy confidence, so a compiler-confirmed reference upgrades one read from syntax.
