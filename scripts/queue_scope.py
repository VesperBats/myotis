#!/usr/bin/env python3
"""Decide whether a merge-queue entry builds an artifact whose workflow takes no path filter.

A check the merge queue requires must report on `pull_request` and on `merge_group` alike, and
`merge_group` takes no `paths:` filter, so a required artifact workflow cannot be path-filtered.
Its build job reads this script's answer instead (through .github/workflows/queue-scope.yml):
in the queue, it builds only when the entry changes a file on the workflow's path list, the list
its `pull_request` trigger was filtered on before the queue.

What the entry changes is the diff from the queue's base (`main`) to the commit the queue tests,
which also holds the entries queued ahead of it: one compare-API call. FAIL-OPEN: an API error, a
file list the API capped (300 files) or anything unexpected means "build". On every event but
`merge_group` the answer is "build" without asking.

Patterns follow GitHub's `paths:` filter: `*` matches within a path segment, `**` across them,
`**/x` also matches a root `x`, and a pattern starting with `!` excludes what earlier ones matched
(the last matching pattern wins).

Environment: EVENT (github.event_name), BASE_SHA and HEAD_SHA (the merge_group's), PATHS (the
patterns, one per line; `#` starts a comment), GH_REPO, GH_TOKEN; GITHUB_OUTPUT gets
`build=true|false`. Tests: scripts/test_queue_scope.py. Standard library and `gh` only.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys

# The compare API lists at most this many files; a list of that length may be truncated.
COMPARE_FILE_CAP = 300

SHA = re.compile(r"[0-9a-f]{40}")


def glob_to_regex(pattern: str) -> re.Pattern[str]:
    """Translates one `paths:` glob into an anchored regular expression."""
    out = []
    i = 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
        elif pattern.startswith("**", i):
            out.append(".*")
            i += 2
        elif pattern[i] == "*":
            out.append("[^/]*")
            i += 1
        elif pattern[i] == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(pattern[i]))
            i += 1
    return re.compile("".join(out) + r"\Z")


def parse_patterns(text: str) -> list[tuple[bool, re.Pattern[str]]]:
    """(include?, regex) per non-empty, non-comment line; a leading `- ` (YAML list) is allowed."""
    patterns = []
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].strip()
        if line.startswith("- "):
            line = line[2:].strip()
        line = line.strip("'\"")
        if not line:
            continue
        negated = line.startswith("!")
        patterns.append((not negated, glob_to_regex(line[1:] if negated else line)))
    return patterns


def matches(path: str, patterns: list[tuple[bool, re.Pattern[str]]]) -> bool:
    """Whether the filter selects the path: the last pattern that matches it decides."""
    selected = False
    for include, regex in patterns:
        if regex.match(path):
            selected = include
    return selected


def decide(files: list[str], truncated: bool, patterns: list[tuple[bool, re.Pattern[str]]]) -> tuple[bool, str]:
    """(build?, why) for a queue entry that changes `files`."""
    if not patterns:
        return True, "no path list was given"
    if truncated:
        return True, f"the compare API listed {len(files)} files, which may be truncated"
    hits = [f for f in files if matches(f, patterns)]
    if hits:
        shown = ", ".join(hits[:5]) + (" …" if len(hits) > 5 else "")
        return True, f"{len(hits)} changed file(s) on the path list: {shown}"
    return False, f"none of the {len(files)} changed file(s) is on the path list"


def compare_files(repo: str, base: str, head: str) -> tuple[list[str], bool]:
    """The files the queue entry changes (renames by both names), and whether the list is capped."""
    # per_page=1: the files come with the first page whatever its size; commits are not needed.
    out = subprocess.run(
        ["gh", "api", f"repos/{repo}/compare/{base}...{head}?per_page=1"],
        check=True, capture_output=True, text=True, timeout=120,
    ).stdout
    listed = json.loads(out).get("files")
    if not isinstance(listed, list):
        raise ValueError("the compare response has no file list")
    files = []
    for f in listed:
        files.append(f["filename"])
        if f.get("previous_filename"):
            files.append(f["previous_filename"])
    return files, len(listed) >= COMPARE_FILE_CAP


def answer(build: bool, why: str) -> None:
    print(("Build: " if build else "Skip: ") + why)
    output = os.environ.get("GITHUB_OUTPUT")
    if output:
        with open(output, "a", encoding="utf-8") as fh:
            fh.write(f"build={'true' if build else 'false'}\n")


def main() -> int:
    event = os.environ.get("EVENT", "")
    if event != "merge_group":
        answer(True, f"not a merge-queue entry ({event or 'no event'})")
        return 0
    try:
        base = os.environ.get("BASE_SHA", "")
        head = os.environ.get("HEAD_SHA", "")
        repo = os.environ.get("GH_REPO", "")
        if not (SHA.fullmatch(base) and SHA.fullmatch(head)):
            raise ValueError(f"unexpected base or head sha ({base!r}, {head!r})")
        if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
            raise ValueError(f"unexpected repository name {repo!r}")
        files, truncated = compare_files(repo, base, head)
        build, why = decide(files, truncated, parse_patterns(os.environ.get("PATHS", "")))
    except Exception as e:  # noqa: BLE001 - fail-open, by design
        print(f"::warning title=Queue scope::{type(e).__name__}: {e}. Building (fail-open).")
        build, why = True, "the scope could not be read"
    answer(build, why)
    return 0


if __name__ == "__main__":
    sys.exit(main())
