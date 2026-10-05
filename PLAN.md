# Onus: Implementation Plan

Source: the manifesto *Code Is Not the Product* by Mikias Abebe (Sep 28, 2026).
Status: **plan updated 2026-10-05.** Nothing is implemented yet. Onus is written in Rust and will be fully open source under Apache-2.0. Section 2 lists the decisions; the open ones are marked.

**Naming.** The project and the software are both called **Onus**. The name comes from *onus probandi*, the burden of proof: today that burden sits on a reviewer who has to find problems by reading lines, and Onus moves it to the agent, which has to prove its change with evidence. Tagline: *"The onus is on the agent."*

---

## 1. What we are building

Onus is the layer that every coding agent works through. It is not an agent itself. It does four things:

1. **Understands changes as shifts in meaning.** It uses a codebase map derived from the code and a semantic diff over that map.
2. **Scores every change and every request for access by risk.** A classifier routes each one to a lane: auto-merge, judge, human or blocked.
3. **Approves by evidence.** A verifying judge re-runs the evidence in fresh environments.
4. **Governs agents at the doors.** Scoped, short-lived tokens are checked at MCP servers, the CLI and a git gateway. Agents escalate with evidence, not persuasion.

The two load-bearing ideas are **the map** (machines understand what a change means) and **evidence** (machines can trust it). Everything else is plumbing around those two, so the build order follows the manifesto's roadmap:

```
Phase 1  Semantic change reports on today's PRs     ← the wedge; builds map v1
  gate:  reviewers prefer the report to the raw diff on large PRs
Phase 2  The map as a service (MCP + CLI)
  gate:  map answers match reality on a sampled question set
Phase 3  Scoped tokens + evidence-based escalation
  gate:  escalations rare enough not to stall work; zero out-of-scope writes land
Phase 4  Risk lanes + the verifying judge
  gate:  audits of auto-approved changes find an acceptably low miss rate
Phase 5  Evidence factory, production loop, contract-level scopes
```

Each phase must ship something useful on its own. Phase 1 gets planned in detail. Phases 2–5 are planned at the level of architecture and gates, and each gets its own detailed plan once the previous gate is passed.

### Target architecture (end state)

```
   agents (Claude Code, Codex, custom)            people (CLI, PR UI)
                │ MCP / CLI — every call: token check + audit log
 ┌──────────────┴─────────────────────────────────────────────────────┐
 │ doors:  map.query  scope.request  env.create/run/destroy            │
 │         change.submit  escalate            (+ git gateway for push) │
 └──┬───────────────┬──────────────────┬──────────────┬───────────────┘
    │               │                  │              │
 ┌──▼─────────┐  ┌──▼───────────┐  ┌───▼──────────┐ ┌─▼──────────────┐
 │ Codebase   │  │ Token service│  │ Evidence     │ │ Classifier     │
 │ map        │◄─┤ (Biscuit,    │  │ factory      │ │ (rules floor + │
 │ static +   │  │ per task)    │  │ (disposable  │ │ learned model) │
 │ traces +   │  └──────────────┘  │ envs)        │ └─┬──────────────┘
 │ declared   │                    └───▲──────────┘   │ lanes
 └──▲───┬─────┘                        │          ┌───▼──────────────┐
    │   └── semantic diff ─────────────┼─────────►│ Judge (verify    │
    │                                  └──────────┤ first, taste last)│
    │                                             └───┬──────────────┘
    │                                         merge + staged rollout
    └──── traces / incidents / reverts ◄──── production signals
                 (provenance log links every step)
```

---

## 2. Decisions to confirm before implementation

These are my recommended defaults. The plan below assumes them, and changing any of them now costs nothing.

