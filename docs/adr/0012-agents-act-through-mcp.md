# 0012. Agents hand work in through MCP, and cannot approve themselves

Status: Accepted (2026-10-09)

## Context

Agents reached the map through `onus mcp`, but everything after writing the code (running tests as evidence, submitting, asking for more access) needed the command line, so agents skipped it or did it inconsistently. Giving agents those steps as tools is what makes Onus the layer every agent works through, but some of Onus's actions decide what an agent may do and must stay with people or policy.

## Decision

1. `onus mcp` adds tools that act: `onus_lanes`, environments (`onus_env_create`, `onus_env_run`, `onus_envs`, `onus_env_destroy`), `onus_evidence`, `onus_run_test`, `onus_submit`, `onus_judge`, `onus_escalate`, `onus_escalation` and `onus_outcomes` (read-only). `--no-actions` leaves them out.
2. They call the same functions as `onus ui` and the commands, through a fixed list. Anything not on it is refused.
3. **Agents cannot approve themselves.** They cannot record outcomes (which earn auto-merge), grant or deny escalations, mint or widen tokens, or pick the scope a change is checked against. With `--token`, the server attaches its task token to submissions, environments and escalations, whatever the agent passes.
4. Submissions are kept in `.onus/submissions/` with the judge's verdict beside them, so people see what agents handed in (`onus ui`, Lanes & judge). A granted escalation keeps its token beside the request (owner-only), so the agent that asked can collect it with `onus_escalation`.

## Consequences

- The workflow an agent follows is the one Onus checks: evidence from an environment, a submission with an intent, the judge's verdict.
- An agent that runs `onus mcp` can start containers. They run with the isolation of ADR 0010, but operators who want only questions answered pass `--no-actions`.
- Outcome records stay a person's or CI's to write; that is what keeps track records meaningful.
