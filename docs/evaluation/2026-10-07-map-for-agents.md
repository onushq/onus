# Does the map help coding agents? (2026-10-07)

An A/B test of the Phase 2 prototype (`onus mcp`, ADR 0007): coding agents implement a real feature with and without the Onus tools. We measured time, cost and tool use, and graded output quality blind against what the maintainers actually shipped.

## Setup

- **Agents**: Claude Code in headless mode (`claude -p`) with Claude Sonnet. Each run was a fresh session, and all runs went in parallel.
  - The settings were identical across runs: the same prompt, the same tool allowlist (read and edit files, read-only git and search commands), sub-agents and web access off, and no other MCP servers (`--strict-mcp-config`).
  - Agents could not install dependencies or run builds, tests or any repository code.
- **Onus condition**: the same, plus `onus mcp` (ten tools) and one paragraph in the prompt asking the agent to explore with the `onus_*` tools and to run `onus_check` before finishing.
- **Task**: a real feature from Twenty (open-source CRM, `twentyhq/twenty`, 31,000 TypeScript files). The feature is a workspace flag that turns off record-share visibility gating, and it has to be honoured at every point where gating is enforced (PR #27177). It was rewritten as a product spec without file names. Each run started from the parent of the shipped commit.
- **Grading**: a separate reviewer agent saw the task, the shipped diff and the six diffs labeled A–F (assignments randomized). It verified defects against the base commit with `git show`/`git grep` and scored each solution from 0 to 10.

## First evaluation: the map tools

| Run | Wall time | Turns | Cost | Tool calls | Onus calls | Overall (0–10) |
|---|---|---|---|---|---|---|
| Baseline 1 | 254 s | 67 | $1.48 | 66 | n/a | 7.5 |
| Baseline 2 | 220 s | 73 | $1.31 | 72 | n/a | **8.5** |
| Baseline 3 | 230 s | 63 | $1.41 | 62 | n/a | 7.5 |
| Onus 1 | 229 s | 68 | $1.51 | 67 | `check` ×1 | 7.5 |
| Onus 2 | 215 s | 78 | $1.40 | 77 | `check` ×1 | **8.5** |
| Onus 3 | 248 s | 85 | $1.57 | 84 | `check` ×1 | 6.5 |
| **Mean, baseline** | **235 s** | | **$1.40** | | | **7.8** |
| **Mean, Onus** | **231 s** | | **$1.49** | | | **7.5** |

All six solutions honoured the flag at every enforcement point the shipped PR covers. They were ranked on edge cases, formatting and test strength.

### Findings

1. **No measurable gain in speed, cost or quality.** The two conditions tied, and the spread within a condition (6.5 to 8.5) was larger than the gap between them.
2. **Agents did not use the map to explore, even when asked.** They made no `onus_find`, `onus_dependents`, `onus_dependencies`, `onus_file` or `onus_tests_for` calls. Instead they went straight to `grep`, `cat` and `sed -n`, which find code in a well-named repository in a few calls.
   - Claude Code defers MCP tool definitions until the agent searches for them, so tools that are not suggested are rarely loaded.
   - In earlier trial runs, a first `onus_find` that came back empty (it required every query word in one symbol name) ended the agent's use of Onus. Search now ranks by word coverage and also matches field names and paths.
3. **`onus_check` is the tool agents reach for unprompted, but it must be sharper.** Every Onus run called it at the end.
   - Its rows here were noisy, such as "may have changed its inferred type" and a full union type printed out.
   - They were also too coarse: all of the permission logic was one row, "375 changed lines inside `twenty-server`".
   - No agent changed anything after reading it.

## Second evaluation: a precise `onus_check`

After the first evaluation, `onus_check` was changed in five ways:

- It compares untyped values key by key and union types member by member, and folds the rest into one quiet row.
- It flags APIs of a package that production code uses for the first time, together with the version the repository pins.
- It names the implementations and test doubles that a breaking change left untouched.
- It names the module folders changed inside large packages.
- It returns a short checklist of what to verify before finishing.

Everything else stayed the same: the same task, model, prompt and allowlist, three runs per condition, graded blind again.

| | Baseline | Onus |
|---|---|---|
| Quality (mean of 3) | **8.0** | 6.3 |
| Time (mean) | 269 s | 227 s |
| Cost (mean) | $1.67 | $1.32 |

- **The checklist was empty in every Onus run**, because the change used no new APIs and broke no interfaces. So `onus_check` had nothing to add. As in the first evaluation, the Onus runs scored lower and stopped sooner.
- A plausible cause is that **a clean check reads as "done"**. In response, `onus_check` now states in every answer what it does not verify (logic, edge cases, compilation, tests).

## What this means for Onus

Onus is not meant to make agents better at writing code; agents and their harnesses do that. Onus assesses the risk of each change, tells people when they need to look, and asks agents for evidence instead of trust. Read that way:

- **Navigation tools are not where the value is.** Agents find code with text search. The MCP tools stay, as an evidence channel, with navigation experimental.
- **Verified facts are the hypothesis to test next.** `onus_check` should state facts an agent cannot easily see, such as a first-time API and its pinned version or an untouched test double, and the agent should answer them with evidence. These are the same facts a pull request report should show a reviewer. This evaluation's task triggered none of them, so the next evaluation needs tasks that do.
- **A clean result is not a sign-off.** The risk report and `onus_check` say what they did not verify, so that neither an agent nor a person mistakes "nothing flagged" for "safe".

## Limits

- One model (Sonnet), one harness (Claude Code), one task, three runs per condition.
- Agents could not run builds or tests, which raises the value of any static verification relative to normal use.
- The grader is itself a model, but it verified its findings against the repository.
