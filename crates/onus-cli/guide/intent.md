# The intent check

The gap between what a change says it does and what it actually does is the most important signal in a review. Onus checks it deterministically: you (or the agent that wrote the change) state the intent in a small block, and every row outside it is an intent mismatch, ranked at the very top and marked as needing a person.

## Stating an intent

In a pull request body, a fenced block:

    ```onus-intent
    summary: Add SMS alerts on OrderShipped
    touches: [notifications]
    contracts: [UserPreferences]
    externals: [Acme SMS]
    ```

Or a YAML file with the same keys. Pass either to `--intent`:

    onus report --base main --head HEAD --intent pr-body.md
    onus diff old new --intent intent.yaml

All keys are optional; unknown keys are an error. A Markdown file without an `onus-intent` block skips the check with a warning on stderr.

## What each key covers

- `summary`: free text, shown in the report.
- `touches`: the components the change may touch, by id (`notifications`), or path globs (`services/notifications/**`, any entry with `/` or `*`). A row is inside `touches` when its component is listed, or when every file it points at matches one of the globs.
- `contracts`: the contracts the change may alter, by name (`UserPreferences`) or map id. Applies to contract rows (subkinds `contract-*` and `export-*`).
- `externals`: the external services the change may start calling, by vendor (`Acme SMS`), id (`acme-sms`) or SDK package (`@acme/sms`), case-insensitive. Applies to `new-external-service` and `new-external-call` rows.

Every row must be inside `touches`. Contract rows must also be listed in `contracts`, and external-service rows in `externals`. Everything else is a mismatch, including the internal row of a component outside `touches`.

## What the report shows

    **Intent check:** **2 changes outside stated intent** · **Boundary rules:** no new violations

    Stated intent: "Add structured fields to log lines"

    - 10 lines of new code inside `billing`: `billing` is not in `touches` (logger)
    - `billing` now writes to the `payment` table: `billing` is not in `touches` (logger)

The mismatching rows move to the top of the table with "Outside stated intent" in their Kind column. In JSON, they have `hints.intentMismatch: true`, and `intentCheck.mismatches` lists `{ changeId, reason }` for each.

## Tips

- Ask coding agents to end every pull request description with an `onus-intent` block. It costs them nothing and makes scope creep visible.
- Keep `touches` tight. A change that legitimately touches more should say so; that is the point.
- An intent never hides a row: rows inside it are still reported and ranked as usual.
