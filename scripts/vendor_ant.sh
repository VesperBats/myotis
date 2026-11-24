#!/usr/bin/env bash
# Vendors ant (https://github.com/freedom-hq/ant), the embedded Swarm node behind
# rust/swarm, into rust/vendor/ant: the crates rust/swarm builds, copied verbatim at one
# commit, so that no build fetches ant from GitHub (owner's decision, 2026-11-20).
#
#   scripts/vendor_ant.sh <commit>   replace rust/vendor/ant with ant at <commit>, patched
#   scripts/vendor_ant.sh --check    compare rust/vendor/ant with upstream at its recorded
#                                    commit plus the patch series
#
# Both fetch ant from GitHub; nothing else in the build does. An update is a PR whose diff
# is ant's own change between the two commits, reviewed like code (rust/vendor/ant/VENDORED.md).
# Two things are not upstream's: the workspace manifest's `members` list names the vendored
# crates instead of all of ant's, and the patch series in rust/vendor/ant-patches
# (NNNN-<name>.patch, applied in order after the copy, each sent upstream as a PR and named
# in VENDORED.md; owner's decision, 2026-11-24). A patch goes when upstream merges its PR:
# at the next update it no longer applies, and the script stops until it is deleted.
set -euo pipefail

REPO=https://github.com/freedom-hq/ant
CRATES=(ant-chain ant-control ant-crypto ant-node ant-p2p ant-postage ant-retrieval)
# contracts/: ant-chain embeds contracts/BzzSwapHelper.bin (include_str!); its .sol source
# comes with it.
FILES=(LICENSE-APACHE LICENSE-MIT contracts)

root=$(cd "$(dirname "$0")/.." && pwd)
dest=$root/rust/vendor/ant
patches_dir=$root/rust/vendor/ant-patches