| # | Decision | Recommendation | Why | Alternatives |
|---|---|---|---|---|
| D1 | Language Onus itself is written in | ✅ **Decided: Rust.** The engine and the `onus` CLI are one Cargo workspace that ships a single static binary per platform. | One binary with no runtime to install runs the same in CI, on laptops and inside agent sandboxes. Fast enough to map a large monorepo on every pull request. Strong typing for the schemas everything else depends on. tree-sitter is native to Rust. MCP comes later through the official Rust SDK (`rmcp`). | TypeScript on Node (richer TS type information, but needs a Node runtime everywhere); Go |
| D2 | First language Onus *analyzes*, and how | **TypeScript/JavaScript, parsed with tree-sitter.** Onus does its own module and name resolution; contract shapes are read from the syntax. | Agent-written code is heavily TS. Declared shapes (interface members and their `?`, parameter lists, return annotations) are enough to tell an optional field from a required one. tree-sitter grammars exist for every language we will add next, behind the same adapter trait. Trade-off: no type checker, so inferred or computed types are marked `unverified` and treated as higher risk. | `oxc` (Rust, TS-only, richer semantics, faster-moving API); shelling out to the TypeScript compiler (precise types, needs Node) |
| D3 | Phase 1 delivery surface | **CLI + GitHub Action** (no server). The Action is a thin composite action that downloads the release binary and runs `onus report`. | Zero infrastructure. Works with GitHub as it is today. No Node bundle to maintain. A GitHub App can come later for org-wide installs. | GitHub App from day one (needs hosting, auth, webhooks) |
| D4 | Map storage | **In-memory graph + versioned, deterministic JSON snapshot** (Phase 1, `serde`); add a SQLite store (`rusqlite`) behind the same interface in Phase 2 | JSON is golden-testable and easy to debug, and Phase 1 only needs two maps in memory to diff | SQLite from day one |
| D5 | Token format (Phase 3) | **Biscuit** (its reference implementation is the Rust crate `biscuit-auth`) | Public-key verification at many doors without a shared secret, offline attenuation (narrow, never widen), and Datalog caveats that express path and contract scopes | Macaroons (HMAC, so every verifier needs the root secret) |
| D6 | Repo tooling | Cargo workspace on stable Rust (MSRV pinned in `rust-toolchain.toml`), `rustfmt`, `clippy -D warnings`, `insta` snapshot tests for maps and reports, `cargo-deny` for licenses and advisories, `cargo-dist` for release binaries and installers | Conventional for Rust, reproducible, and easy for agents to work in | `cargo-nextest` later if test time grows |
| D7 | Name | ✅ **Decided: Onus.** CLI `onus`, config `onus.yaml`, npm scope `@onushq` (`@onus` is taken on npm; the installed command can still be `onus`), domain **onushq.com**, GitHub org **`onushq`** (matches the domain; the `onus` handle is a dormant 2018 org). | Short, says the core idea (the burden of proof moves to the agent), and has no collision in developer tools. Rejected after collision checks: Writ, Keycard, Interlock, Tracon, Clearway, Clearance. | Probative, IFR (both clean, less catchy) |
| D8 | License / distribution | ✅ **Decided: fully open source, Apache-2.0.** Public repository `onushq/onus`; contributions under the Developer Certificate of Origin (`Signed-off-by`), no CLA. | Permissive like MIT, plus an explicit patent grant, which matters for a tool companies run in CI. A DCO keeps contributing simple. See section 7a. | MIT (no patent grant); a CLA (more paperwork for contributors) |

---

## 3. Engineering principles for building Onus

These come straight from the manifesto and apply to every phase.

1. **Facts are computed, never generated.** Report rows, change kinds, blast radius and rule violations come from deterministic analysis. LLMs are optional and limited to prose, intent extraction and the judge's "taste" step. A wrong fact in the report is worse than a missing one.
2. **Conservative under uncertainty.** Low-confidence parts of the map (dynamic imports, unresolved symbols, reflection) *raise* risk and are always shown.
3. **Derive, don't document.** The map is rebuilt from code like a build artifact. People declare only a thin layer: contracts, owners, risk labels and rules.
4. **Every capability is a door.** Each feature ships as a CLI command with `--json` output. From Phase 2 it is also an MCP tool. No capability exists only in a UI.
5. **Schemas are the product surface.** The map, semantic changes, change applications, escalations and provenance records have versioned JSON Schemas generated from `crates/onus-core` and committed under `schemas/`. This is also a concrete answer to the manifesto's open question about a standard semantic-change format.
6. **No execution of analyzed code until Phase 3+.** Phase 1 is pure static analysis: no `npm install`, no running tests. That makes it safe on fork PRs.
7. **Dogfood from Phase 1.** The Onus repo runs its own report bot on every PR.
8. **Every phase has a measurable gate.** Instrumentation ships with the feature, not after it.

---

## 4. Core data model (`crates/onus-core`)

Sketch only, written as TypeScript for readability. The real definitions are Rust structs with `serde`, serialized to exactly this JSON shape, with JSON Schema generated from them (`schemars`) so tools in any language can read Onus output.

