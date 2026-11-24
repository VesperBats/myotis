#!/usr/bin/env bash
# Watches upstream ant (https://github.com/freedom-hq/ant) for what the vendored copy in
# rust/vendor/ant does not have yet, and reports it to the owner as ONE tracking issue
# (owner's decision, 2026-11-24): ant is on no registry, so no advisory database names it,
# and a security fix in it reaches Myotis only when the copy is moved
# (rust/vendor/ant/VENDORED.md, "Watching upstream"). Run daily by
# .github/workflows/ant-upstream-watch.yml.
#
#   scripts/ant_upstream_watch.sh            compare, then open or update the issue
#   scripts/ant_upstream_watch.sh --dry-run  compare and print the report; write nothing
#
# What it compares, against the commit VENDORED.md records (the same `Commit:` line that
# scripts/vendor_ant.sh reads):
#   - the commits on upstream's default branch since that commit (count, the newest few,
#     the ones whose message reads like a security fix) and whether they touch the
#     vendored crates;
#   - upstream's tags and releases on those commits;
#   - upstream's published GitHub security advisories, or a note that they could not be
#     read;
#   - the state of the three upstream issues that keep `swarm` out of a release build.
#
# The issue: label `ant-upstream`, assigned to the owner. Nothing new (the same report as
# the open issue's, or as the last closed one's) writes nothing. A changed report rewrites
# the open issue's body and adds one comment saying what changed, so the owner is
# notified; with no open issue, a new one is opened. It never closes an issue and never
# re-vendors: both are the owner's (re-vendoring is `scripts/vendor_ant.sh <commit>`).
#
# Read-only on freedom-hq/ant. Needs gh and jq; to report, GH_TOKEN with issues: write on
# the repository named by GITHUB_REPOSITORY (set in Actions; required outside a dry run).
# Upstream's text (commit subjects, titles, advisory summaries) goes into the issue as code
# spans, so a `#123` or an `@name` in it neither links this repository's #123 nor notifies.
set -euo pipefail

UPSTREAM=freedom-hq/ant
LABEL=ant-upstream
OWNER=biafra23
TITLE="Upstream ant is ahead of the vendored copy"
# The upstream issues `rust/swarm/src/lib.rs` names as the reasons the swarm feature is off.
BLOCKERS=(269 270 271)
# What makes a commit message read like a security fix. A hint for the owner, not a
# verdict: a fix can be silent, and a match can be harmless.
SECURITY_RE='secur|vulnerab|cve-|ghsa-|rustsec|advisor|exploit|attack|malicious|dos\b|denial|overflow|underflow|out[- ]of[- ]bounds|oob\b|panic|crash|unsound|unsafe|sanitiz|validat|untrusted|bypass|leak|amplif|exhaust|oom\b|unbounded'
NEWEST=8

dry_run=false
case "${1:-}" in
  "") ;;
  --dry-run) dry_run=true ;;
  *) echo "usage: $0 [--dry-run]" >&2; exit 2 ;;
esac

root=$(cd "$(dirname "$0")/.." && pwd)
repo=${GITHUB_REPOSITORY:-}
[ "$dry_run" = true ] || [ -n "$repo" ] || { echo "GITHUB_REPOSITORY is not set: which repository gets the issue?" >&2; exit 2; }
vendored_md=$root/rust/vendor/ant/VENDORED.md
vendored=$(sed -n 's/^Commit: *\([0-9a-f]\{40\}\)$/\1/p' "$vendored_md")
[ -n "$vendored" ] || { echo "no 'Commit: <40 hex>' line in $vendored_md" >&2; exit 1; }
# The vendored crates are the directories of the copy.
crates=$(cd "$root/rust/vendor/ant/crates" && ls -d -- */ | tr -d /)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

branch=$(gh api "repos/$UPSTREAM" --jq .default_branch)
head=$(gh api "repos/$UPSTREAM/commits/$branch" --jq .sha)

# The compare view pages its commits (oldest first); --slurp joins the pages. A vendored
# commit upstream no longer knows (a rewritten history) is a report too, never a red run
# that only whoever last edited the schedule hears about.
compare_ok=true
if ! gh api --paginate --slurp "repos/$UPSTREAM/compare/$vendored...$head?per_page=100" \
    > "$work/compare.json" 2>"$work/compare.err"; then
  compare_ok=false
  echo "::warning::cannot compare $vendored with $UPSTREAM's $branch: $(tr '\n' ' ' < "$work/compare.err")"
  echo '[{"status": "unknown", "ahead_by": 0, "behind_by": 0, "commits": [], "files": []}]' > "$work/compare.json"
