#!/usr/bin/env bash
# Hold a `v*` tag's later macOS jobs until its first wave is done (owner's
# request, 2026-11-26: "Build android and macos first. I am not waiting for the
# rest.").
#
#   tag-first-wave.sh
#
# A tag push starts about ten macOS jobs at once, and the account runs only a
# few at a time, so the jobs a release is waited for queued behind ones nobody
# waits for: v0.1.24's iOS engine module, which the Maven publish needs, waited
# 43 minutes for a runner. The first wave is
#
#   - "Engine library": the engine's Maven set (host libraries, the iOS module,
#     the Android AAR), which the hosts resolve;
#   - "Desktop DMG (macOS)": the macOS app.
#
# Held: the builds of MyotisKit (ios-framework.yml), ios-xcframework.yml and
# node-binding.yml (its Linux and Windows legs too: one matrix job), and the
# macOS legs of reproducible-builds.yml. Each of those workflows has a
# `first-wave` job on Linux that runs this script on a tag push, and the held
# jobs `need` it, so they enter the queue only once the first wave has
# finished. The Android APK and every other job are not held.
#
# Never a reason for a release to fail: it waits for the first wave to finish,
# whatever its outcome (a failed engine library is its own red run), and once
# $FIRST_WAVE_MINUTES (default 120) of wall-clock time have passed it lets the
# jobs go anyway. The cap keeps a held build inside `verify release`'s
# four-hour wait (a held MyotisKit build takes up to 90 minutes). A listing that
# fails or takes over 60 s is asked again, and so is reading this run's own
# creation time (five tries; then it lets the jobs go, and says so). Only runs
# created with this tag push count (not before this run's own creation, less
# two minutes), so a tag pushed again at the same commit is not answered by its
# earlier runs. The first
# wave's runs are created with the tag push, as this one is; a name not listed
# within 5 minutes is taken as not coming, and said once. The workflows also
# run the held jobs when this job fails (`!cancelled()`), and its steps are
# `continue-on-error`.
#
# Needs GH_TOKEN (actions: read), GH_REPO, GITHUB_SHA, GITHUB_REF_NAME and
# GITHUB_RUN_ID.
set -euo pipefail

FIRST_WAVE=("Engine library" "Desktop DMG (macOS)")
limit="${FIRST_WAVE_MINUTES:-120}"
tag="$GITHUB_REF_NAME"
start=$SECONDS
deadline=$((SECONDS + limit * 60))

# This run's creation, in epoch seconds: the runs of the same push are created
# within moments of it.
own=""
for attempt in 1 2 3 4 5; do
  own=$(timeout 60 gh api "repos/$GH_REPO/actions/runs/$GITHUB_RUN_ID" --jq '.created_at | fromdateiso8601') && break
  own=""
  sleep 15
done
[ -n "$own" ] || { echo "::warning::could not read this run's creation time: not waiting for the first wave of $tag"; exit 0; }
since=$((own - 120))

declare -A warned=()
while [ "$SECONDS" -lt "$deadline" ]; do
  minute=$(( (SECONDS - start) / 60 ))
  if ! runs=$(timeout 60 gh api --paginate \
    "repos/$GH_REPO/actions/runs?head_sha=$GITHUB_SHA&event=push&per_page=100" \
    --jq ".workflow_runs[] | select(.head_branch == \"$tag\" and (.created_at | fromdateiso8601) >= $since) | [.name, .status, (.conclusion // \"\")] | @tsv"); then
    echo "listing the tag's runs failed; asking again"
    sleep 60
    continue
  fi
  waiting=()
  for name in "${FIRST_WAVE[@]}"; do
    line=$(printf '%s\n' "$runs" | awk -F'\t' -v n="$name" '$1 == n' | head -n 1)
    if [ -z "$line" ]; then
      if [ "$minute" -lt 5 ]; then
        waiting+=("$name (not listed yet)")
      elif [ -z "${warned[$name]:-}" ]; then
        echo "::warning::no '$name' run for $tag: not waited for"
        warned[$name]=1
      fi
      continue
    fi
    status=$(printf '%s' "$line" | cut -f2)
    if [ "$status" != "completed" ]; then
      waiting+=("$name ($status)")
    fi
  done
  if [ "${#waiting[@]}" -eq 0 ]; then
    printf '%s\n' "$runs" | awk -F'\t' '$1 == "Engine library" || $1 == "Desktop DMG (macOS)" { print "  " $1 ": " $3 }'
    echo "the first wave of $tag is done after $minute min: going on"
    exit 0
  fi
  echo "minute $minute: waiting for ${waiting[*]}"
  sleep 60
done
echo "::warning::the first wave of $tag is not done after $limit minutes: going on anyway"