```ts
// ---- The codebase map ----
type Confidence = "declared" | "static" | "traced" | "inferred" | "low";

interface CodebaseMap {
  schemaVersion: 1;
  commit: string;                 // git sha the map was built from
  builtWith: { onus: string; adapters: Record<string, string>; configHash: string };
  components: Component[];
  symbols: SymbolNode[];          // functions, classes, types, events, routes, schemas…
  edges: Edge[];
  externals: ExternalService[];
  packages: PackageDep[];         // third-party deps per component
  tests: TestNode[];
  rules: BoundaryRule[];          // from the declared layer
  diagnostics: MapDiagnostic[];   // unresolved imports, dynamic code → low confidence
}

interface Component {
  id: string;                     // "notifications"
  kind: "package" | "service" | "module";
  roots: string[];                // globs
  publicEntrypoints: string[];    // define the public surface (contracts)
  owners: string[];               // from onus.yaml or CODEOWNERS
  labels: string[];               // "payments" | "auth" | "pii" | custom
  confidence: Confidence;
}

interface SymbolNode {
  id: string;                     // stable: "user-preferences:src/types.ts#UserPreferences"
  componentId: string;
  kind: "function" | "class" | "method" | "interface" | "type" | "enum" | "const"
      | "event" | "http-route" | "db-table" | "config-key";
  visibility: "public" | "internal";   // public = reachable from a public entrypoint
  shape?: ContractShape;          // normalized signature / members, for contract diffing
  bodyFingerprint?: string;       // normalized-AST hash, for rename/move detection
  invariants?: string[];          // declared by humans
  loc: { file: string; start: number; end: number };
}

interface Edge {
  from: string; to: string;       // symbol, component, event, external or table ids
  kind: "imports" | "calls" | "references-type" | "reads" | "writes"
      | "publishes" | "consumes" | "calls-external" | "reads-config";
  confidence: Confidence;
  sites: { file: string; line: number }[];
}

// ---- Semantic changes ----
type ChangeKind = "additive" | "breaking" | "internal" | "dependency"
                | "config" | "test" | "security-sensitive";   // manifesto's kinds

interface SemanticChange {
  id: string;
  kind: ChangeKind;
  subkind: string;                // "contract-field-added-optional", "new-external-service",
                                  // "new-event-consumer", "rename", "boundary-condition-changed",
                                  // "test-assertion-removed", "rule-violation", …
  level: "relationship" | "behavior" | "structure";
  subject: string;                // map id
  title: string;                  // "UserPreferences gains an optional phoneVerified field"
  whyItMatters: string;           // template-generated, deterministic
  hints: {
    labels: string[];             // sensitivity labels touched
    blastRadius: number;          // dependents in the head map
    novelty: string[];            // "new-vendor", "new-data-egress:phone", …
    confidence: Confidence;
    rulesOfTheGame: boolean;      // tests/CI/policy/labels/contracts touched
  };
  locations: { file: string; lines: [number, number] }[];   // "one click away"
  stats?: { linesAdded: number; linesRemoved: number; files: number };
}

interface SemanticReport {
  schemaVersion: 1;
  base: string; head: string;
  changes: SemanticChange[];      // ranked
  intentCheck?: { stated: string; mismatches: string[] };
  ruleViolations: SemanticChange[];
  textStats: { files: number; linesAdded: number; linesRemoved: number };
  mapDiagnostics: MapDiagnostic[];
}
```

Phases 3–5 add `ScopeToken`, `EscalationRequest`, `ChangeApplication`, `JudgeVerdict`, `Lane`, `AgentSetup` and `ProvenanceRecord` to the same package.

---

## 5. Phase 1 in detail: semantic change reports

**Goal:** a bot that turns a 2,000-line PR into a one-page summary of meaning changes, on GitHub as it is today, regardless of who or what wrote the PR.

### 5.1 Pipeline

```
onus report --base <ref> --head <ref> [--format md|json] [--intent-file f]

 1. Materialize base and head (git worktrees in a temp dir; no installs)
 2. Load config: onus.yaml if present, otherwise infer (workspaces, CODEOWNERS, heuristics)
 3. Build map(base) [cached by sha+config+version] and map(head) via language adapters
 4. Text stats (git diff --numstat) for line attribution
 5. Semantic diff: match components → symbols → edges → externals → packages → tests
 6. Classify each difference into kind/subkind; attach hints (labels, blast radius, novelty)
 7. Evaluate declared boundary rules on head; report only *new* violations
 8. Intent check (if an intent is stated)
 9. Rank, collapse internal churn per component, render markdown + JSON
```

### 5.2 TypeScript adapter (`crates/onus-lang-ts`)