fi
status=$(jq -r '.[0].status' "$work/compare.json")
ahead=$(jq -r '.[0].ahead_by' "$work/compare.json")
behind=$(jq -r '.[0].behind_by' "$work/compare.json")
vendored_date=$(gh api "repos/$UPSTREAM/commits/$vendored" --jq .commit.committer.date 2>/dev/null || echo "")
if [ "$behind" != 0 ]; then
  # The vendored commit is not an ancestor of upstream's branch: a force-push upstream,
  # or a commit that was never on it (vendor_ant.sh refuses one). Reported, never hidden.
  echo "::warning::the vendored commit $vendored is not on $UPSTREAM's $branch ($status, behind by $behind)"
fi
jq '[.[].commits[]]' "$work/compare.json" > "$work/commits.json"
jq -r '.[].sha' "$work/commits.json" > "$work/shas.txt"
listed=$(wc -l < "$work/shas.txt" | tr -d ' ')
jq -r '[.[].files // [] | .[].filename] | unique | .[]' "$work/compare.json" > "$work/files.txt"
# The compare view lists at most 300 files: past that, "none touched" would be a guess.
files_complete=true
[ "$(wc -l < "$work/files.txt")" -lt 300 ] || files_complete=false
crate_re="^($(printf '%s\n' $crates | sed 's|^|crates/|; s|$|/|' | paste -sd'|')|Cargo\\.toml$|contracts/)"
touched=$(grep -E "$crate_re" "$work/files.txt" | sed -E 's|^crates/([^/]+)/.*|\1|' | sort -u | paste -sd' ' || true)

# Tags on the commits since the vendored one; releases of those tags.
gh api --paginate "repos/$UPSTREAM/tags?per_page=100" --jq '.[] | [.name, .commit.sha] | @tsv' > "$work/tags.tsv"
newer_tags=$(awk -F'\t' 'NR==FNR { s[$1]; next } ($2 in s) { print $1 }' "$work/shas.txt" "$work/tags.tsv" | sort -V)
gh api --paginate "repos/$UPSTREAM/releases?per_page=100" \
  --jq '.[] | select(.draft | not) | [.tag_name, .published_at, (.prerelease|tostring), .html_url] | @tsv' > "$work/releases.tsv"
newer_releases=$(awk -F'\t' 'NR==FNR { t[$1]; next } ($1 in t)' <(printf '%s\n' "$newer_tags") "$work/releases.tsv")

# Published advisories: public to anyone, but read with a token scoped to another
# repository, so an unauthenticated read is tried before saying they could not be read.
advisories_ok=true
if ! gh api "repos/$UPSTREAM/security-advisories?state=published&per_page=100" > "$work/adv.json" 2>"$work/adv.err"; then
  if ! curl -fsSL -H 'Accept: application/vnd.github+json' \
      "https://api.github.com/repos/$UPSTREAM/security-advisories?state=published&per_page=100" \
      > "$work/adv.json" 2>>"$work/adv.err"; then
    advisories_ok=false
    echo '[]' > "$work/adv.json"
    echo "::warning::cannot read $UPSTREAM's security advisories: $(tr '\n' ' ' < "$work/adv.err")"
  fi
fi
advisory_count=$(jq 'length' "$work/adv.json")
# New to the copy: published or revised after the vendored commit (all of them when its
# date is unknown). An older one is listed but is no reason to report.
new_advisory_count=$(jq --arg d "$vendored_date" '[.[] | select($d == "" or .updated_at > $d or .published_at > $d)] | length' "$work/adv.json")

