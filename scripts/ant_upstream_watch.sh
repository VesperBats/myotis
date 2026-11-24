#!/usr/bin/env bash
# Watches upstream ant (https://github.com/freedom-hq/ant) for what the vendored copy in
# rust/vendor/ant does not have yet, and reports it to the owner as ONE tracking issue
# (owner's decision, 2026-11-24): ant is on no registry, so no advisory database names it,
# and a security fix in it reaches Myotis only when the copy is moved
# (rust/vendor/ant/VENDORED.md, "Watching upstream"). Run daily by
# .github/workflows/ant-upstream-watch.yml.
#
#   scripts/ant_upstream_watch.sh            compare, then open or update the issue
#   scripts/ant_upstream_watch.sh --dry-run  compare and print the report; write nothing,
#                                            and fail if a read the report needs failed
#
# What it compares, against the commit VENDORED.md records (the same `Commit:` line that
# scripts/vendor_ant.sh reads):
#   - the commits on upstream's default branch since that commit (count, the newest few,
#     the ones whose message reads like a security fix) and whether they touch the
#     vendored crates;
#   - upstream's tags and releases on those commits;
#   - upstream's published GitHub security advisories, or a note that they are not
#     readable;
#   - the state of the three upstream issues that keep `swarm` out of a release build.
#
# The issue: label `ant-upstream`, assigned to the owner; only issues whose body starts
# with this script's marker are its own. Two levels of news:
#   - the facts that notify: the vendored commit, whether upstream knows it or is behind
#     it, the tags, the commits whose message reads like a security fix, the advisories
#     (with their updated_at), the three issues' states, and whether the advisories are
#     readable. A change comments on the open issue first, naming each fact that changed,
#     then rewrites its body; with no open issue of its own, it opens one, unless the last
#     closed one already reported the same facts (the owner closed it on purpose).
#   - everything else (upstream's head moved by commits that match nothing above): the
#     open issue's body is refreshed without a comment; a closed one stays closed.
# Unchanged writes nothing. It never closes an issue and never re-vendors: both are the
# owner's (re-vendoring is `scripts/vendor_ant.sh <commit>`).
#
# A read of upstream that fails is retried; if it still fails, the run ends red without
# writing, and the next day's run tries again. Only upstream's own answers become facts:
# a 404 from the compare view (upstream does not know the vendored commit), and a 4xx
# from the advisories endpoint (not readable with this token, nor without one).
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
MARKER_PREFIX="<!-- ant-upstream-watch "
# The upstream issues `rust/swarm/src/lib.rs` names as the reasons the swarm feature is off.
BLOCKERS=(269 270 271)
# What makes a commit message read like a security fix. A hint for the owner, not a
# verdict: a fix can be silent, and a match can be harmless.
SECURITY_RE='secur|vulnerab|cve-|ghsa-|rustsec|advisor|exploit|attack|malicious|\bdos\b|denial|overflow|underflow|out[- ]of[- ]bounds|\boob\b|panic|crash|unsound|unsafe|sanitiz|validat|untrusted|bypass|leak|amplif|exhaust|\boom\b|unbounded'
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

# up <errfile> <gh api args...>: a read of upstream, tried three times. Its stderr (gh
# prints "HTTP 404" and the like there) is kept in <errfile> for the caller to tell
# upstream's answer from a failed call.
up() {
  local err=$1; shift
  local i
  for i in 1 2 3; do
    if gh api "$@" 2>"$err"; then return 0; fi
    grep -q 'HTTP 4[0-9][0-9]' "$err" && ! grep -q 'HTTP 429' "$err" && return 1
    [ "$i" = 3 ] || sleep $((i * 20))
  done
  return 1
}
# must <gh api args...>: a read the report cannot do without; a failure ends the run.
must() {
  up "$work/err" "$@" || { echo "::error::reading $UPSTREAM failed: $(tr '\n' ' ' < "$work/err")" >&2; exit 1; }
}

branch=$(must "repos/$UPSTREAM" --jq .default_branch)
head=$(must "repos/$UPSTREAM/commits/$branch" --jq .sha)

