# Onus records

Outcome records (`outcomes.jsonl`), pull request records (`pulls.jsonl`) and
production signals (`signals.jsonl`) kept by the Onus GitHub Action. Each line
is one record; the files only grow. Read them with
`onus outcomes summary --file outcomes.jsonl`, `onus signals areas --records .`
or in `onus ui`.

An agent's submission of a change may be kept in `submissions/<pull request
number>.json` or `submissions/<head commit>.json`; the action passes it to
`onus ci pr --submission` for the change's provenance.
