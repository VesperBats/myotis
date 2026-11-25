#!/usr/bin/env python3
"""Tests of scripts/queue_scope.py: the `paths:` glob semantics, the decision and its fail-open.

    python3 scripts/test_queue_scope.py
"""

import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parent / "queue_scope.py"
spec = importlib.util.spec_from_file_location("queue_scope", SCRIPT)
qs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qs)

LIST = """
# a comment line
- 'rust/**'
- "!rust/roost/**"
- '**/build.gradle.kts'
- .java-version   # trailing comment
- app-desktop/src/main/kotlin/io/myotis/desktop/Main.kt
- myotis-engine-rust/src/main/kotlin/uniffi/**
- docs/*.md
"""


class Globs(unittest.TestCase):
    def setUp(self):
        self.p = qs.parse_patterns(LIST)

    def test_double_star_crosses_segments(self):
        self.assertTrue(qs.matches("rust/myotis-engine/src/lib.rs", self.p))
        self.assertTrue(qs.matches("myotis-engine-rust/src/main/kotlin/uniffi/a/b.kt", self.p))

    def test_leading_double_star_matches_the_root_too(self):
        self.assertTrue(qs.matches("build.gradle.kts", self.p))
        self.assertTrue(qs.matches("app-desktop/build.gradle.kts", self.p))
        self.assertFalse(qs.matches("app-desktop/build.gradle.kts.bak", self.p))

    def test_single_star_stays_in_its_segment(self):
        self.assertTrue(qs.matches("docs/readme.md", self.p))
        self.assertFalse(qs.matches("docs/guides/readme.md", self.p))

    def test_negation_overrides_an_earlier_match(self):
        self.assertFalse(qs.matches("rust/roost/src/main.rs", self.p))

    def test_literal_paths_are_exact(self):
        self.assertTrue(qs.matches(".java-version", self.p))
        self.assertFalse(qs.matches("x.java-version", self.p))
        self.assertTrue(qs.matches("app-desktop/src/main/kotlin/io/myotis/desktop/Main.kt", self.p))
        self.assertFalse(qs.matches("app-desktop/src/main/kotlin/io/myotis/desktop/Other.kt", self.p))

    def test_dots_are_not_wildcards(self):
        self.assertFalse(qs.matches("Xjava-version", qs.parse_patterns(".java-version")))


class Decide(unittest.TestCase):
    def setUp(self):
        self.p = qs.parse_patterns(LIST)

    def test_a_file_on_the_list_builds(self):
        self.assertTrue(qs.decide(["README.md", "rust/Cargo.lock"], False, self.p)[0])

    def test_nothing_on_the_list_skips(self):
        self.assertFalse(qs.decide(["README.md", "docs/guides/x.md"], False, self.p)[0])

    def test_a_capped_list_builds(self):
        self.assertTrue(qs.decide(["README.md"] * 300, True, self.p)[0])

    def test_an_empty_path_list_builds(self):
        self.assertTrue(qs.decide(["README.md"], False, [])[0])


class Main(unittest.TestCase):
    """The script end to end, with a stub `gh` first on PATH."""

    def run_script(self, event, gh_body=None, gh_exit=0, base="refs/heads/main", head="b" * 40, paths=LIST):
        with tempfile.TemporaryDirectory() as tmp:
            stub = Path(tmp) / "gh"
            body = Path(tmp) / "body.json"
            body.write_text(gh_body or "{}", encoding="utf-8")
            stub.write_text(f"#!/bin/sh\ncat '{body}'\nexit {gh_exit}\n", encoding="utf-8")
            stub.chmod(0o755)
            out = Path(tmp) / "out"
            env = dict(os.environ, PATH=f"{tmp}:{os.environ['PATH']}", EVENT=event, BASE_REF=base,
                       HEAD_SHA=head, GH_REPO="o/r", PATHS=paths, GITHUB_OUTPUT=str(out))
            r = subprocess.run([sys.executable, "-I", str(SCRIPT)], env=env, capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stderr)
            return out.read_text(encoding="utf-8").strip(), r.stdout

    def test_other_events_build_without_asking(self):
        for event in ("push", "pull_request", "workflow_dispatch", ""):
            self.assertEqual(self.run_script(event, gh_exit=1)[0], "build=true")

    def test_queue_entry_on_the_list_builds(self):
        body = '{"files": [{"filename": "README.md"}, {"filename": "rust/x.rs"}]}'
        self.assertEqual(self.run_script("merge_group", body)[0], "build=true")

    def test_queue_entry_off_the_list_skips(self):
        self.assertEqual(self.run_script("merge_group", '{"files": [{"filename": "README.md"}]}')[0], "build=false")

    def test_a_rename_counts_by_its_old_name(self):
        body = '{"files": [{"filename": "attic/x.rs", "previous_filename": "rust/x.rs"}]}'
        self.assertEqual(self.run_script("merge_group", body)[0], "build=true")

    def test_fail_open(self):
        self.assertEqual(self.run_script("merge_group", gh_exit=1)[0], "build=true")
        self.assertEqual(self.run_script("merge_group", "not json")[0], "build=true")
        self.assertEqual(self.run_script("merge_group", '{"message": "Not Found"}')[0], "build=true")
        self.assertEqual(self.run_script("merge_group", '{"files": []}', base="main")[0], "build=true")
        self.assertEqual(self.run_script("merge_group", '{"files": []}', base="refs/heads/../x")[0], "build=true")

    def test_the_compare_cap_builds(self):
        body = '{"files": [' + ",".join(f'{{"filename": "docs/{i}.txt"}}' for i in range(300)) + "]}"
        self.assertEqual(self.run_script("merge_group", body)[0], "build=true")
        body = '{"files": [' + ",".join(f'{{"filename": "docs/{i}.txt"}}' for i in range(299)) + "]}"
        self.assertEqual(self.run_script("merge_group", body)[0], "build=false")

    def test_an_unsupported_pattern_builds(self):
        for pattern in ("docs/?.md", "docs/a+.md", "docs/[ab].md"):
            self.assertEqual(self.run_script("merge_group", '{"files": [{"filename": "x"}]}', paths=pattern)[0],
                             "build=true")


if __name__ == "__main__":
    unittest.main()