# The compare view pages its commits (oldest first); --slurp joins the pages. A vendored
# commit upstream does not know (404: a rewritten history) is a fact for the issue.
compare_ok=true
if ! up "$work/compare.err" --paginate --slurp "repos/$UPSTREAM/compare/$vendored...$head?per_page=100" \
    > "$work/compare.json"; then
  grep -q 'HTTP 404' "$work/compare.err" \
    || { echo "::error::the compare view failed: $(tr '\n' ' ' < "$work/compare.err")" >&2; exit 1; }
  compare_ok=false
  echo "::warning::$UPSTREAM does not know the vendored commit $vendored"
  echo '[{"status": "unknown", "ahead_by": 0, "behind_by": 0, "commits": [], "files": []}]' > "$work/compare.json"
fi
status=$(jq -r '.[0].status' "$work/compare.json")
ahead=$(jq -r '.[0].ahead_by' "$work/compare.json")
behind=$(jq -r '.[0].behind_by' "$work/compare.json")
vendored_date=""
[ "$compare_ok" = false ] || vendored_date=$(must "repos/$UPSTREAM/commits/$vendored" --jq .commit.committer.date)
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
must --paginate "repos/$UPSTREAM/tags?per_page=100" --jq '.[] | [.name, .commit.sha] | @tsv' > "$work/tags.tsv"
newer_tags=$(awk -F'\t' 'NR==FNR { s[$1]; next } ($2 in s) { print $1 }' "$work/shas.txt" "$work/tags.tsv" | sort -V)
must --paginate "repos/$UPSTREAM/releases?per_page=100" \
  --jq '.[] | select(.draft | not) | [.tag_name, .published_at, (.prerelease|tostring), .html_url] | @tsv' > "$work/releases.tsv"
newer_releases=$(awk -F'\t' 'NR==FNR { t[$1]; next } ($1 in t)' <(printf '%s\n' "$newer_tags") "$work/releases.tsv")

# Published advisories: public to anyone, but read with a token scoped to another
# repository, so an unauthenticated read is tried before calling them not readable. Only a
# 4xx from both is that answer; anything else ends the run (retried tomorrow).
advisories_ok=true
adv_path="repos/$UPSTREAM/security-advisories?state=published&per_page=100"
if ! up "$work/adv.err" "$adv_path" > "$work/adv.json"; then
  grep -q 'HTTP 4[0-9][0-9]' "$work/adv.err" \
    || { echo "::error::the advisories read failed: $(tr '\n' ' ' < "$work/adv.err")" >&2; exit 1; }
  code=$(curl -sSL -o "$work/adv.json" -w '%{http_code}' --retry 2 -H 'Accept: application/vnd.github+json' \
    "https://api.github.com/$adv_path" 2>>"$work/adv.err") || code=000
  if [ "$code" != 200 ]; then
    case "$code" in
      4??) ;;
      *) echo "::error::the advisories read failed (HTTP $code): $(tr '\n' ' ' < "$work/adv.err")" >&2; exit 1 ;;
    esac
    advisories_ok=false
    echo '[]' > "$work/adv.json"
    echo "::warning::$UPSTREAM's security advisories are not readable: $(tr '\n' ' ' < "$work/adv.err") (HTTP $code)"
  fi
fi
advisory_count=$(jq 'length' "$work/adv.json")
# New to the copy: published or revised after the vendored commit (all of them when its
# date is unknown). An older one is listed but is no reason to report.
new_advisory_count=$(jq --arg d "$vendored_date" '[.[] | select($d == "" or .updated_at > $d or .published_at > $d)] | length' "$work/adv.json")

blockers=""
blocker_facts=""
for n in "${BLOCKERS[@]}"; do
  line=$(must "repos/$UPSTREAM/issues/$n" \
    --jq '"\(.state)\t- [\(.state)] [\(.html_url | sub("https://github.com/"; "") | sub("/issues/"; "#"))](\(.html_url)) `\(.title | gsub("`"; "'"'"'"))`"')
  blocker_facts+="$UPSTREAM#$n is ${line%%$'\t'*}"$'\n'
  blockers+="${line#*$'\t'}"$'\n'
done

security_commits=$(jq -r --arg re "$SECURITY_RE" \
  '.[] | select(.commit.message | test($re; "i")) | "- [`\(.sha[0:12])`](\(.html_url)) `\(.commit.message | split("\n")[0] | gsub("`"; "'"'"'"))`"' \
  "$work/commits.json")
