# Kinds of change

Every row has a **kind** (one of seven), a **subkind** (the specific change) and a **level** (relationship, behavior or structure). The Kind column of the Markdown report shows a short label; the JSON has `kind` and `subkind`.

| Kind | Meaning |
|---|---|
| security-sensitive | Data leaves the system in a new way, a secret was committed, authentication or authorization code changed, a new HTTP route or public API operation appeared, or code in a sensitive component changed how it decides or writes. Always needs a person. |
| breaking | Callers or consumers outside this change may break: a contract lost something or changed in a way Onus cannot prove compatible and some user was not updated, an API operation or route was removed, or a boundary rule was broken. Always needs a person. |
| dependency | Third-party code changed: a new, upgraded or removed npm package, a lockfile, or a patched package (which needs a person). |
| config | Configuration changed. CI, policies, onus.yaml and CODEOWNERS are also rules of the game. Migrations, root build config and infrastructure need a person. |
| additive | Something new that existing callers do not notice: an optional field, a new export, a new event consumer, a new read; also a contract change whose users were all updated in the same change. |
| test | Tests were weakened (rules of the game), or removed together with the code they tested. |
| internal | Everything else, collapsed into one row per component, plus renames, generated code and notable edits outside sensitive components. |

"Sensitive component" means a component with a label of medium or high sensitivity in onus.yaml; `payments`, `auth` and `pii` are high unless onus.yaml says otherwise.

## Relationships between components, data and the outside world

These are compared per component, so moving code around inside a component never looks like a new relationship.

| Subkind | Kind | When |
|---|---|---|
| new-external-service | security-sensitive | A component starts calling an external service that is new to the repository, or that sends a kind of data (phone numbers, email addresses, payment details, ...) the component never sent before. Its novelty lists `new-vendor:<vendor>` and `new-data-egress:<kind>`. A new npm package that is the service's SDK is folded into this row (`new-dependency:<package>`). |
| new-external-call | additive | A component starts calling a service another component already uses, sending nothing new. |
| external-service-removed | internal | A component stops calling a service. |
| new-event-consumer | additive | A component starts subscribing to an event. The row says whether the publishers also changed; with no publisher in the map it is low confidence. |
| new-event-publisher | additive | A component starts publishing an event; the row names the components that consume it. |
| event-consumer-removed | breaking | A component stops subscribing to an event. |
| event-publisher-removed | breaking | A component stops publishing an event others still consume (internal when nobody consumes it). |
| new-data-write | security-sensitive or additive | A component starts writing a table (Prisma). Security-sensitive in a sensitive component. |
| new-data-read | additive | A component starts reading a table. |
| data-write-removed, data-read-removed | internal | A component stops writing or reading a table. |
| new-cross-component-dependency | additive | A component starts depending on another component's code. The row names what it uses, and says so when that is internal to the other component. |
| rule-violation | breaking | A declared boundary rule is broken by a new reference (see `onus help configuration`). Rules of the game. Not reported when the same violation existed before. |

## Contracts

Contracts are the public symbols of a component: everything reachable from its entrypoint through exports and re-exports. Shapes come from the declared types; a type that is inferred rather than written is "unverified".

