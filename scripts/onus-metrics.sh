#!/usr/bin/env bash
# Collects the Phase 1 measures (PLAN.md 5.10) from a repository's pull
# requests and prints one JSON line per pull request that has an Onus report.
#
#   scripts/onus-metrics.sh onushq/onus                 # the last 100 PRs
#   scripts/onus-metrics.sh onushq/onus --limit 500
#   scripts/onus-metrics.sh onushq/onus --summary       # one line of totals
#
# Everything comes from GitHub as it is: the Onus comment (with the metrics
# line the action embeds), the 👍/👎 reactions on it, `/onus caught` replies
# and the pull request's own timeline. Nothing is stored anywhere else.
# Needs the GitHub CLI (logged in) and jq.

set -euo pipefail

usage() {
  sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

repo=""
limit=100
summary=false
while [ $# -gt 0 ]; do
  case "$1" in
    --limit) limit="$2"; shift 2 ;;
    --summary) summary=true; shift ;;
    -h | --help) usage ;;
    -*) echo "onus-metrics: unknown flag $1" >&2; usage 1 ;;
    *) repo="$1"; shift ;;
  esac
done
[ -n "$repo" ] || usage 1
command -v gh > /dev/null || { echo "onus-metrics: needs the GitHub CLI (gh)" >&2; exit 1; }
command -v jq > /dev/null || { echo "onus-metrics: needs jq" >&2; exit 1; }

# The most recent pull requests, newest first.
prs=$(gh api "repos/$repo/pulls?state=all&sort=created&direction=desc&per_page=100" --paginate \
  --jq '.[] | {number, title, author: .user.login, createdAt: .created_at, mergedAt: .merged_at, closedAt: .closed_at}' |
  head -n "$limit")

lines=$(
  while IFS= read -r pr; do
    [ -n "$pr" ] || continue
    n=$(jq -r .number <<< "$pr")
    comments=$(gh api "repos/$repo/issues/$n/comments" --paginate --jq '.[]' | jq -s .)
    report=$(jq -c '[.[] | select(.body | startswith("<!-- onus-report -->"))][0] // empty' <<< "$comments")
    [ -n "$report" ] || continue
    reviews=$(gh api "repos/$repo/pulls/$n/reviews" --paginate --jq '.[]' | jq -s .)
    jq -c -n \
      --argjson pr "$pr" --argjson report "$report" \
      --argjson comments "$comments" --argjson reviews "$reviews" '
      (first($report.body | capture("<!-- onus-metrics (?<m>\\{.*\\}) -->") | .m | fromjson?) // null) as $metrics
      | {
          pr: $pr.number,
          title: $pr.title,
          author: $pr.author,
          createdAt: $pr.createdAt,
          mergedAt: $pr.mergedAt,
          closedAt: $pr.closedAt,
          firstReviewAt: ([$reviews[] | select(.user.type != "Bot") | .submitted_at] | min),
          report: $metrics,
          reactions: {up: $report.reactions["+1"], down: $report.reactions["-1"]},
          caught: [
            $comments[]
            | select(.user.type != "Bot")
            | select(.body | startswith("/onus caught"))
            | {by: .user.login, at: .created_at, note: (.body | ltrimstr("/onus caught") | ltrimstr(" ") | split("\n")[0])}
          ]
        }'
  done <<< "$prs"
)

if [ "$summary" = false ]; then
  [ -z "$lines" ] || printf '%s\n' "$lines"
  exit 0
fi

# Totals. Large pull requests have at least 500 changed lines (PLAN.md 5.10).
printf '%s\n' "$lines" | jq -s -c --arg repo "$repo" '
  def ratio(a; b): if b > 0 then ((a / b) * 1000 | round) / 1000 else null end;
  def median: sort | if length == 0 then null else .[(length / 2 | floor)] end;
  map(select(. != null)) as $all
  | ($all | map(select((.report.changedLines // 0) >= 500))) as $large
  | {
      repo: $repo,
      reports: ($all | length),
      largeReports: ($large | length),
      usefulOnLarge: ratio(($large | map(.reactions.up) | add // 0);
                           ($large | map(.reactions.up + .reactions.down) | add // 0)),
      catchRate: ratio(($all | map(select(.caught | length > 0)) | length); ($all | length)),
      medianLinesPerRow: ($all | map(.report | select(. != null and .rows > 0) | .changedLines / .rows) | median),
      medianSecondsToFirstReview: ($all
        | map(select(.firstReviewAt != null)
              | ((.firstReviewAt | fromdate) - (.createdAt | fromdate)))
        | median)
    }'