newest=$(jq -r --argjson n "$NEWEST" \
  'reverse | .[0:$n][] | "- [`\(.sha[0:12])`](\(.html_url)) \(.commit.committer.date[0:10]) `\(.commit.message | split("\n")[0] | gsub("`"; "'"'"'"))`"' \
  "$work/commits.json")

# The facts that notify, one per line (plain words: a changed one is quoted in the
# comment), and the full state, which adds upstream's head.
{
  echo "vendored commit ${vendored:0:12}"
  [ "$compare_ok" = true ] || echo "upstream does not know the vendored commit"
  [ "$behind" = 0 ] || echo "the vendored commit is not on $branch (behind by $behind)"
  for t in $newer_tags; do echo "tag $t"; done
  jq -r --arg re "$SECURITY_RE" '.[] | select(.commit.message | test($re; "i")) | "security-hint commit \(.sha[0:12])"' "$work/commits.json"
  jq -r '.[] | "advisory \(.ghsa_id) updated \(.updated_at)"' "$work/adv.json" | sort
  [ "$advisories_ok" = true ] || echo "advisories not readable"
  printf '%s' "$blocker_facts"
} | tr -d '>' > "$work/facts.txt"   # no '>' can end the HTML comment they are kept in
notify=$(sha256sum < "$work/facts.txt" | cut -c1-16)
state=$({ cat "$work/facts.txt"; echo "head $head"; } | sha256sum | cut -c1-16)
nothing_ahead=false
if [ "$compare_ok" = true ] && [ "$ahead" = 0 ] && [ "$behind" = 0 ] && [ -z "$newer_tags" ] \
    && [ "$new_advisory_count" = 0 ]; then
  nothing_ahead=true
fi

today=$(date -u +%Y-%m-%d)
vshort=${vendored:0:12}
hshort=${head:0:12}
{
  echo "${MARKER_PREFIX}state=$state notify=$notify -->"
  echo "Reported by the scheduled [ant upstream watch](https://github.com/${repo:-VesperBats/myotis}/actions/workflows/ant-upstream-watch.yml), last written on $today (\`scripts/ant_upstream_watch.sh\`; owner's decision, 2026-11-24). Re-vendoring, and whether a release follows, are the owner's; this workflow never re-vendors and never closes this issue. It comments when a tag, a security-hinted commit, an advisory or one of the issues below changes; other upstream commits only refresh this description."
  echo
  echo "## Commits"
  echo
  echo "Vendored: [\`$vshort\`](https://github.com/$UPSTREAM/commit/$vendored) (\`rust/vendor/ant/VENDORED.md\`). Upstream \`$branch\`: [\`$hshort\`](https://github.com/$UPSTREAM/commit/$head)."
  echo
  if [ "$compare_ok" = false ]; then
    echo "**Upstream does not know the vendored commit** (the compare view answers 404): upstream rewrote its history, or the copy is not upstream's. Look at it by hand."
  else
    if [ "$behind" != 0 ]; then
      echo "**The vendored commit is not on \`$branch\`** (behind by $behind): upstream rewrote its history, or the copy is not upstream's."
      echo
    fi
    if [ "$nothing_ahead" = true ]; then
      echo "Upstream has nothing the vendored copy does not: no commit, tag or advisory newer than it."
    elif [ "$ahead" = 0 ]; then
      echo "No commit on \`$branch\` since the vendored commit."
    else
      echo "**$ahead commit(s)** on \`$branch\` since the vendored commit ([compare](https://github.com/$UPSTREAM/compare/$vendored...$head))."
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
      echo "Messages that read like a security fix (a keyword hint over the whole message, not a verdict):"
      echo
      if [ -n "$security_commits" ]; then echo "$security_commits"; else echo "- none"; fi
      echo
      echo "Newest $( [ "$ahead" -lt "$NEWEST" ] && echo "$ahead" || echo "$NEWEST" ):"
      echo
      echo "$newest"
    fi
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
    echo "**Not readable** (\`GET /repos/$UPSTREAM/security-advisories?state=published\` answers an error, with the job's token and without one): look at https://github.com/$UPSTREAM/security/advisories by hand."
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
  echo
  echo "<!-- ant-upstream-watch-facts"
  cat "$work/facts.txt"
  echo "-->"
} > "$work/body.md"