| Subkind | Kind | When |
|---|---|---|
| contract-field-added-optional | additive | An optional field or member is added (`phoneVerified?: boolean`). |
| contract-member-added | additive | An enum member or a class method or property is added. |
| contract-param-added-optional | additive | An optional or rest parameter is added. |
| contract-param-now-optional | additive | A required parameter becomes optional. |
| contract-key-added | additive | A public value without a type annotation gains a key: its initializer is an object literal, or a call such as `Schema.struct({...})` or `z.object({...})`, and the object gains a key. |
| contract-type-widened | additive | A field, method or result type changes only by gaining optional parameters or optional fields (`(p: string) => R` becomes `(p: string, opts?: O) => R`); callers and implementations keep compiling. |
| contract-union-widened | additive | A union type accepts more members (`'A' \| 'B'` becomes `'A' \| 'B' \| 'C'`). |
| export-added | additive | A symbol becomes public. Several new exports of one component are one row. |
| contract-field-added-required | breaking | A required field or method is added to an interface or type, and an implementation outside this change was not updated. |
| contract-field-removed | breaking | A field, member or method is removed. |
| contract-field-now-required | breaking | An optional field becomes required. |
| contract-field-now-optional | breaking | A required field becomes optional: readers may now get `undefined`. |
| contract-field-now-readonly | breaking | A field becomes readonly. |
| contract-param-added-required | breaking | A required parameter is added. |
| contract-param-removed | breaking | A parameter is removed. |
| contract-param-now-required | breaking | An optional parameter becomes required. |
| export-removed | breaking | A public symbol is removed or no longer exported. |
| contract-key-removed | breaking | A public value without a type annotation loses a key of its initializer. |
| contract-union-narrowed | breaking | A union type no longer accepts some members. |
| contract-changed-unverified | breaking | A type changed in a way Onus cannot prove compatible: a parameter, return, field or key type, or type parameters. Types are compared with object members and union members sorted, so reordering (as generators do) is no change. |
| contract-inferred-changed | internal | Public symbols whose types are inferred, not written, changed their bodies, and Onus has nothing to compare. One row for all of them, low confidence. |

A contract change is breaking only when something outside the change can break: a file that uses the symbol and was not changed, or, for a new required member, an implementation that was not changed (code that only reads the type is unaffected). Breaking rows name those files, test files first. Implementations are found by how they are written: `implements X`, `satisfies X`, `: X = {...}`, a function declared to return `X`, and Effect's `Layer.succeed(X, ...)` and `X.of({...})`. A cast (`as X`) is not an implementation: it compiles whatever members `X` gains.

When every user was updated in the same change, or nothing outside it uses the symbol, the row keeps its subkind but is additive ("Contract change, users updated" or "no users affected") and does not need a person. It stays breaking when Onus cannot see every user: the package is published (its package.json is not private and has `files` or `publishConfig`), or some imports of it could not be resolved.

Renaming a parameter is not a contract change. When a contract declared in onus.yaml has invariants, the row repeats them.

## Renames and moves

| Subkind | Kind | When |
|---|---|---|
| rename | internal | A symbol is renamed with an identical body. The row counts the call sites and files updated. |
| rename-incomplete | breaking | The same, but some references still use the old name. |
| moved-between-components | internal or breaking | Symbols moved from one component to another: one row per pair of components ("17 symbols moved from `orders-core` to `billing-shared`"). Exported symbols edited while moving (same name and kind, most members in common) are included, and their contract changes are listed in the row; breaking when any of those changes is breaking. The row also says when the old component no longer exports them. |
| moved-incomplete | breaking | The same, but some references still point at the old place. |

A symbol or file that moved inside one component without other changes, and a file whose only changes are formatting, comments or import paths, is not a row: it is listed under "Structure only".

## Dependencies

| Subkind | Kind | When |
|---|---|---|
| new-dependency | dependency | A package.json gains a third-party package. Novelty `new-package:<name>` when no other component used it. A package new to the repository that several components add is one row, and new packages added to the same components share a row. |
| dependency-version-changed | dependency | A declared version range changed. |
| dependency-removed | dependency | A package is removed. |
| lockfile-changed | dependency | A lockfile changed while no manifest dependency did. The row lists the installed versions that change (read from pnpm-lock.yaml, package-lock.json and yarn.lock), or says that none do when only patch hashes, checksums or resolution changed. |
| dependency-patched | dependency | A patch of a third-party package is added, changed or removed (`patches/*.patch` for pnpm and patch-package, `.yarn/patches/`). The row names the package and the files the patch edits. Needs a person: installed code changes without a version change, and the patch stops applying when the version moves. |

Workspace packages (your own `@scope/...` packages) are not third-party: depending on one shows as a relationship between components.

| Subkind | Kind | When |
|---|---|---|
| external-api-first-use | internal | Production code uses an API of a package the repository already depends on (`Effect.retry` from `effect`) that no other code in the repository uses. The row names the version the repository pins, to check the API exists in it: code written from memory of another version often calls functions that are not there. Node's built-in modules and framework virtual modules (`$app/…`, `$env/…`) are left out, and an API counts as used when the repository already reaches it another way (`RawData` imported by name, `WebSocket.RawData` through the default import). |