- **Parsing:** tree-sitter with the TypeScript and TSX grammars, one parse per file, in parallel (`rayon`). Nothing from the analyzed repository is installed or executed.
- **Module resolution:** Onus resolves import specifiers itself: relative paths, `index` files, `tsconfig` `paths` and `baseUrl`, and workspace package names mapped to their source entrypoints. Third-party imports become opaque `npm:<pkg>` nodes.
- **Components** come from pnpm/npm/yarn workspaces, tsconfig project references or `onus.yaml`. The fallback is top-level directories under `services/`, `packages/` or `apps/`.
- **Public surface:** symbols reachable from a component's entrypoints (`package.json` `exports`/`main`/`types` mapped back to source), following re-exports. Everything else is internal.
- **Symbols and shapes:** exported functions (parameters with optionality, return annotation), interfaces and type aliases (members with optionality and normalized type text), classes, enums and consts. Shapes come from the syntax; where a type is inferred rather than written, the shape is marked `unverified`. Each symbol gets a `bodyFingerprint`: a hash of its syntax tree with identifiers of locals, formatting and comments normalized away.
- **Name resolution:** each file builds a scope table (local declarations, imports with their local aliases). Identifiers in calls and type references resolve through it to declaration ids, across files via the module resolver. Dynamic access (`obj[key]`, `require(variable)`) is recorded as a low-confidence diagnostic.
- **Extractors** (tree-sitter queries, configurable in `onus.yaml`):
  - *External services:* a registry maps SDK packages to vendor, category and default data egress (e.g. `twilio` → SMS, egress: phone; `stripe` → payments). Also `fetch`/`axios`/`got` calls with literal hosts. Users extend it in `onus.yaml`.
  - *Events:* call patterns such as `bus.publish($EVENT, …)`, `bus.subscribe($EVENT, …)`, `@OnEvent($EVENT)` → `publishes` / `consumes` edges to `event:<name>`.
  - *Data access:* Prisma (`prisma.<model>.<create|update|delete|find…>`) → `reads` / `writes` edges to `db-table:<model>`. Other ORMs come later.
  - *Config:* `process.env.X` → `reads-config`.
- **Tests:** test files are matched by globs. For each, extract test cases, assertion counts, `.skip`/`.only`/`xit`/`todo` markers and snapshot usage. Static `exercises` edges are the symbols referenced from test files; real coverage arrives in Phase 5.
- **Adapter trait** (`LanguageAdapter::build(&Workspace) -> PartialMap`) lives in `onus-core` from day one, so Python, Go and others plug in through their tree-sitter grammars without touching the diff engine.

### 5.3 Semantic diff engine (`crates/onus-diff`)

Matching runs in order:

1. **Symbols by stable id.** Ids that appear only on one side go to the next steps.
2. **Rename and move detection.** An unmatched removed/added pair in the same component with an equal (or ≥0.9 similar) body fingerprint, plus references updated, is a *rename* or *move*. This produces one internal row, not hundreds of lines.
3. **Contract diff on public symbols:**
   - optional member or parameter added → `additive`
   - member or parameter removed, required parameter added, required member added to an input-position type, export removed → `breaking`
   - type text changed in ways that can't be proven compatible → `breaking` (conservative) with subkind `contract-changed-unverified`
4. **Edge diff.** New or removed cross-component edges, new event publishers or consumers, new data reads or writes, new external calls.
5. **Packages and externals.** A new third-party dependency gives `dependency`. A new vendor or new data egress category gives `security-sensitive` + novelty.
6. **Config.** Changes to `.env*`, YAML/JSON config, Dockerfiles and `package.json` scripts give `config`. CI workflows, `onus.yaml`, CODEOWNERS and policy files also get `rulesOfTheGame: true`.
7. **Tests.** Added tests or cases are reported as coverage gains. Removed cases, fewer assertions, new skip/only markers, edited expected literals and source code that special-cases literals found in tests all count as **weakened** (`test`, rules of the game).
8. **Notable edits inside bodies.** Changes in matched function bodies are checked for flipped comparison operators in conditions (`>` → `>=`), changed numeric or string constants in conditions, removed guards or throws, removed `await` and swallowed errors. In a labeled component these get promoted to `security-sensitive`.
9. **Secrets.** Added lines are scanned for high-confidence secret patterns (gitleaks-style rules) → `security-sensitive`.
10. **Internal bucket.** All other changes inside a component collapse into one row: "~N lines of code and tests inside `X`; no other component touched".

**Ranking:** security-sensitive and intent mismatches first, then breaking, dependency/novelty, rules-of-the-game, config, additive contracts, new relationships, tests, internal. Ties break by sensitivity labels, then blast radius.

### 5.4 Declared layer: `onus.yaml` (optional)