usage() { echo "usage: $0 <40-hex commit> | --check" >&2; exit 2; }
[ $# -eq 1 ] || usage
mode=update
commit=$1
if [ "$1" = "--check" ]; then
  mode=check
  commit=$(sed -n 's/^Commit: *\([0-9a-f]\{40\}\)$/\1/p' "$dest/VENDORED.md")
  [ -n "$commit" ] || { echo "no 'Commit: <40 hex>' line in $dest/VENDORED.md" >&2; exit 1; }
fi
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || usage

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
src=$work/src
git -C "$work" init -q src
# Upstream's bytes, whatever this machine's git does with line endings (core.autocrlf on
# Windows) and whatever upstream's own .gitattributes say.
git -C "$src" config core.autocrlf false
git -C "$src" config core.eol lf
echo '* -text' > "$src/.git/info/attributes"
git -C "$src" remote add origin "$REPO"

# The commit must be on upstream's default branch. GitHub serves any commit of a
# repository's fork network through the parent's URL, so a fetch by SHA alone would also
# take a commit that exists only in somebody's fork, or on a branch upstream never merged.
# The branch's history is fetched without trees (commits only: small).
branch=$(git -C "$src" ls-remote --symref origin HEAD | sed -n 's|^ref: refs/heads/\([^[:space:]]*\)[[:space:]]*HEAD$|\1|p')
[ -n "$branch" ] || { echo "cannot read $REPO's default branch" >&2; exit 1; }
git -C "$src" fetch -q --filter=tree:0 origin "+refs/heads/${branch}:refs/remotes/origin/${branch}"
if ! git -C "$src" merge-base --is-ancestor "$commit" "origin/${branch}" 2>/dev/null; then
  echo "$commit is not on $REPO's $branch: a commit only in a fork, or on a branch upstream" >&2
  echo "never merged, is not upstream's ant" >&2
  exit 1
fi
git -C "$src" fetch -q --no-filter --depth 1 origin "$commit"
git -C "$src" -c advice.detachedHead=false checkout -q FETCH_HEAD
[ "$(git -C "$src" rev-parse HEAD)" = "$commit" ] || { echo "fetched another commit" >&2; exit 1; }

# The tree as it is committed here: the crates and licenses verbatim, the workspace
# manifest with only the vendored crates as members.
out=$work/out
mkdir -p "$out/crates"
for c in "${CRATES[@]}"; do
  cp -R "$src/crates/$c" "$out/crates/$c"
done
for f in "${FILES[@]}"; do
  cp -R "$src/$f" "$out/$f"
done
members=$(printf '  "crates/%s",\n' "${CRATES[@]}")
python3 - "$src/Cargo.toml" "$out/Cargo.toml" "$members" <<'EOF'
import re, sys
src, dst, members = sys.argv[1], sys.argv[2], sys.argv[3]
text = open(src, 'rb').read()
lines = members.rstrip('\n').encode()
new, n = re.subn(rb'(?ms)^members = \[\n.*?^\]', b'members = [\n' + lines + b'\n]', text, count=1)
if n != 1:
    sys.exit("ant's Cargo.toml has no `members = [ ... ]` list in the expected form")
open(dst, 'wb').write(new)
EOF

# The patch series, in order: rust/vendor/ant-patches/NNNN-<name>.patch and nothing else
# there, each named in VENDORED.md. --check reads what git tracks, so the series must have
# no unstaged or untracked change either (the tree comparison below has the same rule).
if [ "$mode" = check ] && [ -d "$patches_dir" ]; then
  if ! git -C "$root" diff --quiet -- rust/vendor/ant-patches \
     || [ -n "$(git -C "$root" ls-files --others --exclude-standard -- rust/vendor/ant-patches)" ]; then
    git -C "$root" status --short -- rust/vendor/ant-patches >&2
    echo "rust/vendor/ant-patches has unstaged or untracked changes (above): --check compares what git tracks" >&2
    exit 1
  fi
fi
patches=()
if [ -d "$patches_dir" ]; then
  for f in "$patches_dir"/* "$patches_dir"/.[!.]*; do
    [ -e "$f" ] || continue
    name=$(basename "$f")
    if ! [[ -f "$f" && "$name" =~ ^[0-9]{4}-[a-z0-9-]+\.patch$ ]]; then
      echo "rust/vendor/ant-patches/$name: the series holds only NNNN-<name>.patch files" >&2
      exit 1
    fi
    if ! grep -qF "ant-patches/$name" "$dest/VENDORED.md" 2>/dev/null; then
      echo "rust/vendor/ant-patches/$name is not described in rust/vendor/ant/VENDORED.md" >&2
      exit 1
    fi
    patches+=("$f")
  done
fi
# A git directory of its own for `git apply`, so that paths are taken relative to $out
# whatever repository the temporary directory may sit in; bytes as they are (no autocrlf,
# no whitespace fixing), and no fuzz: a patch whose context is not exactly there fails.
git init -q --bare "$work/apply.git"
git --git-dir="$work/apply.git" config core.autocrlf false
for f in ${patches[@]+"${patches[@]}"}; do
  if ! (cd "$out" && git --git-dir="$work/apply.git" --work-tree=. apply --whitespace=nowarn "$f"); then
    echo "rust/vendor/ant-patches/$(basename "$f") does not apply to ant at $commit." >&2
    echo "If upstream has merged its PR, delete the patch and its entry in VENDORED.md;" >&2
    echo "otherwise rebase it onto $commit." >&2
    exit 1
  fi
done

# Instructions for coding agents have no place in a dependency: Claude Code loads a nested
# CLAUDE.md when it reads files below it, so one in the copy would instruct every session
# that works there (and .claude/agent/pull-requests.md notes that files below the root are
# not restored from the base branch). None is in the vendored commit; a later commit that
# adds one is refused, so that the update PR decides about it in the open.
agent_files=$(cd "$out" && find . \( -name CLAUDE.md -o -name CLAUDE.local.md -o -name AGENTS.md \
  -o -name GEMINI.md -o -name .claude -o -name .mcp.json -o -name .cursor -o -name .cursorrules \) -print)
if [ -n "$agent_files" ]; then
  echo "ant at $commit has agent instruction files in the vendored crates:" >&2
  printf '  %s\n' $agent_files >&2
  echo "decide about them in the update PR: this script refuses them as it stands" >&2
  exit 1
fi

if [ "$mode" = check ]; then
  # Cargo builds the working tree, the comparison below reads the index: an unstaged edit
  # of a tracked file would pass it while being built.
  if ! git -C "$root" diff --quiet -- rust/vendor/ant; then
    git -C "$root" diff --stat -- rust/vendor/ant >&2
    echo "rust/vendor/ant has unstaged changes (above): --check compares what git tracks" >&2
    exit 1
  fi
  # What git tracks (the index: a commit in CI, staged changes locally), not the working
  # tree: build leftovers there (a target/, a Cargo.lock from building inside the copy) are
  # not part of the copy. File modes count, and the top-level VENDORED.md, ours, does not.
  tracked=$work/tracked
  (cd "$root" && git ls-files -z -- rust/vendor/ant | git checkout-index -z --stdin --prefix="$tracked/")
  rm -f "$tracked/rust/vendor/ant/VENDORED.md"
  if git -c core.autocrlf=false diff --no-index --stat --exit-code "$out" "$tracked/rust/vendor/ant"; then
    echo "rust/vendor/ant matches $REPO at $commit with its ${#patches[@]} patch(es)"
  else
    echo "rust/vendor/ant differs from $REPO at $commit with its ${#patches[@]} patch(es) (above)" >&2
    exit 1
  fi
  exit 0
fi

notes=""
[ -f "$dest/VENDORED.md" ] && notes=$(cat "$dest/VENDORED.md")
rm -rf "$dest"
mkdir -p "$(dirname "$dest")"
cp -R "$out" "$dest"
if [ -n "$notes" ]; then
  printf '%s\n' "$notes" | sed "s/^Commit: *[0-9a-f]\{40\}$/Commit: $commit/" > "$dest/VENDORED.md"
else
  echo "Commit: $commit" > "$dest/VENDORED.md"
fi
grep -qx "Commit: $commit" "$dest/VENDORED.md" \
  || { echo "VENDORED.md does not record Commit: $commit; fix its Commit line by hand" >&2; exit 1; }
# Myotis's ignore rules are unanchored (build/, out/, *.log, nodekey, target/, ...), so
# `git add` would leave an upstream file of such a name out of the commit without a word.
# Right after the copy the directory holds no build leftovers: every hit is upstream's.
ignored=$(git -C "$root" ls-files --others --ignored --exclude-standard -- rust/vendor/ant)
if [ -n "$ignored" ]; then
  echo "these upstream files match a Myotis ignore rule; add them with git add -f:" >&2
  printf '  %s\n' $ignored >&2
  exit 1
fi
echo "rust/vendor/ant is now $REPO at $commit with ${#patches[@]} patch(es); review the diff, then update rust/swarm/Cargo.lock"