blockers=""
blocker_states=""
for n in "${BLOCKERS[@]}"; do
  if line=$(gh api "repos/$UPSTREAM/issues/$n" \
      --jq '"\(.state)\t- [\(.state)] [\(.html_url | sub("https://github.com/"; "") | sub("/issues/"; "#"))](\(.html_url)) `\(.title | gsub("`"; "'"'"'"))`"' 2>/dev/null); then
    blocker_states+="$n ${line%%$'\t'*} "
    line=${line#*$'\t'}
  else
    blocker_states+="$n unknown "
    line="- [$UPSTREAM#$n](https://github.com/$UPSTREAM/issues/$n): could not be read"
  fi
  blockers+="$line"$'\n'
done

security_commits=$(jq -r --arg re "$SECURITY_RE" \
  '.[] | select(.commit.message | test($re; "i")) | "- [`\(.sha[0:12])`](\(.html_url)) `\(.commit.message | split("\n")[0] | gsub("`"; "'"'"'"))`"' \
  "$work/commits.json")
newest=$(jq -r --argjson n "$NEWEST" \
  'reverse | .[0:$n][] | "- [`\(.sha[0:12])`](\(.html_url)) \(.commit.committer.date[0:10]) `\(.commit.message | split("\n")[0] | gsub("`"; "'"'"'"))`"' \
  "$work/commits.json")

# What the report is about, without the date: the same state means nothing new to say.
# Advisories count with their updated_at, so a revised one is news; a blocker issue
# closing is news too. Whether the advisories could be read is not: a passing API error
# would otherwise rewrite the issue twice.
state=$(
  { echo "vendored $vendored"; echo "head $head"; echo "behind $behind"; echo "compare $compare_ok"
    printf 'tag %s\n' $newer_tags
    jq -r '.[] | "advisory \(.ghsa_id) \(.updated_at)"' "$work/adv.json" | sort
    echo "blockers $blocker_states"
  } | sha256sum | cut -c1-16)
nothing_ahead=false
if [ "$compare_ok" = true ] && [ "$ahead" = 0 ] && [ "$behind" = 0 ] && [ -z "$newer_tags" ] \
    && [ "$new_advisory_count" = 0 ]; then
  nothing_ahead=true
fi

today=$(date -u +%Y-%m-%d)
vshort=${vendored:0:12}
hshort=${head:0:12}
{
  echo "<!-- ant-upstream-watch state=$state -->"
  echo "Reported by the scheduled [ant upstream watch](https://github.com/${repo:-VesperBats/myotis}/actions/workflows/ant-upstream-watch.yml) on $today (\`scripts/ant_upstream_watch.sh\`; owner's decision, 2026-11-24). Re-vendoring, and whether a release follows, are the owner's; this workflow never re-vendors and never closes this issue."
  echo
  echo "## Commits"
  echo
  echo "Vendored: [\`$vshort\`](https://github.com/$UPSTREAM/commit/$vendored) (\`rust/vendor/ant/VENDORED.md\`). Upstream \`$branch\`: [\`$hshort\`](https://github.com/$UPSTREAM/commit/$head)."
  echo
  if [ "$compare_ok" = false ]; then
    echo "**Upstream does not know the vendored commit** (the compare view failed): upstream rewrote its history, or the copy is not upstream's. Look at it by hand."
  elif [ "$nothing_ahead" = true ]; then
    echo "Upstream has nothing the vendored copy does not: no commit, tag or advisory newer than it."
  elif [ "$ahead" = 0 ]; then
    echo "No commit on \`$branch\` since the vendored commit."
  else
    echo "**$ahead commit(s)** on \`$branch\` since the vendored commit ([compare](https://github.com/$UPSTREAM/compare/$vendored...$head))."
    [ "$behind" = 0 ] || echo "**The vendored commit is not on \`$branch\`** (behind by $behind): upstream rewrote its history, or the copy is not upstream's."
    [ "$listed" = "$ahead" ] || echo "The compare view listed $listed of them; the rest are in the compare link."
    echo
    if [ "$files_complete" = false ]; then
      echo "The compare view lists only the first 300 changed files: whether they change the vendored crates is in the compare link."
    elif [ -n "$touched" ]; then
      echo "They change the vendored part of ant: $(printf '`%s`, ' $touched | sed 's/, $//')."
    else
      echo "None of them changes the vendored crates, \`Cargo.toml\` or \`contracts/\` (by the compare view's file list)."
    fi
    echo
    echo "Messages that read like a security fix (a keyword hint, not a verdict):"
    echo
    if [ -n "$security_commits" ]; then echo "$security_commits"; else echo "- none"; fi
    echo
    echo "Newest $( [ "$ahead" -lt "$NEWEST" ] && echo "$ahead" || echo "$NEWEST" ):"
    echo
    echo "$newest"
  fi
  echo
  echo "## Tags and releases since the vendored commit"
  echo
  if [ -n "$newer_tags" ]; then
    printf -- '- `%s`\n' $newer_tags
    if [ -n "$newer_releases" ]; then
      echo
      echo "Releases:"
      echo
      printf '%s\n' "$newer_releases" | awk -F'\t' '{ pre = ($3 == "true") ? " (pre-release)" : ""; printf "- [%s](%s), %s%s\n", $1, $4, substr($2, 1, 10), pre }'
    fi
  else
    echo "None."
  fi
  echo
  echo "## Published security advisories of $UPSTREAM"
  echo
  if [ "$advisories_ok" = false ]; then
    echo "**Could not be read** (\`GET /repos/$UPSTREAM/security-advisories?state=published\` failed): look at https://github.com/$UPSTREAM/security/advisories by hand."
  elif [ "$advisory_count" = 0 ]; then
    echo "None published."
  else
    jq -r --arg d "$vendored_date" '.[] | "- [\(.ghsa_id)](\(.html_url)) \(.severity // "unknown severity"): `\(.summary | gsub("`"; "'"'"'"))` (published \(.published_at[0:10]), updated \(.updated_at[0:10]))\(if $d == "" or .updated_at > $d or .published_at > $d then " **newer than the vendored commit**" else "" end)"' "$work/adv.json"
  fi
  echo
  echo "## The upstream issues that keep \`swarm\` out of a release build"
  echo
  printf '%s' "$blockers"
  echo
  echo "To move the copy: \`scripts/vendor_ant.sh <commit>\` in a PR that links the compare view (\`rust/vendor/ant/VENDORED.md\`, its rules: a commit at least 7 days old, a security fix excepted)."
} > "$work/body.md"

if [ "$dry_run" = true ]; then
  echo "state=$state nothing_ahead=$nothing_ahead compare_ok=$compare_ok ahead=$ahead behind=$behind tags=[$(echo $newer_tags)] advisories=$advisory_count new_advisories=$new_advisory_count advisories_ok=$advisories_ok blockers=[$blocker_states]"
  echo "----- issue body -----"
  cat "$work/body.md"
  exit 0
fi

marker() { sed -n 's/^<!-- ant-upstream-watch state=\([0-9a-f]*\) -->.*/\1/p' | head -n1; }

if ! gh api "repos/$repo/labels/$LABEL" --silent 2>/dev/null; then
  gh api "repos/$repo/labels" -f name="$LABEL" -f color=d93f0b \
    -f description="Upstream ant is ahead of rust/vendor/ant (scheduled watch)" --silent
fi
gh api "repos/$repo/issues?labels=$LABEL&state=all&sort=created&direction=desc&per_page=20" \
  --jq '[.[] | select(.pull_request | not)]' > "$work/issues.json"
open=$(jq -r '[.[] | select(.state == "open")][0].number // empty' "$work/issues.json")

if [ -n "$open" ]; then
  old=$(jq -r --argjson n "$open" '.[] | select(.number == $n) | .body // ""' "$work/issues.json" | marker)
  if [ "$old" = "$state" ]; then
    echo "issue #$open is current; nothing written"
    exit 0
  fi
  jq -n --rawfile body "$work/body.md" '{body: $body}' | gh api -X PATCH "repos/$repo/issues/$open" --input - --silent
  if [ "$nothing_ahead" = true ]; then
    note="The vendored copy now has everything upstream has (\`$vshort\` is upstream's \`$branch\`). Closing this issue is the owner's."
  else
    note="The report changed: $ahead commit(s) since the vendored \`$vshort\`, upstream at \`$hshort\`"
    [ -z "$newer_tags" ] || note+=", tags $(printf '`%s` ' $newer_tags | sed 's/ $//')"
    [ "$advisory_count" = 0 ] || note+=", **$advisory_count published advisory(ies)**, $new_advisory_count newer than the copy"
    [ "$advisories_ok" = true ] || note+=", advisories **not readable**"
    note+=". The description above is updated."
  fi
  jq -n --arg body "$note" '{body: $body}' | gh api "repos/$repo/issues/$open/comments" --input - --silent
  echo "issue #$open updated"
  exit 0
fi

if [ "$nothing_ahead" = true ]; then
  echo "upstream has nothing beyond the vendored commit; no issue"
  exit 0
fi
last=$(jq -r '.[0].body // ""' "$work/issues.json" | marker)
if [ "$last" = "$state" ]; then
  echo "the last closed issue #$(jq -r '.[0].number' "$work/issues.json") reported the same; nothing written"
  exit 0
fi
assignees='[]'
gh api "repos/$repo/assignees/$OWNER" --silent 2>/dev/null && assignees="[\"$OWNER\"]"
jq -n --arg title "$TITLE" --rawfile body "$work/body.md" --arg lbl "$LABEL" --argjson a "$assignees" \
  '{title: $title, body: $body, labels: [$lbl], assignees: $a}' \
  | gh api "repos/$repo/issues" --input - --jq '"issue #\(.number) opened: \(.html_url)"'