## Risk classes

Some changes need a person whatever their size or shape, so they get their own row instead of disappearing into a component's changed lines. Onus finds them by fixed patterns in the changed files.

| Subkind | Kind | When |
|---|---|---|
| migration-changed | config | A database migration is added, edited or removed: versioned files in a `migrations`, `migration` or `migrate` folder, `*.migration.*`, Flyway `V1__name.sql`, Drizzle `drizzle/0001_name.sql`, Alembic versions, and versioned upgrade commands (`upgrade…/…command…<timestamp>…`). The row says what the migration does to stored data, read from its SQL and TypeORM calls (creates or drops tables, adds or drops columns, creates indexes, deletes or updates rows, grants), and mentions `ON DELETE CASCADE`. The `down` step is left out. Editing a migration that already ran is called out. Snapshots and journals the tool writes beside it (`drizzle/meta/`, Prisma's `migration_lock.toml`) are part of the migration, and other code in a migrations folder gets a row only when it touches stored data. |
| public-api-changed | security-sensitive, additive or breaking | GraphQL queries, mutations or subscriptions are added, removed or change their signature, in schema files (`.graphql`, `.gql`), `gql` templates or code-first resolvers (`@Query`, `@Mutation`, `@Subscription`). Operations that skip authentication (`@skipAuth`, `@Public()` and similar) get their own security-sensitive row. Always needs a person. |
| route-added | security-sensitive | New HTTP routes: SvelteKit `+server` files, Next.js `route` files and `pages/api`, and controller decorators (`@Controller('users')` with `@Get(':id')`). The row names the methods and paths, and the `cache-control` header the route sets. |
| route-removed | breaking | HTTP routes removed. |
| auth-code-changed | security-sensitive | Code changed in files whose names say they decide who may do what (`auth`, `permission`, `guard`, `policy`, `role`, `acl`, `jwt`, `oauth`, `password`, …, also in camelCase names). One row per component, even for a refactor. |

## Configuration and rules of the game

| Subkind | Kind | Rules of the game | Files |
|---|---|---|---|
| ci-changed | config | yes | .github/workflows/*, .gitlab-ci.yml, .circleci/, .buildkite/, Jenkinsfile |
| onus-config-changed | config | yes | onus.yaml (the report used the base version) |
| onus-pack-changed | config | yes | a framework pack listed in onus.yaml (the report used the base version) |
| codeowners-changed | config | yes | CODEOWNERS |
| policy-changed | config | yes | *.rego, policy/, policies/ |
| test-config-changed | config | yes | jest, vitest, playwright, cypress, karma and mocha configs |
| repo-settings-changed | config | yes | anything else under .github/ |
| root-build-config-changed | config | needs a person | at the repository root: tsconfig*.json, babel config, nx.json, turbo.json, .npmrc, .nvmrc, .node-version, .tool-versions |
| infrastructure-changed | config | needs a person | Terraform (.tf, .tfvars, .hcl), Kubernetes manifests (YAML with `apiVersion` and `kind`, or YAML in k8s, helm, charts, deploy, manifests, infra folders), Chart.yaml, serverless, fly, vercel, netlify, render, wrangler, cdk, Pulumi and a root app.yaml |
| env-changed | config | no | .env* |
| container-changed | config | no | Dockerfile*, docker-compose*, compose.* |
| data-schema-changed | config | no | *.prisma |
| workspace-config-changed | config | no | pnpm-workspace.yaml, .yarnrc.yml, lerna.json |
| build-config-changed | config | no | tsconfig*.json, project.json, angular.json, ESLint, Prettier, Biome and SWC configs, and bundler, framework and codegen configs (vite, webpack, rollup, tsup, esbuild, svelte, next, nuxt, astro, babel, postcss, tailwind, codegen, …) |
| package-scripts-changed | config | no | package.json `scripts` |
| package-manifest-changed | config | no | other package.json fields (dependencies are reported as packages) |
| config-changed | config | no | other .yaml, .yml, .json, .toml, .ini and .properties files |

Rows for a changed config file name what changed: the top-level keys of JSON and YAML (with `compilerOptions` broken down in tsconfig files), and the instructions of a Dockerfile (`ENV BODY_SIZE_LIMIT`, the base image).

## Tests

| Subkind | Kind | When |
|---|---|---|
| test-weakened | test | One row per component when any of these happen: a test case is removed; a case has fewer assertions; a case gains `skip`, `only` or `todo`; expected values in assertions are edited; a test file is deleted; or source code starts comparing against a string literal that only the tests use. Rules of the game. |
| tests-removed-with-code | test | Removed test cases or assertions that used code this change deletes. They covered nothing that still exists, so they do not weaken the tests and do not need a person. |

A removed case that reappears among the change's new cases (the same body, or the same title in a file of the same name, as when tests move with their code) is neither. New test cases are not a row; the component's internal row counts them.

## Notable edits inside functions

Found in functions and methods that exist on both sides. Security-sensitive in a sensitive component, internal otherwise. An await, guard or throw that the change adds to another function, as when a body is extracted into a helper, moved rather than disappeared, and is not reported.

| Subkind | When |
|---|---|
| boundary-condition-changed | A comparison flips with the same operands: `a > b` becomes `a >= b`. |
| condition-constant-changed | A comparison keeps its operator but compares against a different constant. |
| constant-changed | A `const` or variable initialized with a literal changes value: `LOYALTY_RATE` from 0.1 to 0.15. |
| guard-removed | A function has fewer early exits (`if (...) return/throw`) or fewer `throw`s. An edited guard is not a removed guard. |
| await-removed | A function awaits less, and a specific await disappeared. Awaiting calls together (`await Promise.all([...])`) is not a removal. |
| error-swallowed | A new empty `catch` block. |

## Secrets

| Subkind | Kind | When |
|---|---|---|
| secret-committed | security-sensitive | An added line matches a high-confidence pattern: AWS access key ID or secret access key, GitHub token, Stripe secret key, Slack token, Google API key, SendGrid API key, npm token, or a private key block. One row per file. The value is never printed, and it is redacted from the map too. |

## Everything else

| Subkind | Kind | When |
|---|---|---|
| internal-changes | internal | The changed lines of a component that no other row explains, with their count, labeled as code, tests or docs. In components with more than 300 files, the row also names the module folders that changed. |
| generated-code-changed | internal | Files written by a tool changed: found by folder (`generated`, `__generated__`, `codegen`), name (`.generated.`, `.gen.`, `_pb.`) or a marker in the first lines (`@generated`, `DO NOT EDIT`, "This file was generated"). One row per component that lists what the other rows would have said; review the schema or generator change it mirrors. |

## Grouped rows

To keep large changes readable, Onus merges rows that say the same thing:

- Several config files of one kind in one component are one row per kind of edit ("Build config: 3 files in `billing` change `module`").
- Packages already used elsewhere in the repository, dropped packages and version changes are one row per component ("2 npm packages already used in this repository added in `c`"). A package new to the repository always keeps its own row.
- When the same kind of config or dependency change appears in more than 5 components, it is one row for all of them, and config rows stay apart by the keys they change ("Build config changes `module` in 262 components (445 files)").
- When more than 10 components have internal changes, the 10 largest keep their rows and the rest are summarized in one ("1,490 changed lines inside 106 other components").
- Several new exports of one component are one row.

Grouped rows keep every file and line in their evidence.

## Hints on every row

- `labels`: sensitivity labels of the component.
- `blastRadius`: files in the new version (tests included) that depend on the subject, not counting the file that defines it.
- `novelty`: `new-vendor:<vendor>`, `new-data-egress:<kind>`, `new-dependency:<package>`, `new-package:<package>`, `secret`.
- `confidence`: `declared` (from onus.yaml), `compiler` (confirmed by a compiler, indexer or language server), `static` (read from the code), `inferred` (from conventions) or `low` (Onus could not resolve something).
- `rulesOfTheGame`, `intentMismatch`, `needsPerson`.
