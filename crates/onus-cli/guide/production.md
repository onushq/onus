# Production: signals, rollout plans and rollback

A merge is not the end of a change. Production is the final judge: a change meets a small share of real traffic first, it rolls back when the numbers move the wrong way, and what production says goes back into the records. Incidents and rollbacks count against the agent setup that made the change and against the components it touched, so the next change there gets a stricter lane and a more careful rollout.

Onus decides and never moves traffic. Your deploy tool, flag service or rollout controller does that; Onus gives it a plan per change, a decision per stage, and keeps what happened.

Everything here lives in the records folder (`onus help ci`), in a third file beside `pulls.jsonl` and `outcomes.jsonl`: `signals.jsonl`. `onus ci records pull|push` carries it on the records branch like the others.

## Production signals

A signal is one fact from production, in one vendor-neutral JSON shape (`schemas/production-signal.schema.json`):

| Field | What it holds |
|---|---|
| `id` | Unique per source, such as `pagerduty:Q1ABC`. Required. A signal whose id is on record already is not recorded again. |
| `kind` | `incident`, `error-spike`, `latency`, `metric`, `rollback`, `deploy` or `trace`. (`rollout` is Onus's own, below.) |
| `at` | Unix seconds; the time of the ingest when left out. |
| `source` | The system it came from: `pagerduty`, `sentry`, `datadog`, `argo`, … |
| `severity` | `low`, `medium`, `high` or `critical`. |
| `change` | The change it is about, `owner/repo#123`. |
| `commit` | A commit sha, or at least 7 hex digits of one. |
| `deploy`, `environment` | The deploy tool's id, and where (absent means production). |
| `components`, `symbols` | Map ids it involved; a symbol counts for its component. |
| `note` | What happened. |
| `metrics` | Samples, `{name, value, baseline?, unit?}`. |
| `rollout` | For a deploy: `{percent}`, the share of traffic it reached. |
| `calls` | For a trace: caller and callee pairs observed. |

One example per kind:

    {"id":"pagerduty:Q1ABC","kind":"incident","severity":"high","commit":"3f9a1c2","components":["billing"],"note":"checkout 500s"}
    {"id":"sentry:4711","kind":"error-spike","change":"acme/shop#42","metrics":[{"name":"error_rate","value":0.031,"baseline":0.012}]}
    {"id":"datadog:lat-9","kind":"latency","components":["orders"],"metrics":[{"name":"latency_p95_ms","value":840,"baseline":310}]}
    {"id":"datadog:conv-2","kind":"metric","severity":"medium","components":["billing"],"metrics":[{"name":"checkout_conversion","value":0.021,"baseline":0.03}]}
    {"id":"argo:rb-17","kind":"rollback","commit":"3f9a1c2","source":"argo","note":"error rate 3x"}
    {"id":"deploy:4711:25","kind":"deploy","commit":"3f9a1c2","environment":"production","rollout":{"percent":25}}
    {"id":"trace:9f2c41d0a7b3e611","kind":"trace","calls":[{"from":{"service":"orders","file":"src/orders.ts","function":"placeOrder"},"to":{"service":"billing"},"count":214}]}

Record them with one object, an array, or JSON Lines; `-` reads stdin:

    onus signals ingest --records rec --file pagerduty-export.json
    # {"recorded":3,"duplicates":1,"attributed":[{"id":"pd:Q1ABC","change":"acme/shop#42","by":"commit"}],
    #  "outcomes":[{"change":"acme/shop#42","result":"incident"}]}

What happens to each signal:

- **It is tied to a change**: by `change` when that change is on record, else by `commit`, matched against the commit each change landed as and the head of its pull request (either one may be a prefix of the other). A commit several changes match goes to the most recent, and the answer lists a `warnings` entry. The stored line gets `change` and `attributedBy` (`change` or `commit`), and the pull record's components when it named none. Both fields are Onus's: an `attributedBy` in the input is dropped, and so is a `change` that is not on record (the answer warns), so a signal cannot claim a tie Onus did not find.
- **An incident or a rollback tied to a change becomes its outcome**: `incident` or `rolled-back`, with `signal` naming the signal, through the same logic as `/onus incident`. The agent setup's track record, `onus outcomes summary`, `onus ui` and Onus Cloud count it with no other step. Ingesting it again adds nothing, and neither does a second incident while the change's latest result is already an incident. A rollback after a planned one is a new rollback.
- **A signal tied to no change is still kept.** It counts toward its components' history (below).
- **A whole file fails on one bad signal**, naming the line: an empty `id`, a `commit` that is not hex or shorter than 7, a `change` that is not `owner/repo#123`, a `trace` without `calls`, `rollout` on a kind other than `deploy`, a `deploy` that names neither commit nor change, a deploy carrying a `decision`, or any `rollout` signal. Rollout stages carry Onus's decisions, and only `onus ci rollout` writes them.

### Wiring monitoring in

Turn each tool's export into signals with `jq`, and pipe them in.

PagerDuty incidents (the REST API's `/incidents`), with the commit in a custom field or the title:

    curl -s -H "Authorization: Token token=$PD_TOKEN" "https://api.pagerduty.com/incidents?since=$SINCE" \
      | jq -c '.incidents[] | {id: ("pagerduty:" + .id), kind: "incident", source: "pagerduty",
               severity: (if .urgency == "high" then "high" else "medium" end),
               at: (.created_at | fromdateiso8601), note: .title,
               components: [.service.summary]}' \
      | onus signals ingest --records rec --file -

Sentry issues carry the release, which is the commit when releases are named after it. A release that is not a commit (`1.2.3`) is left out, since one bad signal fails the whole file:

    jq -c '.[] | {id: ("sentry:" + .id), kind: "error-spike", source: "sentry", note: .title,
                  commit: (.firstRelease.version // "" | if test("^[0-9a-f]{7,64}$") then . else null end)}' \
         sentry-issues.json \
      | onus signals ingest --records rec --file -

Datadog monitor alerts, from a webhook's payload saved to a file:

    jq -c '{id: ("datadog:" + .alert_id), kind: "metric", source: "datadog", severity: "high",
            components: [.tags | split(",")[] | select(startswith("component:")) | ltrimstr("component:")],
            metrics: [{name: .metric, value: (.value | tonumber)}], note: .title}' alert.json \
      | onus signals ingest --records rec --file -

Then push the records: `onus ci records push --dir rec`.

## Production traces into the map

Calls production observed are calls the map should know. Keep them from an OpenTelemetry Collector's file exporter:

    onus signals traces --records rec --file otel-export.json --source otel-prod
    # {"recorded":1,"calls":214,"id":"trace:9f2c41d0a7b3e611"}

It reads OTLP JSON (one document, or one per line), pairs each span with its parent where the code attributes allow (`code.filepath`, `code.function`), counts identical pairs, keeps the 5000 busiest, and records one `trace` signal named by a hash of its calls, so the same export twice is one signal. The signal's `components` are the services that are components: those of the repository `--repo` names (found as the map finds them), and those `--service-component checkout-api=orders` names for a service whose name is not a component id. Without either, they are left out.

`--traces` (on `onus map`, `onus diff` and `onus report`) reads `signals.jsonl` as well as OTLP files: the `trace` lines give their calls, other lines are skipped. A call the map did not have becomes a `calls` edge with confidence `traced`. A call the map had only guessed (`inferred` or `low`) that production observed is upgraded to `traced`, and `onus map` says how many (`traces: 3 recorded calls, 2 traced calls added, 1 confirmed, 1 upgraded`). Static, compiler and declared edges stay as they are. Traced edges are only added, never removed. The action passes the records' `signals.jsonl` to `onus report --traces` when `records: true`, so every report's map includes what production saw (`onus help environments`).

## Rollout plans

Every classification carries a plan for the change's lane (`onus classify`, `onus judge`, `onus ci pr`, `onus ui`). When the judge moves the lane, `onus judge` prints the final lane's plan as `rollout` beside the classification, and `onus ci pr` writes it as `rollout.json`:

    "rollout": {
      "lane": "judge", "strategy": "canary", "flag": null,
      "steps": [5, 25, 100], "bakeMinutes": 30,
      "guards": [{"metric": "error_rate", "maxRatio": 1.5}, {"metric": "latency_p95_ms", "maxRatio": 1.3}],
      "automaticRollback": true,
      "components": ["billing", "orders"],
      "reasons": ["the judge lane's plan",
                  "`billing` had 1 incident and 1 revert or rollback in the last 90 days: the human lane's plan instead"]
    }

The built-in plans:

| Lane | Strategy | Steps (% of traffic) | Bake | Rolls back when (ratio to baseline) |
|---|---|---|---|---|
| auto-merge | `canary` | 10, 100 | 10 min | error_rate > 2.0×, latency_p95_ms > 1.5× |
| judge | `canary` | 5, 25, 100 | 30 min | error_rate > 1.5×, latency_p95_ms > 1.3× |
| human | `flag` (behind a feature flag, then turned up) | 1, 5, 25, 100 | 120 min | error_rate > 1.2×, latency_p95_ms > 1.2× |
| blocked | `none`: it may not ship | none | none | none |

`lanes.rollout` in `onus.yaml` replaces a lane's plan; a lane left out keeps its built-in one (`onus help configuration`):

    lanes:
      rollout:
        judge:
          strategy: canary
          steps: [5, 25, 100]
          bakeMinutes: 30
          guards:
            - { metric: error_rate, maxRatio: 1.5 }
            - { metric: checkout_conversion, minRatio: 0.97 }

A guard sets `max` or `min` (on the value) or `maxRatio` or `minRatio` (on the value over its baseline, for metrics that must not rise or drop). Steps go up strictly from 1 to 100 and end at 100; `none` has no steps. A plan for strategy `flag` names a suggested flag key, `onus-<owner>-<repo>-<n>`.

**History tightens the plan.** When a component the change touches had an incident, a revert or a rollback in the history window (`lanes.inputs.history.days`, default 90), the change gets the next stricter lane's plan: auto-merge takes the judge's, the judge the human's. In the human lane the bake time doubles. The reason is listed. The same history raises the lane itself (the `history` input, `onus help lanes`).

`onus ci pr` writes the plan as `rollout.json`, keeps it on the pull record, adds it to its summary (`rollout.strategy`, `steps`, `bakeMinutes`, `automaticRollback`), and shows it in the comment:

    **Rollout:** `canary` 5% → 25% → 100%, 30 min at each step; rolls back automatically when error_rate > 1.5× baseline or latency_p95_ms > 1.3× baseline.

## Deciding each stage

A deploy tool reports a stage, with the metric samples it took (`schemas/rollout-metrics.schema.json`), and acts on Onus's decision:

    onus ci rollout --change acme/shop#42 --records rec                # the plan; records nothing
    onus ci rollout --commit 3f9a1c2 --percent 5 --metrics canary.json --records rec
    # {"change":"acme/shop#42","percent":5,"decision":"rollback","next":null,
    #  "reasons":["error_rate 0.031 is 2.6× its baseline 0.012 (limit 1.5×)"]}

with `canary.json`:

    {"metrics": [{"name": "error_rate", "value": 0.031, "baseline": 0.012},
                 {"name": "latency_p95_ms", "value": 200, "baseline": 190}]}

| Decision | When | Exit |
|---|---|---|
| `rollback` | A guard is breached. It wins over everything else. | 2 |
| `hold` | A guard has no sample or no baseline, or the stage has not run for `bakeMinutes` since it was first reported. | 3 |
| `complete` | The stage is 100% and healthy. | 0 |
| `promote` | Healthy and baked; `next` is the next step. | 0 |

Every call with `--percent` records the stage as a `rollout` signal: the share, the decision, the samples judged. A `rollback` also records a `rolled-back` outcome against the change, once. The change is found by `--change`, or by `--commit` as signals are; one Onus has no record of, or one in the `blocked` lane, is an error (exit 1). A change recorded before rollout plans gets the plan its lane gets now: `lanes.rollout` from the `onus.yaml` in `--repo` (default the working directory) or `--config`, tightened by its components' history.

A rollback decided elsewhere, by a person or by the deploy tool's own analysis:

    onus ci rollback --change acme/shop#42 --records rec --reason "checkout 500s" --involved billing
    onus ci rollback --commit 3f9a1c2 --records rec --reason "flag off as planned" --planned

It records a `rollback` signal and the `rolled-back` outcome (or says `already rolled back`). `--planned` is for a flag turned off on purpose: the outcome is marked `planned` and counts against neither the agent setup nor the components.

### Argo Rollouts

Run `onus ci rollout` as a `job` metric provider: the analysis fails when Onus says roll back.

    apiVersion: argoproj.io/v1alpha1
    kind: AnalysisTemplate
    metadata: { name: onus }
    spec:
      args: [{ name: commit }, { name: percent }]
      metrics:
        - name: onus
          provider:
            job:
              spec:
                backoffLimit: 0
                template:
                  spec:
                    restartPolicy: Never
                    containers:
                      - name: onus
                        image: ghcr.io/acme/onus-runner:latest   # onus, git, and your metrics query
                        command: [sh, -c]
                        args:
                          - |
                            onus ci records pull --dir rec
                            ./query-metrics.sh > canary.json
                            onus ci rollout --commit {{args.commit}} --percent {{args.percent}} \
                              --metrics canary.json --records rec; code=$?
                            onus ci records push --dir rec
                            [ $code -eq 0 ] || [ $code -eq 3 ]

A hold (exit 3) passes the job; the next analysis run decides again. A rollback (exit 2) fails it, and so does an error (exit 1: a change not on record, a blocked change, unreadable metrics), so a rollout Onus could not judge is not promoted. Set each step's pause to at least the plan's bake time.

### A flag service

A step is a flag percentage. Turn the flag named in the plan up to each step, report the stage with `onus ci rollout --percent`, and on exit 2 turn it off. The rollback is already recorded.

## Area history

    onus signals areas --records rec [--days 90]
    # {"since":1757000000,"days":90,"areas":{"billing":{"reverts":1,"incidents":1,
    #   "changes":["acme/shop#42"],"errorSpikes":2,"lastAt":1759990000}}}

Each component's history in the window: reverts and rollbacks (a planned rollback does not count), incidents, the changes behind them (a signal tied to no change stands for itself by its id), error spikes (error-spike, latency and metric signals of medium severity or more, or none given) and when the latest was recorded. Incidents and rollbacks tied to a change are counted once, from their outcomes. Deploys, rollout stages and traces are not history. A signal tied to no change names no repository, so it counts for its components in every repository whose records hold it (Onus Cloud keeps one records folder per organisation); give its `change` or `commit` to scope it. This is the history the classifier's `history` input reads, and what tightens rollout plans.

An agent setup whose recent changes were rolled back cannot auto-merge either: `record.recentRollbacks` in `onus ci pr`'s summary, and a `record` reason in the lane.

## In provenance

The provenance record's `rollout` (`onus help lanes`) holds what production said about the change: `status`, `percent`, the `plan` from its pull record, every deploy and rollout stage in time order (`stages`), and every other signal tied to it (`signals`). `onus provenance --records` reads `signals.jsonl` there; `--signals FILE` reads another.

| First that applies | `status` | `percent` |
|---|---|---|
| still open | `pending` | none |
| closed without merging | `not-shipped` | none |
| latest outcome `rolled-back` | `rolled-back` | the last stage's |
| latest outcome `reverted` | `reverted` | the last stage's |
| no production stage yet | `awaiting-deploy` | none |
| last stage decided `rollback` | `rolled-back` | its share |
| last stage decided `hold` | `holding` | its share |
| last stage at 100, `complete` or a deploy | `complete` | 100 |
| otherwise | `in-progress` | its share |

Only stages in production (`environment` absent or `production`) count; the others are listed. A deploy that names no share counts as 100.
