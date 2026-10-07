# Does the map help coding agents? (2026-10-07)

An A/B test of the Phase 2 prototype (`onus mcp`, ADR 0007): coding agents implement a real feature, with and without the Onus tools. We measured time, cost and tool use, and graded output quality blind against what the maintainers actually shipped.

## Setup

- **Agents**: Claude Code in headless mode (`claude -p`), Claude Sonnet, one fresh session per run, all running in parallel. The settings were identical: same prompt, same tool allowlist (read and edit files, read-only git and search commands), sub-agents and web access off, and no other MCP servers (`--strict-mcp-config`). Agents could not install dependencies or run builds, tests or any repository code.
- **Onus condition**: the same plus `onus mcp` (ten tools). The "hint" variants added one paragraph to the prompt asking the agent to explore with the `onus_*` tools and to run `onus_check` before finishing.
- **Tasks**: real features from each repository's history, rewritten as a product spec without file names. Each run started from the parent of the shipped commit.
  - **Marketplace** (private Nx/pnpm monorepo, 8,600 source files): LinkedIn URL and CV upload on publisher profiles. A vertical slice through migration, repository, domain, GraphQL and SvelteKit (the shipped commit touched 25 files).
  - **Twenty** (open-source CRM, `twentyhq/twenty`, 31,000 TypeScript files): a workspace flag that turns off record-share visibility gating. It has to be honoured at every point where gating is enforced (PR #27177).
- **Grading**: a separate reviewer agent saw the task, the shipped diff and the diffs labeled A–F (assignments randomized), and verified defects against the base commit with `git show`/`git grep`. It scored each solution from 0 to 10.

## Results

### Marketplace

| Run | Wall time | Turns | Cost | Tool calls | Onus calls | Overall (0–10) |
|---|---|---|---|---|---|---|
| Baseline | 288 s | n/a¹ | n/a¹ | ~96¹ | n/a | **8.5** |
| Baseline (repeat) | 273 s | 78 | $1.34 | 77 | n/a | **7.5** |
| Onus, no hint | 275 s | n/a¹ | n/a¹ | ~67¹ | 0 | 3.5 |
| Onus, no hint (repeat) | 299 s | 114 | $1.91 | 113 | `check` ×1 | 6 |
| Onus + hint | 342 s | 98 | $1.69 | 97 | `find` ×1 (empty), `check` ×1 | 5 |
| Onus + hint, improved `find` | 248 s | 71 | $1.19 | 70 | `find` ×1, `check` ×1 | 2 |

¹ The logs of these two runs were lost to a harness error. Their code was graded normally.

Most of the quality gap comes from one failure that has nothing to do with Onus. Three Onus runs and one other wrote `effect` APIs that do not exist in the repository's pinned `effect@2.4.19` (`Effect.void`, `Effect.asVoid`, `Effect.fromEither`, reversed `Either` parameters), and two of them crash at runtime. Both baselines happened to avoid them.

### Twenty

| Run | Wall time | Turns | Cost | Tool calls | Onus calls | Overall (0–10) |
|---|---|---|---|---|---|---|
| Baseline 1 | 254 s | 67 | $1.48 | 66 | n/a | 7.5 |
| Baseline 2 | 220 s | 73 | $1.31 | 72 | n/a | **8.5** |
| Baseline 3 | 230 s | 63 | $1.41 | 62 | n/a | 7.5 |
| Onus + hint 1 | 229 s | 68 | $1.51 | 67 | `check` ×1 | 7.5 |
| Onus + hint 2 | 215 s | 78 | $1.40 | 77 | `check` ×1 | **8.5** |
| Onus + hint 3 | 248 s | 85 | $1.57 | 84 | `check` ×1 | 6.5 |
| **Mean, baseline** | **235 s** | | **$1.40** | | | **7.8** |
| **Mean, Onus** | **231 s** | | **$1.49** | | | **7.5** |

All six solutions honoured the flag at every enforcement point the shipped PR covers. They were ranked on edge cases, formatting and test strength.

## Findings

1. **No measurable gain in speed, cost or quality.** On Twenty the two conditions tied, and the spread within a condition (6.5 to 8.5) was larger than the gap between them. On Marketplace the baselines ranked first and second, mostly because of library-version mistakes unrelated to the map.
2. **Agents do not use the map to explore, even when asked.** Across the seven Onus runs there was at most one `onus_find` per run, and no `onus_dependents`, `onus_dependencies`, `onus_file` or `onus_tests_for` calls. Agents went straight to `grep`, `cat` and `sed -n`, which find code in a well-named repository in a few calls. Without the hint, agents never loaded the exploration tools, only `onus_check` at the end: Claude Code defers MCP tool definitions until the agent searches for them.
3. **A first answer that comes back empty ends the experiment.** The first version of `onus_find` required every word of the query to appear in one symbol name. `"publisher profile"` returned nothing, and the agent never called Onus again. Search now ranks by word coverage and also matches field names and paths, but agents still made only one call.
4. **`onus_check` is the tool agents reach for unprompted, but it must be sharper.** All six Onus runs with complete logs called it at the end, including one without a hint (the server instructions suggest it).
   - Once, it changed behaviour: flagging that `PublisherRepository` gained required methods ("used in 36 files across 8 components") sent the agent back to check its test doubles.
   - Otherwise it was ignored. Its rows were noisy ("may have changed its inferred type") or too coarse: in Twenty, all of the permission logic was one row, "375 changed lines inside `twenty-server`".
5. **The defects that mattered could be caught by verification, not navigation.** For example, using an API of an external package that the repository has never used, which the pinned version may not have. The map can know this without a compiler.

## Conclusions

- Navigation tools over the map do not beat text search for agents implementing a clearly specified feature. Phase 2 should not be built around them.
- What agents cannot get from `grep` is *verification*: what a change means and what else it breaks. That is `onus_check`, the same engine as the Phase 1 pull request report, so improving it serves people and agents alike.
- Next: make `onus_check` precise. Drop unverified type rows, flag unprecedented external API use with the pinned version, list every implementer and test double of a changed interface, and split large packages into components by module. Then repeat this test on tasks where verification is the hard part (reviewing or extending an existing change), with at least three runs per condition.

## Second evaluation: a precise `onus_check`

After the first evaluation, `onus_check` was changed in four ways:
- It compares untyped values key by key and union types member by member, and folds the rest into one quiet row.
- It flags a package's APIs that production code uses for the first time, together with the version the repository pins.
- It names the implementations and test doubles that a breaking change left untouched.
- It names the module folders changed inside large packages.

It now also returns a short checklist of what to verify before finishing. Everything else stayed the same: the same tasks, model, prompts, hint and allowlist, with three runs per condition on both repositories, graded blind again.

| | Baseline | Onus | Difference |
|---|---|---|---|
| Marketplace quality (mean of 3) | 5.7 | **6.7** | +1.0 |
| Twenty quality (mean of 3) | **8.0** | 6.3 | −1.7 |
| Time, Marketplace / Twenty | 268 s / 269 s | 248 s / 227 s | |
| Cost, Marketplace / Twenty | $1.42 / $1.67 | $1.33 / $1.32 | |

- **On Marketplace the checklist changed the code.** Every Onus run got "First use of `Effect.asVoid` from `effect`; check that it exists in `effect` 2.4.19". In two of the three runs the agent searched the repository, found the 2.x name, and fixed it. One of those became the only solution of the six that compiles, passes its tests and works for logged-out visitors. Two of the three baselines shipped APIs that the pinned version does not have, against one of the three Onus runs (that agent ignored the item).
- **On Twenty the checklist was empty**, because the change used no new APIs and broke no interfaces. As in the first evaluation, the Onus runs scored lower and stopped sooner. A plausible cause is that a clean check reads as "done". `onus_check` now says what it does not verify (logic, edge cases, compilation, tests) in every answer.
- **The defect that sank three Marketplace solutions is also a static fact.** A new GraphQL type's fields lacked the `@skipAuth` directive that every sibling field in the file carries. Checking for drift from neighbouring conventions like this is a candidate for the next check.

## What this means for Onus

Onus is not meant to make agents better at writing code. Agents and their harnesses do that. Onus assesses the risk of each change, tells people when they need to look, and asks agents for evidence instead of trust. Read that way:

- **Navigation tools are not where the value is.** Agents find code with text search. The MCP tools stay, as an evidence channel, with navigation experimental.
- **Verified facts are.** When `onus_check` stated a fact an agent could not easily see (a first-time API and its pinned version, an untouched test double), the agent acted on it, and the submission came with evidence. These are the same facts a pull request report should show a reviewer.
- **A clean result is not a sign-off.** The risk report and `onus_check` say what they did not verify, so neither an agent nor a person mistakes "nothing flagged" for "safe".

## Limits

One model (Sonnet), one harness (Claude Code), two tasks, and two or three runs per condition. Agents could not run builds or tests, which raises the value of any static verification relative to normal use. The grader is itself a model, but it verified its findings against the repositories.
