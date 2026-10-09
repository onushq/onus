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

## testData

Globs of files that are test data rather than code: fixture projects, sample apps, recorded responses.

    testData:
      - fixtures/**
      - e2e/recordings/**

Test data is left out of the map, so a fixture app's imports, services and contracts do not show up as your code. Changes to it are summarized in one "changed lines of test data" row, and config files inside it (a fixture's package.json or tsconfig.json) are not reported as config changes.

Secret-shaped values added in test data are not counted as committed secrets: `fail-on: secrets` does not trip on them, and they appear as one row, "... added in test data", that names the files without the values. Declare only folders that hold fake credentials on purpose. As with every setting, the report uses the base commit's onus.yaml, so a pull request cannot declare its own new secret as test data.

## Files Onus never reads

Folders named .git, node_modules, dist, build, out, .next, .turbo, .cache, coverage, target or vendor; anything ignored by a .gitignore inside the tree; files over 2 MB; and *.min.js files.

## lanes

What happens to a change after it is reported: the default lane, rules that set or raise it, the share of automatic merges audited by a person, the record an agent setup needs before it may auto-merge, held-out checks and an optional taste reviewer for the judge. See `onus help lanes`.

    lanes:
      default: judge          # auto-merge, judge, human or blocked
      auditRate: 0.05
      minRecord: 10
      rules:
        - { lane: auto-merge, match: every, kinds: [internal], components: [docs] }
        - { lane: human, subkinds: [migration-changed] }
      heldOut: { command: "npm run test:held-out", image: "node:22", setup: "npm ci" }

## environment

The container `onus env create` builds from a commit: its image (default: the devcontainer's), a setup command run once per lockfile and kept as a warm image, a seed command, and the files a run writes that are kept as evidence. See `onus help environments`.

    environment:
      image: node:22-alpine
      setup: npm ci
      seed: node scripts/seed-test-data.mjs
      evidence: ["reports/**/*.xml"]     # JUnit XML
      traces: ["otel/*.json"]            # OTLP JSON
      lockfiles: [package-lock.json]     # default: the usual lockfiles
      egressImage: ubuntu/squid          # the proxy for hosts a token names
