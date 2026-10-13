# Configuration: onus.yaml

Onus works without configuration. An `onus.yaml` at the repository root adds what Onus cannot infer: which components are sensitive, which boundaries must hold, how your events are published, and SDKs it does not know. `onus init` writes a starter file; the format is in schemas/onus-config.schema.json. `onus ui` edits it with a form: every edit is checked before it is saved, a save never overwrites a change made to the file meanwhile, and the map is rebuilt with the new config at once (as it is for any edit to the file: the map server follows onus.yaml like any other file).

## A complete example

    version: 1
    components:
      orders:           { path: "services/orders/**",           owners: ["@team-orders"] }
      notifications:    { path: "services/notifications/**",    owners: ["@team-growth"] }
      user-preferences: { path: "services/user-preferences/**", labels: [pii] }
      billing:          { path: "services/billing/**",          labels: [payments] }
    contracts:
      UserPreferences:
        symbol: "user-preferences:src/types.ts#UserPreferences"
        invariants: ["phone numbers are stored in E.164"]
    rules:
      - deny: { from: billing, to: notifications.internal }
    labels:
      payments: { sensitivity: high }
      pii:      { sensitivity: high }
    extractors:
      events:
        publish:   ["bus.publish($EVENT, ...)"]
        subscribe: ["bus.subscribe($EVENT, ...)"]
      externals:
        "@acme/sms": { vendor: "Acme SMS", category: sms, egress: [phone] }
      prisma:
        clients: [prisma, db]
    tests:
      globs: ["**/*.test.ts", "e2e/**"]
      command: "npx vitest run {test}"
      coverage: ["coverage/lcov.info"]
    testData: ["fixtures/**"]

Unknown keys are an error, so a typo never silently does nothing.

## Which onus.yaml a report uses

`onus diff` and `onus report` build both versions with the **base** version's onus.yaml (or `--config`). A pull request therefore cannot relax the rules it is checked against; if it changes onus.yaml, that change is itself a row needing a person.

## version

Must be `1`.

## components

A map from component id to its settings. Ids may not be empty or contain `:`, `#`, `/` or spaces.

- `path` (required): a glob, or a list of globs, of the component's files, such as `"services/orders/**"`. When several components match a file, the most specific one wins.
- `kind`: `service`, `package` or `module`. Default: `service` under services/ or apps/, `package` under packages/ or libs/, otherwise `module`.
- `owners`: default: the last matching rule in CODEOWNERS (looked up in .github/, the root, then docs/).
- `labels`: sensitivity labels, such as `payments`, `auth`, `pii` or your own.
- `entrypoints`: source files that define the public surface, relative to the component folder. Default: from package.json `exports`, `types`, `module` or `main`, mapped from build output back to source (`dist/index.js` → `src/index.ts`), else `src/index.*` or `index.*`.

Without a `components` section, Onus finds components itself:

- Cargo packages (`Cargo.toml` with a `[package]`) and Python projects (`pyproject.toml` with a project name) are components named after the package.
- In an Nx repository (an `nx.json` at the root), every folder with a `project.json` is a component, named after the project. `projectType: application` is a service and `library` a package.
- npm, pnpm or yarn workspace packages are components too; where a workspace package contains Nx projects, each file belongs to the most specific one.
- Failing both, every folder under services/, packages/ and apps/.

Files outside every component belong to `root`.

Only symbols reachable from an entrypoint (through exports and re-exports) are public contracts. A component with no entrypoint has no public surface; for libraries Onus notes it in the map, while applications are expected to have none.

## contracts

Names important contracts and the invariants people rely on. `symbol` is a map id, `<component>:<path inside the component>#<name>`; list all ids with `onus map . --json` and look under `symbols`. The invariants are repeated on any row that changes the contract. An id that matches nothing is noted in the map.

## rules

Boundary rules, evaluated on the new version of the code. Only new violations are reported.

    rules:
      - deny: { from: billing, to: notifications }            # billing may not use notifications at all
      - deny: { from: billing, to: notifications.internal }   # only its public surface

A reference is an import, a call or a type reference. When onus.yaml declares components, both sides must be among them, and `from` and `to` must differ. Use `--fail-on rule-violation` to fail CI on a new violation.

## labels

The sensitivity of each label: `low`, `medium` or `high`. Labels of medium or high sensitivity make a component sensitive: new data writes and notable edits there become security-sensitive. `payments`, `auth` and `pii` are high by default; set one to `low` to turn that off.

## extractors.events

Call patterns that publish and subscribe to events. `$EVENT` marks the argument holding the event name; `...` stands for the rest.

    events:
      publish:   ["bus.publish($EVENT, ...)", "emitter.emit($EVENT, ...)"]
      subscribe: ["bus.subscribe($EVENT, ...)", "@OnEvent($EVENT)"]

