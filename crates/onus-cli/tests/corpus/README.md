# Regression corpus

Each `*.case` file is a small, invented change that reproduces a kind of
mistake reviewers found in real reports, or a control that must keep
working. `tests/corpus.rs` builds both trees, runs the diff and checks the
expectations. No case contains code from the repositories that were audited.

```
One or more lines describing the case (free text until the first section).
status: open          # optional: a known mistake, allowed to fail

=== both package.json  # a file in both trees
...
=== base src/a.ts      # only in the base tree (deleted, or changed below)
...
=== head src/a.ts      # only in the head tree (added, or changed above)
...
=== expect
row <subkind> [●|○] [title~"text"] [why~"text"]   # such a row exists
no <subkind> [title~"text"]                       # no such row; `*` = any subkind
attention <n>                                     # exactly n rows need a person
```

Run one case, and see why it fails even when it is open, with
`ONUS_CORPUS_CASE=<part of its name> cargo test -p onus-cli --test corpus`.