if [ "$dry_run" = true ]; then
  echo "state=$state notify=$notify nothing_ahead=$nothing_ahead compare_ok=$compare_ok ahead=$ahead behind=$behind tags=[$(echo $newer_tags)] advisories=$advisory_count new_advisories=$new_advisory_count advisories_ok=$advisories_ok"
  echo "----- issue body -----"
  cat "$work/body.md"
  # A dry run is the proof that every read works with the job's token: a read that was
  # answered with an error fails it, though a real run would report it.
  [ "$compare_ok" = true ] && [ "$advisories_ok" = true ] || { echo "::error::a read failed (see above)" >&2; exit 1; }
  exit 0
fi

field() { sed -n "s/^${MARKER_PREFIX}.*$1=\\([0-9a-f]*\\).*/\\1/p" | head -n1; }
old_facts() { sed -n '/^<!-- ant-upstream-watch-facts$/,/^-->$/p' | sed '1d;$d'; }

if ! gh api "repos/$repo/labels/$LABEL" --silent 2>/dev/null; then
  gh api "repos/$repo/labels" -f name="$LABEL" -f color=d93f0b \
    -f description="Upstream ant is ahead of rust/vendor/ant (scheduled watch)" --silent
fi
# Only the watch's own issues: a hand-written issue with the label is never rewritten.
gh api "repos/$repo/issues?labels=$LABEL&state=all&sort=created&direction=desc&per_page=50" \
  --jq "[.[] | select(.pull_request | not) | select((.body // \"\") | startswith(\"$MARKER_PREFIX\"))]" \
  > "$work/issues.json"
open=$(jq -r '[.[] | select(.state == "open")][0].number // empty' "$work/issues.json")

if [ -n "$open" ]; then
  jq -r --argjson n "$open" '.[] | select(.number == $n) | .body // ""' "$work/issues.json" > "$work/old.md"
  if [ "$(field state < "$work/old.md")" = "$state" ]; then
    echo "issue #$open is current; nothing written"
    exit 0
  fi
  if [ "$(field notify < "$work/old.md")" != "$notify" ]; then
    # The comment first: it is what notifies. If the body write then fails, the next run
    # comments again (at least once), rather than never.
    old_facts < "$work/old.md" | sort > "$work/old_facts.txt"
    sort "$work/facts.txt" > "$work/new_facts.txt"
    {
      echo "The report changed (upstream \`$branch\` at \`$hshort\`, $ahead commit(s) beyond the vendored \`$vshort\`):"
      echo
      comm -13 "$work/old_facts.txt" "$work/new_facts.txt" | sed 's/^/- new: /'
      comm -23 "$work/old_facts.txt" "$work/new_facts.txt" | sed 's/^/- gone: /'
      [ "$nothing_ahead" = false ] || { echo; echo "The vendored copy now has everything upstream has. Closing this issue is the owner's."; }
      echo
      echo "The description above is updated."
    } > "$work/comment.md"
    jq -n --rawfile body "$work/comment.md" '{body: $body}' | gh api "repos/$repo/issues/$open/comments" --input - --silent
    echo "issue #$open: commented"
  fi
  jq -n --rawfile body "$work/body.md" '{body: $body}' | gh api -X PATCH "repos/$repo/issues/$open" --input - --silent
  echo "issue #$open: description updated"
  exit 0
fi

if [ "$nothing_ahead" = true ]; then
  echo "upstream has nothing beyond the vendored commit; no issue"
  exit 0
fi
if [ "$(jq -r '.[0].body // ""' "$work/issues.json" | field notify)" = "$notify" ]; then
  echo "the last closed issue #$(jq -r '.[0].number' "$work/issues.json") reported the same facts; nothing written"
  exit 0
fi
assignees='[]'
gh api "repos/$repo/assignees/$OWNER" --silent 2>/dev/null && assignees="[\"$OWNER\"]"
jq -n --arg title "$TITLE" --rawfile body "$work/body.md" --arg lbl "$LABEL" --argjson a "$assignees" \
  '{title: $title, body: $body, labels: [$lbl], assignees: $a}' \
  | gh api "repos/$repo/issues" --input - --jq '"issue #\(.number) opened: \(.html_url)"'