```yaml
version: 1
components:
  orders:            { path: "services/orders/**",            owners: ["@team-orders"], labels: [pii] }
  notifications:     { path: "services/notifications/**",     owners: ["@team-growth"] }
  user-preferences:  { path: "services/user-preferences/**",  labels: [pii] }
  billing:           { path: "services/billing/**",           labels: [payments] }
contracts:
  UserPreferences:
    symbol: "user-preferences:src/types.ts#UserPreferences"
    invariants: ["phone numbers are stored in E.164"]
rules:
  - deny: { from: billing, to: notifications.internal }   # "billing may not reach into notifications' internals"
labels:
  payments: { sensitivity: high }
  auth:     { sensitivity: high }
  pii:      { sensitivity: high }
extractors:
  events:
    publish:   ["bus.publish($EVENT, ...)"]
    subscribe: ["bus.subscribe($EVENT, ...)"]
  externals:
    "@acme/sms-sdk": { vendor: "Acme SMS", category: sms, egress: [phone] }
```

`onus init` writes a starter file inferred from the repo. Inferred labels (for example, a path containing `auth/`) are marked `inferred` and shown as suggestions, never enforced silently.

### 5.5 Intent check

The manifesto calls the gap between stated intent and actual effect "the most important signal in the whole review". Phase 1 has two levels:

- **Deterministic:** an optional fenced block in the PR body. Agents can be told to emit it.
  ````
  ```onus-intent
  summary: Add SMS alerts on OrderShipped
  touches: [notifications]
  contracts: [UserPreferences]      # contracts I expect to change
  externals: [twilio]
  ```
  ````
  Any semantic change outside the declared `touches`, `contracts` and `externals` is a mismatch and goes to the top of the report.
- **Optional LLM assist** (off by default, behind a flag): extract the same structure from free-text PR descriptions. The LLM only produces the *expected* side. The comparison stays deterministic.

### 5.6 Report output

A sticky PR comment (identified by a hidden marker and updated in place) plus the job summary, with the full JSON as a workflow artifact:

```
Onus · 4 meaning-level changes · 1 needs attention · 1,412 lines in 23 files

    Change                                               Kind                      Why it matters
 ●  notifications now calls an external SMS provider     New external dependency   Customer phone numbers leave the system
 ●  UserPreferences gains optional phoneVerified         Contract change, additive Shared contract (3 dependents); callers unaffected
 ○  notifications subscribes to OrderShipped             New event consumer        Additive; orders is unchanged
 ○  ~1,400 lines of new code and tests in notifications  Internal                  No other component touched

 Intent check: matches stated intent         Boundary rules: no new violations
 ▸ Evidence per row (files and line links)   ▸ Map confidence notes   ▸ Was this useful? 👍 / 👎
```

### 5.7 GitHub Action (`action/`)

- A composite action (`onushq/onus@v1`) that downloads the released `onus` binary for the runner's platform, verifies its checksum, and runs `onus report`. It triggers on `pull_request` and needs only `contents: read` and `pull-requests: write`. Posting the sticky comment uses `gh api` with the workflow token.
- Caches `map(base)` with `actions/cache`, keyed by base sha + config hash + Onus version.
- Inputs: `config`, `comment: sticky|off`, `fail-on: rule-violation|secrets|none`, `max-rows`.
- Outputs: path to the report JSON and the highest-severity kind, so later workflow steps can branch on it.
- Fork PRs are safe because nothing from the PR is executed or installed.
- The CLI also runs anywhere else: `onus report --base main --head HEAD` locally, or in GitLab, Buildkite or Jenkins.
- A companion `issue_comment` workflow handles `/onus caught <note>`. It records that the report caught something the diff hid, which feeds the Phase 1 gate metric.

### 5.8 Golden scenarios (Phase 1 acceptance criteria)

A fixture monorepo, `fixtures/shop`, mirrors the manifesto's example: `orders`, `notifications`, `user-preferences` and `billing`, with an event bus and Prisma. Each scenario is a scripted commit pair with an expected-report snapshot:

| # | Scenario (from the manifesto) | Expected report |
|---|---|---|
| S1 | SMS alerts when an order ships (~1,400 lines) | Exactly the manifesto's four rows, vendor row first |
| S2 | Rename a function used in 40 files | One row: rename, no behavior change, 40 call sites updated |
| S3 | `>` → `>=` in the discount check in `billing` | Boundary-condition change in a `payments` component, ranked first |
| S4 | Intent says "logging only" but adds a write to the payments table | Intent mismatch at the top of the report |
| S5 | Agent deletes an assertion / adds `.skip` while changing source | Test weakened; flagged as rules of the game |
| S6 | `billing` imports `notifications` internals | New boundary-rule violation |
| S7 | Adds a new npm dependency | Dependency row (supply-chain novelty) |
| S8 | Commits an API key | Security-sensitive row; `fail-on: secrets` fails the check |
| S9 | Pure formatting / moved file | Zero meaning-level rows (beyond "no semantic changes") |

**Non-functional targets:** a full report for a 2,000-line PR on a ~200k-line TS monorepo in under 30 seconds cold, and under 5 seconds with a cached base map (Rust and parallel parsing make this realistic). Output is byte-for-byte deterministic for the same inputs.

