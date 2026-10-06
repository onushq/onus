# Kinds of change

Every row has a **kind** (one of seven), a **subkind** (the specific change) and a **level** (relationship, behavior or structure). The Kind column of the Markdown report shows a short label; the JSON has `kind` and `subkind`.

| Kind | Meaning |
|---|---|
| security-sensitive | Data leaves the system in a new way, a secret was committed, or code in a sensitive component changed how it decides or writes. Always needs a person. |
| breaking | Callers or consumers may break: a contract lost something or changed in a way Onus cannot prove compatible, or a boundary rule was broken. Always needs a person. |
| dependency | Third-party code changed: a new, upgraded or removed npm package. |
| config | Configuration changed. CI, policies, onus.yaml and CODEOWNERS are also rules of the game. |
| additive | Something new that existing callers do not notice: an optional field, a new export, a new event consumer, a new read. |
| test | Tests were weakened (rules of the game). |
| internal | Everything else, collapsed into one row per component, plus renames and notable edits outside sensitive components. |

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
| export-added | additive | A symbol becomes public. Several new exports of one component are one row. |
| contract-field-added-required | breaking | A required field is added to an interface or type. |
| contract-field-removed | breaking | A field, member or method is removed. |
| contract-field-now-required | breaking | An optional field becomes required. |
| contract-field-now-optional | breaking | A required field becomes optional: readers may now get `undefined`. |
| contract-field-now-readonly | breaking | A field becomes readonly. |
| contract-param-added-required | breaking | A required parameter is added. |
| contract-param-removed | breaking | A parameter is removed. |
| contract-param-now-required | breaking | An optional parameter becomes required. |
| export-removed | breaking | A public symbol is removed or no longer exported. |
| contract-changed-unverified | breaking | A type changed in a way Onus cannot prove compatible (a parameter, return or field type, type parameters), or a public symbol with no type annotation changed its body, so its inferred type may have changed (low confidence). |

Renaming a parameter is not a contract change. When a contract declared in onus.yaml has invariants, the row repeats them.

## Renames and moves

| Subkind | Kind | When |
|---|---|---|
| rename | internal | A symbol is renamed with an identical body. The row counts the call sites and files updated. |
| rename-incomplete | breaking | The same, but some references still use the old name. |
| moved-between-components | internal or breaking | Symbols moved from one component to another: one row per pair of components ("17 symbols moved from `meeting-abstractions` to `infrastructure-gemini-live`"). Symbols edited while moving (same name and kind, most members in common) are included, and their contract changes are listed in the row; breaking when any of those changes is breaking. The row also says when the old component no longer exports them. |
| moved-incomplete | breaking | The same, but some references still point at the old place. |

A symbol or file that moved inside one component without other changes, and a file whose only changes are formatting, comments or import paths, is not a row: it is listed under "Structure only".

## Dependencies

| Subkind | Kind | When |
|---|---|---|
| new-dependency | dependency | A package.json gains a third-party package. Novelty `new-package:<name>` when no other component used it. |
| dependency-version-changed | dependency | A declared version range changed. |
| dependency-removed | dependency | A package is removed. |
| lockfile-changed | dependency | A lockfile changed while no manifest dependency did. |

Workspace packages (your own `@scope/...` packages) are not third-party: depending on one shows as a relationship between components.

## Configuration and rules of the game

| Subkind | Kind | Rules of the game | Files |
|---|---|---|---|
| ci-changed | config | yes | .github/workflows/*, .gitlab-ci.yml, .circleci/, .buildkite/, Jenkinsfile |
| onus-config-changed | config | yes | onus.yaml (the report used the base version) |
| codeowners-changed | config | yes | CODEOWNERS |
| policy-changed | config | yes | *.rego, policy/, policies/ |
| test-config-changed | config | yes | jest, vitest, playwright, cypress, karma and mocha configs |
| repo-settings-changed | config | yes | anything else under .github/ |
| env-changed | config | no | .env* |
| container-changed | config | no | Dockerfile*, docker-compose*, compose.* |
| data-schema-changed | config | no | *.prisma |
| build-config-changed | config | no | tsconfig*.json |
| package-scripts-changed | config | no | package.json `scripts` |
| package-manifest-changed | config | no | other package.json fields (dependencies are reported as packages) |
| config-changed | config | no | other .yaml, .yml, .json, .toml, .ini and .properties files |

## Tests

| Subkind | Kind | When |
|---|---|---|
| test-weakened | test | One row per component when any of these happen: a test case is removed; a case has fewer assertions; a case gains `skip`, `only` or `todo`; expected values in assertions are edited; a test file is deleted; or source code starts comparing against a string literal that only the tests use. Rules of the game. |

New test cases are not a row; the component's internal row counts them.

## Notable edits inside functions

Found in functions and methods that exist on both sides. Security-sensitive in a sensitive component, internal otherwise.

| Subkind | When |
|---|---|
| boundary-condition-changed | A comparison flips with the same operands: `a > b` becomes `a >= b`. |
| condition-constant-changed | A comparison keeps its operator but compares against a different constant. |
| constant-changed | A `const` or variable initialized with a literal changes value: `LOYALTY_RATE` from 0.1 to 0.15. |
| guard-removed | A function has fewer early exits (`if (...) return/throw`) or fewer `throw`s. An edited guard is not a removed guard. |
| await-removed | A function awaits less, and a specific await disappeared. |
| error-swallowed | A new empty `catch` block. |

## Secrets

| Subkind | Kind | When |
|---|---|---|
| secret-committed | security-sensitive | An added line matches a high-confidence pattern: AWS access key ID or secret access key, GitHub token, Stripe secret key, Slack token, Google API key, SendGrid API key, npm token, or a private key block. One row per file. The value is never printed, and it is redacted from the map too. |

## Everything else

| Subkind | Kind | When |
|---|---|---|
| internal-changes | internal | The changed lines of a component that no other row explains, with their count, labeled as code, tests or docs. |

## Grouped rows

To keep large changes readable, Onus merges rows that say the same thing:

- Several config files of one kind in one component are one row ("Build config: 3 files changed in `infrastructure-gemini-live`").
- Packages already used elsewhere in the repository, dropped packages and version changes are one row per component ("2 npm packages already used in this repository added in `c`"). A package new to the repository always keeps its own row.
- When the same kind of config or dependency change appears in more than 5 components, it is one row for all of them ("Build config: changes in 269 components (506 files)").
- When more than 10 components have internal changes, the 10 largest keep their rows and the rest are summarized in one ("1,490 changed lines inside 106 other components").
- Several new exports of one component are one row.

Grouped rows keep every file and line in their evidence.

## Hints on every row

- `labels`: sensitivity labels of the component.
- `blastRadius`: files in the new version (tests included) that depend on the subject, not counting the file that defines it.
- `novelty`: `new-vendor:<vendor>`, `new-data-egress:<kind>`, `new-dependency:<package>`, `new-package:<package>`, `secret`.
- `confidence`: `declared` (from onus.yaml), `compiler` (confirmed by a compiler, indexer or language server), `static` (read from the code), `inferred` (from conventions) or `low` (Onus could not resolve something).
- `rulesOfTheGame`, `intentMismatch`, `needsPerson`.
