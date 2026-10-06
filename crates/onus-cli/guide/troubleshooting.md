# Troubleshooting

## Map confidence notes

These diagnostics mean Onus could not resolve something statically. Rows touching those files are marked low confidence and rank higher.

| Diagnostic | Meaning | What to do |
|---|---|---|
| unresolved-import | An import points at a file or export that does not exist. | Check the path, tsconfig `paths`, or the workspace package's entrypoint. |
| dynamic-import | `import(x)` or `require(x)` with a computed path. | Nothing, if intended; the target is unknown to Onus. |
| dynamic-access | A call through a computed member, such as `handlers[name]()`. | Nothing, if intended. |
| dynamic-config-key | `process.env[x]` with a computed key. | Nothing, if intended. |
| dynamic-event-name | An event name computed at runtime. | Use a string literal, a constant or an enum member. |
| raw-query | `prisma.$queryRaw` and similar: the tables are unknown. | Nothing; the call is flagged so a person can look. |
| parse-error | The file has a syntax error; its facts may be incomplete. | Fix the syntax. |
| no-entrypoint | A component has no entrypoint, so all its symbols are internal. | Add `main`, `types` or `exports` to its package.json, or `entrypoints` in onus.yaml. |
| unknown-contract | A contract in onus.yaml names a symbol that is not in the map. | Fix the id; list ids with `onus map . --json`. |

## Common questions

**A change I care about only shows up in the internal row.**
Onus reports the kinds of change listed in `onus help changes`. Label the component (`labels: [payments]`) so edits there become security-sensitive, declare your event patterns and SDKs in onus.yaml, and declare boundary rules.

**An exported symbol is listed as internal.**
Only symbols reachable from the component's entrypoint are public. Check which entrypoint Onus found (`onus map . --json`, under `components`) and set `entrypoints` in onus.yaml if it is wrong.

**My SDK is not detected as an external service.**
Add it under `extractors.externals`, keyed by its npm package. Onus counts an SDK as called when code calls something imported from it, directly or through a class field declared with its type.

**My events are not detected.**
Declare the call patterns under `extractors.events`; declaring them replaces the defaults.

**A rename shows up as a removed and an added symbol.**
Renames are detected only when the body is identical. A rename plus an edit shows as two changes; for a public symbol that means an export removed (breaking) and an export added.

**`onus report` says a ref is unknown.**
The commit is not in the local clone. In CI, fetch the full history (`fetch-depth: 0`) or fetch the base commit explicitly.

**The line counts differ from GitHub's.**
Onus diffs the two trees it analyzes: files in skipped folders or ignored by .gitignore are not counted, and a file moved with identical content counts as zero lines.

**The same pull request gives a different report.**
It should not: output is deterministic for the same two trees and the same onus.yaml. The base version's onus.yaml is used for both trees, so a pull request that edits onus.yaml is still checked against the old file.

**Is it safe to run on pull requests from forks?**
Yes. Onus reads files only: it never installs dependencies, runs scripts or executes the code it analyzes.

**The report states something wrong.**
That is a bug: a wrong fact is worse than a missing one. Open an issue with the JSON report (`--format json`) and, if you can share them, the files involved.

## Current limits

- TypeScript and JavaScript only.
- CommonJS exports (`module.exports`, `exports.x`) are not modeled.
- Data access is detected for Prisma only.
- No runtime information: dependency injection, reflection and calls between services over the network are only visible through the patterns above.