### 5.9 Milestones

| M | Deliverable | Done when |
|---|---|---|
| M0 | Public repo and Cargo workspace (rustfmt, clippy, insta, cargo-deny, CI on Linux/macOS/Windows); `onus-core` types + generated JSON Schemas; `fixtures/shop` with scenario scripts; community files (section 7a) | CI green on all three platforms; `onus --version` runs |
| M1 | `onus-lang-ts` adapter: components, symbols, shapes, fingerprints, imports, calls, tests → `onus map build --json` | Golden map snapshot of `fixtures/shop` is reviewed and correct |
| M2 | Declared layer: `onus.yaml` parsing + validation, inference (workspaces, CODEOWNERS), `onus init` | Inferred config for the fixture matches the hand-written one |
| M3 | Diff engine core: symbol matching, rename/move, contract diff, edge diff, packages, config, blast radius | S2, S7 and S9 pass |
| M4 | Extractors and detectors: externals registry, events, Prisma, env; notable edits; test weakening; secrets | S1 (minus rendering), S3, S5 and S8 pass |
| M5 | Ranking, internal collapsing, intent check, boundary rules, markdown + JSON renderers → `onus report` | All of S1–S9 pass as report snapshots |
| M6 | Release binaries via `cargo-dist` (Linux, macOS, Windows; x86_64 and arm64), Homebrew tap and shell installer; composite GitHub Action, sticky comment, base-map cache, `/onus caught`, metrics JSONL; dogfood on this repo | Running on Onus's own PRs |
| M7 | Field trial on 3–5 real TS repos (including agent-authored PRs); tune false positives; performance pass | Gate data collected (5.10) |

### 5.10 Phase 1 gate and metrics

The manifesto's gate is **"reviewers prefer the report to the raw diff on large pull requests."** From day one we measure:

- Review time for large PRs (≥500 changed lines), before and after: time from open to first review and time to merge, from the GitHub API.
- **Catch rate:** the share of reports where a reviewer says the report surfaced something the diff hid (`/onus caught`, 👍/👎 reactions).
- Compression: changed lines per report row.
- Accuracy: on sampled reports, false rows (wrong facts) and missed rows (meaning changes a human finds that the report omitted). Wrong facts are the critical metric.

**Proposed bar to move to Phase 2:** at least 70% 👍 on large PRs, at least 10% catch rate, and no false facts in sampled reports after tuning. The thresholds are yours to set.

### 5.11 Explicitly out of scope for Phase 1

Runtime traces, running tests, LLM-written report rows, GitHub App hosting, languages other than TS/JS, a map query service, tokens and lanes.

---

## 6. Phases 2–5 (architecture level)

### Phase 2: The map as a service

- **Store:** SQLite (`rusqlite`) behind the same map interface, with incremental rebuilds keyed by file content hash (only changed files and their dependents are re-analyzed). Built per commit, with the PR head map built in CI.
- **Queries** (CLI `onus map …` and MCP tools on one shared implementation):
  - `dependents(id, depth)`, `dependencies(id)`
  - `tests_for(symbol|path)`, `owners(component)`, `invariants(component|contract)`
  - `impact(symbol, hypotheticalChange)`: "if I change this return type, what breaks?"
  - `find(query)`: lexical search over symbol names and doc comments ("is there already a helper that formats phone numbers?"). Embeddings are optional, later.
  - `check(diff|worktree)`: "the codebase talks back". It runs boundary rules and contract checks *before* submission.
- **MCP server:** `onus mcp`, built on the official Rust MCP SDK (`rmcp`), with stdio for local agents and streamable HTTP for remote ones. Read-only in this phase. Same binary as the CLI.
- **Agent-agnostic validation:** run the same tasks with at least two different agents (e.g. Claude Code and Codex) and verify identical tool behavior.
- **Gate:** a benchmark of question/answer pairs over the fixture plus real repos, with ground truth from TS "find all references" and from people. It measures precision and recall per query type.

### Phase 3: Scoped tokens and evidence-based escalation

- **Token service:** mints Biscuit tokens from a task plan. The plan's scope suggestion comes from map queries: generous reads in low-sensitivity code, tight writes everywhere.
  - Path scopes first: `write:path:services/notifications/**`, `read:path:services/orders/events/**`
  - Network scopes: `net:host:sms.test.example`
  - Secret scopes: `secret:SMS_TEST_KEY`
  - TTL equals the task's lifetime
  - Holders can attenuate a token (hand a sub-agent a narrower one), never widen it