A pattern matches the call itself and any call that ends with it, so `bus.publish` also matches `this.bus.publish` and `app.bus.publish`. Patterns starting with `@` match decorators. Declaring patterns replaces the defaults:

    publish:   bus.publish($EVENT, ...), eventBus.publish($EVENT, ...), events.publish($EVENT, ...)
    subscribe: bus.subscribe($EVENT, ...), eventBus.subscribe($EVENT, ...), events.subscribe($EVENT, ...), @OnEvent($EVENT)

Event names are read from string literals, constants (also imported ones) and enum members. A name computed at runtime is recorded with low confidence and noted in the map.

## extractors.externals

External services, keyed by the npm package of their SDK. They extend, and for the same package override, the built-in registry.

    externals:
      "@acme/sms": { vendor: "Acme SMS", category: sms, egress: [phone], hosts: [api.acme-sms.example] }

- `vendor`: the name shown in reports.
- `category`: `sms`, `email`, `payments`, `ai`, `analytics`, `monitoring`, `storage`, `chat`, `http` or your own.
- `egress`: kinds of data that leave the system through it: `phone`, `email`, `payment`, `bank-account`, `pii`, `text`, `files`, `errors` or your own. A component sending a new kind is security-sensitive.
- `hosts`: hostnames, so `fetch("https://api.acme-sms.example/...")` and `axios` calls count as calls to this vendor. Calls to other literal hosts are reported under the host name.

Built-in registry:

| Package | Vendor | Category | Egress |
|---|---|---|---|
| twilio | Twilio | sms | phone |
| stripe | Stripe | payments | payment |
| @sendgrid/mail, @sendgrid/client | SendGrid | email | email |
| nodemailer | SMTP email | email | email |
| mailgun.js | Mailgun | email | email |
| postmark | Postmark | email | email |
| @aws-sdk/client-ses | AWS SES | email | email |
| @aws-sdk/client-sns | AWS SNS | sms | phone |
| @aws-sdk/client-s3 | AWS S3 | storage | files |
| @slack/web-api | Slack | chat | text |
| openai | OpenAI | ai | text |
| @anthropic-ai/sdk | Anthropic | ai | text |
| @segment/analytics-node | Segment | analytics | pii |
| posthog-node | PostHog | analytics | pii |
| @sentry/node | Sentry | monitoring | errors |
| plaid | Plaid | payments | bank-account |
| braintree | Braintree | payments | payment |

An SDK counts as called when a function calls something imported from it, including through a class field declared with the SDK's type (`private readonly client: AcmeSmsClient` and then `this.client.send(...)`).

## extractors.packs

Framework packs: YAML files of tree-sitter queries that say what a pattern in the code means (routes, events, data access, other relationships), relative to onus.yaml. See `onus help plugins`.

    extractors:
      packs: [onus/packs/nestjs.yaml]

Packs are read from the base version of a change, like this file.

## extractors.prisma

`clients`: identifiers that hold a Prisma client (default: `prisma`). Calls such as `prisma.payment.update(...)` become writes to `payment`; `findMany`, `count` and the other read operations become reads. Raw queries (`$queryRaw`, `$executeRaw`) are noted in the map, since their tables are unknown.

## tests

