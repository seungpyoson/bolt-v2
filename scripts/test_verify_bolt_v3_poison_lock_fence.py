#!/usr/bin/env python3
"""Tests for verify_bolt_v3_poison_lock_fence.py."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_poison_lock_fence.py")
SPEC = importlib.util.spec_from_file_location("verify_bolt_v3_poison_lock_fence", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
FENCE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = FENCE
SPEC.loader.exec_module(FENCE)


class PoisonLockFenceTests(unittest.TestCase):
    def test_clean_source_produces_no_violations(self) -> None:
        self.assertEqual(
            FENCE.find_violations_in_text(
                "src/bolt_v3_live_node.rs",
                'let feed = feed.lock().expect("capital admission lock poisoned");\n',
            ),
            [],
        )

    def test_detects_production_poison_recovery(self) -> None:
        violations = FENCE.find_violations_in_text(
            "src/bolt_v3_live_node.rs",
            "let feed = feed.lock().unwrap_or_else(|poisoned| poisoned.into_inner());\n",
        )

        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].path, "src/bolt_v3_live_node.rs")
        self.assertEqual(violations[0].line, 1)

    def test_allows_src_tests_path_poison_recovery(self) -> None:
        self.assertEqual(
            FENCE.find_violations_in_text(
                "src/bolt_v3_live_node/tests/startup_rebuild.rs",
                "let feed = feed.lock().unwrap_or_else(|poisoned| poisoned.into_inner());\n",
            ),
            [],
        )

    def test_allowlist_is_empty_at_merge(self) -> None:
        self.assertEqual(FENCE.ALLOWLIST, frozenset())

    def test_collect_violations_scans_src_tree(self) -> None:
        original_root = FENCE.REPO_ROOT
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            source = root / "src" / "bolt_v3_live_node.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "let feed = feed.lock().unwrap_or_else(|poisoned| poisoned.into_inner());\n",
                encoding="utf-8",
            )
            FENCE.REPO_ROOT = root
            try:
                violations = FENCE.collect_violations()
            finally:
                FENCE.REPO_ROOT = original_root

        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].path, "src/bolt_v3_live_node.rs")


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