- **Doors that enforce:**
  - *Git gateway:* a smart-HTTP proxy that holds the real forge credential. It checks every pushed commit's changed paths against the token's write scope, so the agent never holds a GitHub token.
  - *Read enforcement:* the agent's workspace is materialized as a sparse checkout containing only the readable paths. A file the agent can't see can't be read and can't leak.
  - *MCP servers:* filter map results by read scope.
- **Escalation (`escalate`):** a structured request with these fields:
  - kind: permission, broken test, contradictory spec or impossible task
  - requested scope
  - graded evidence: failing test, then trace, then map path, then draft diff, then rationale
  - blast radius computed from the map

  Low-risk requests with grade-1 evidence are granted automatically; sensitive ones go to a person. Grants are narrow, last for the rest of the task, and are logged. This needs a **minimal test runner** (a container per request) to reproduce failing tests. Phase 5 generalizes it.
- **Errors that teach.** For example: "Denied: `billing/invoices` is outside your scope. Request `write:path:billing/invoices/**` with a failing test."
- **Audit log:** an append-only, hash-chained JSONL log of every door call (who, what was asked, what was returned, why). This is the start of the provenance record.
- **Gate:** escalation rate per task and time-to-grant stay low enough not to stall work, and red-team runs (prompt-injected issues, Invariant-style) land **zero** out-of-scope writes.

### Phase 4: Risk lanes and the verifying judge

- **Change application (`change.submit`):** contains the intent, the semantic report, the evidence, the scope used, escalations granted, and the agent setup (tool + model + config + team).
- **Classifier, rules first:** a declarative policy in `onus.yaml` decides auto-merge, judge, human or blocked.
  - Hard floors: anything labeled `auth`, `payments` or `pii` goes to a human. Writes out of scope, secrets, and tests weakened without approval are blocked.
  - **Rules of the game always go to a person:** test assertions, CI, policies, risk labels, judge configuration and declared contracts.
  - Later, a learned model can only move a change *up* a lane.
  - New agent setups start in stricter lanes.
- **Judge pipeline** (verify first, taste last):
  1. Re-run the evidence in a fresh environment.
  2. Compare intent to effect (semantic report vs. the stated intent).
  3. Check contracts and rules against the map.
  4. Detect weakened tests.
  5. Run held-out checks the author never saw: human-owned hidden suites outside the agent's read scope, plus combination tests the map points to wherever components interact.
  6. Assess taste with an LLM from a different vendor than the author, at the lowest weight.

  The judge sees the contracts, the map and fresh evidence, never the author's reasoning. Its verdict is approve, reject (with structured reasons fed back to the agent), or escalate to a person.
- **GitHub integration:** lanes become labels and required checks; the auto-merge lane uses GitHub's auto-merge.
- **Goodhart defenses:** hidden checks, random human audits of auto-merged changes, rotating judge configurations, and per-judge and per-agent outcome tracking.
- **Gate:** sampled human audits of auto-approved changes estimate a miss rate below a threshold you set. False negatives are weighted far above false positives. There is also a budget metric: the share of changes that land in the human lane.

### Phase 5: Evidence factory, production loop, contract-level scopes

- **Disposable environments** (`env.create/run/destroy`):
  - Containers first, microVMs (Firecracker) where isolation demands it.
  - Declared dependencies (devcontainer/Nix), synthetic seed data, warm pools and snapshots.
  - Network egress and secrets come from the token. Every reachable service is treated as part of the sandbox.
  - Automatic capture of JUnit results, OpenTelemetry traces and logs into a content-addressed evidence store.
- **Production loop:** ingest flag and rollout outcomes, reverts and incidents. These retrain the classifier, update agent-setup track records and generate new held-out tests.
- **Traces feed the map.** OTel spans become `traced` edges, which catch DI, reflection and calls between services that static analysis misses.
- **Contract-level scopes:** `write:component:notifications.internal`, `read:contract:UserPreferences`, `escalate-before:contract:*`.

---

## 7. Repository layout

One public repository, `onushq/onus`. Only Phase 1 crates are created at first; the rest are added when their phase starts.