`globs`: which files are tests. Declaring globs replaces the defaults:

    **/*.test.*  **/*.spec.*  **/__tests__/**  **/test/**  **/tests/**  **/e2e/**

`command`: how to run one test file, with `{test}` replaced by its path, such as `npx vitest run {test}`. `onus escalation assess` uses it to reproduce the failing test an escalation names, in a container built from `environment:` (`onus help scopes`). Without it, failing tests are not reproduced, so no escalation is granted by policy. A command without `{test}` would run every test, so it reproduces nothing either.

`coverage`: coverage files of the working tree (lcov, Cobertura XML or Istanbul JSON), relative to the repository root, for the map agents query (`onus mcp`, `onus query`). Each rebuild reads the ones that exist, and leaves out any source file edited after the coverage file was written, with a note. With them, `onus_tests_for` says which tests ran a target, and finds tests that only reach it through other code (`onus help agents`). Reports read coverage only from `--coverage` and `--evidence` (`onus help environments`).

## testData

Globs of files that are test data rather than code: fixture projects, sample apps, recorded responses.

    testData:
      - fixtures/**
      - e2e/recordings/**

Test data is left out of the map, so a fixture app's imports, services and contracts do not show up as your code. Changes to it are summarized in one "changed lines of test data" row, and config files inside it (a fixture's package.json or tsconfig.json) are not reported as config changes.

Secret-shaped values added in test data are not counted as committed secrets: `fail-on: secrets` does not trip on them, and they appear as one row, "... added in test data", that names the files without the values. Declare only folders that hold fake credentials on purpose. As with every setting, the report uses the base commit's onus.yaml, so a pull request cannot declare its own new secret as test data.

## Files Onus never reads

Folders named .git, .onus (where Onus keeps its records), node_modules, dist, build, out, .next, .turbo, .cache, coverage, target or vendor; anything ignored by a .gitignore inside the tree; files over 2 MB; and *.min.js files.

## lanes

What happens to a change after it is reported: the default lane, rules that set or raise it, the built-in risk inputs, a learned model that may raise it, the shares of automatic merges a person audits before and after merging, the record an agent setup needs before it may auto-merge, held-out checks and an optional taste reviewer for the judge, and how each lane's changes meet production traffic. See `onus help lanes` and `onus help production`.

    lanes:
      default: judge          # auto-merge, judge, human or blocked
      auditRate: 0.05         # would-be auto-merges sent to a person before merging
      auditAfterMerge: 0.05   # changes approved without a person, audited after they merge
      minRecord: 10
      rules:
        - { lane: auto-merge, match: every, kinds: [internal], components: [docs] }
        - { lane: human, subkinds: [migration-changed] }
      heldOut: { command: "npm run test:held-out", image: "node:22", setup: "npm ci" }
      inputs:                                  # each key optional; these are the defaults
        blastRadius: { judge: 3 }              # other components that depend on a row
        tests: { untested: judge }             # code no test or trace reaches
        confidence: { low: judge }             # rows on parts of the map Onus could not resolve
        history: { days: 90, reverts: { judge: 1 }, incidents: { human: 1 } }
        coverage:                              # off unless written
          minChanged: 0.8
          required: false
          commands: ["npm test -- --coverage"] # whose recorded coverage counts
      learned:                                 # off unless written
        model: onus.lanes-model.json           # from onus lanes train, read at the base commit
        judge: 0.6                             # raise to the judge at or above this risk
        human: 0.85                            # raise to a person at or above this risk
      rollout:                                 # each lane optional; built-in plans otherwise
        judge:
          strategy: canary                     # none, canary or flag
          steps: [5, 25, 100]                  # % of traffic
          bakeMinutes: 30
          guards:
            - { metric: error_rate, maxRatio: 1.5 }
            - { metric: checkout_conversion, minRatio: 0.97 }

Inputs only raise a lane. A threshold of N means N or more, and the highest lane whose threshold is met applies (`judge`, `human`, `blocked`). Writing a key replaces its default as a whole: `blastRadius: { human: 10 }` has no judge threshold, and `blastRadius: {}` turns the input off. A lane of `auto-merge` turns `tests` or `confidence` off. Thresholds are at least 1 and must not go down as the lane goes up; `history.days` is 1 to 3650.

`coverage` reads the report's coverage of the head (`onus help reports`): when tests ran less than `minChanged` (a share from 0 to 1) of the changed executable lines, the change goes to the judge. So does a change to a file with runtime code that the coverage leaves out (`notMeasured`): coverage of other files cannot let it through. With `required: true`, a change without coverage goes to the judge too. A change with no changed executable lines, and no changed file the coverage leaves out, passes. Coverage given with `--coverage` counts. Coverage an environment recorded counts only when its run's command is one of `commands`, written exactly as `onus env run` ran it: an agent picks the commands it runs, and one could write a coverage file. When the report holds recorded coverage from another command, the change goes to the judge, whatever `required` says: an extra run must not hide an honest one. The input runs only while the change would auto-merge.

The learned model also only raises a lane, and never to `blocked`. Its thresholds need `0 < judge <= human <= 1`, and the model file's name must end in `lanes-model.json`, so a change to it is a rule of the game and goes to a person.

`rollout` sets the plan of `auto-merge`, `judge` and `human` changes; a lane left out keeps its built-in plan (`onus help production` has the table), and `blocked` has none. Steps go up strictly from 1 to 100 and end at 100, and `none` takes no steps. A guard names a `metric` and at least one of `max`, `min` (on the value), `maxRatio` and `minRatio` (on the value over its baseline).

## environment

The container `onus env create` builds from a commit: its image (default: the devcontainer's), a setup command run once per lockfile and kept as a warm image, a seed command, and the files a run writes that are kept as evidence. See `onus help environments`.

    environment:
      image: node:22-alpine
      setup: npm ci
      seed: node scripts/seed-test-data.mjs
      evidence: ["reports/**/*.xml"]     # JUnit XML
      traces: ["otel/*.json"]            # OTLP JSON
      coverage: ["coverage/lcov.info"]   # lcov, Cobertura XML or Istanbul JSON
      lockfiles: [package-lock.json]     # default: the usual lockfiles
      egressImage: ubuntu/squid          # the proxy for hosts a token names