```
onus/
  Cargo.toml  rust-toolchain.toml  deny.toml  onus.yaml (dogfood)
  README.md  LICENSE  NOTICE  CONTRIBUTING.md  CODE_OF_CONDUCT.md  SECURITY.md  GOVERNANCE.md
  PLAN.md
  crates/
    onus-core/       # map, change and report types; JSON Schema generation; ids; ranking
    onus-lang-ts/    # tree-sitter TypeScript/JavaScript adapter + extractors
    onus-map/        # workspace discovery, adapters + declared layer -> CodebaseMap; caching
    onus-diff/       # semantic diff, classification, rules, intent check
    onus-report/     # markdown and JSON renderers
    onus-cli/        # the `onus` binary (map | report | init | ...)
    # Phase 2+: onus-mcp/  onus-store/   Phase 3+: onus-tokens/ onus-gateway/
    # Phase 4+: onus-classifier/ onus-judge/   Phase 5+: onus-env/ onus-outcomes/
  action/            # composite GitHub Action (action.yml)
  schemas/           # generated JSON Schemas, committed so other tools can use them
  fixtures/
    shop/            # manifesto example monorepo + scenario scripts S1-S9
  docs/
    adr/             # one short record per significant decision (D1-D8 first)
  .github/           # CI, release, issue and pull request templates, dependabot
```

## 7a. Open source setup

Onus is open source from its first commit. Setting it up is part of M0.

**The `onushq` organization**
- Public profile README (`onushq/.github` repository, `profile/README.md`) with what Onus is, the status, and links to the site and the repo.
- Default community health files in `onushq/.github` (code of conduct, contributing guide, security policy, support), inherited by every repo.
- Require two-factor authentication for members; base member permission set to read; only owners create public repositories.
- Verified domain `onushq.com` (done).

**The `onushq/onus` repository**
- Public, Apache-2.0 `LICENSE` and a `NOTICE` file; description, topics (`ai-agents`, `code-review`, `semantic-diff`, `static-analysis`, `rust`, `mcp`, `devtools`) and the website link.
- `README.md`: the one-sentence idea, a picture of a semantic change report, status (Phase 1, pre-alpha), install and quick start, links to `PLAN.md` and the manifesto.
- `CONTRIBUTING.md` (build, test, snapshot workflow, DCO sign-off), `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1), `SECURITY.md` (private vulnerability reporting through GitHub), `GOVERNANCE.md` (maintainers, how decisions and ADRs are made).
- Issue forms (bug, feature, new language adapter), a pull request template, labels (`phase-1`, `good first issue`, `help wanted`, `lang:typescript`, `area:diff`, `area:map`, `area:cli`), and Discussions for questions and design.
- `main` protected: pull requests only, CI must pass (fmt, clippy, tests on three platforms, cargo-deny, DCO check), linear history, squash merges, branches deleted after merge.
- Dependabot for Cargo and GitHub Actions; GitHub private vulnerability reporting and secret scanning on.
- Releases: tagged `vX.Y.Z`, built by `cargo-dist`, with checksums and build provenance attestations; `CHANGELOG.md` kept by hand or with `git-cliff`.
- Crate names `onus-*` on crates.io are claimed when the first release is cut.
- The onushq.com website stays a separate, private repository.

## 8. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Report states a wrong fact and loses reviewer trust immediately | Deterministic templates only; show confidence; golden scenarios; field-trial sampling with "false facts = 0" as part of the gate |
| Event, ORM and DI wiring isn't visible statically | Pattern extractors configurable in `onus.yaml`; unknowns raise risk; traces in Phase 5 |
| Contract compatibility is hard to prove | Conservative classification (`contract-changed-unverified` → treated as breaking) |
| Big monorepos are slow | Cache the base map; incremental analysis in Phase 2; perf budget in CI |
| Scope creep: building Phases 3–5 before the wedge proves itself | Hard rule: no Phase N+1 code until Gate N is evaluated |
| Fixture overfits the manifesto example | M7 field trial on real repos and real agent PRs |

---

## 9. Manifesto open questions: where they get answered

| Open question | Addressed in |
|---|---|
| Right size for a contract (function, module, service)? | Phase 1 uses public symbols per component; Phase 5 contract scopes test it in practice |
| Standard format for semantic changes? | `core` JSON Schema, published from Phase 1 |
| Measuring judge quality without full re-review? | Phase 4 audit sampling + outcome tracking |
| Map across many repos/teams? | After Phase 2: map federation (each repo publishes its map; contracts cross-reference by id) |
| Cost of hundreds of environments per day? | Phase 5 warm pools and caching; measured per task |
| Keeping human skills sharp? | Process, not code: rotation guidance in docs; reports that explain themselves |
| Abuse of the escalation channel? | Phase 3: authenticated, signed escalations; evidence independently re-verified |

---

## 10. Next steps

1. **Open decisions (project lead):** set the Phase 1 gate thresholds (5.10), and decide whether the manifesto PDF goes into the public repository. D1, D7 and D8 are decided; D2–D6 stand as recommended unless you change them.
2. **Then M0:** create the public `onushq/onus` repository and the org setup in section 7a, scaffold the Cargo workspace and CI, write the `onus-core` types, and build `fixtures/shop` with scenario S1 first, since it is the manifesto's headline example.
