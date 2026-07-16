#!/usr/bin/env python3
"""Mutation tests for the closed evidence-novelty registry verifier."""

from __future__ import annotations

import importlib.util
import pathlib
import sys
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name("verify_bolt_v3_evidence_novelty.py")
SPEC = importlib.util.spec_from_file_location("evidence_novelty_verifier", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


class EvidenceNoveltyVerifierTests(unittest.TestCase):
    def test_repository_registry_and_generated_bytes_match(self) -> None:
        self.assertEqual(VERIFIER.verification_findings(VERIFIER.REPO_ROOT), [])

    def test_unknown_registry_key_is_rejected(self) -> None:
        text = (VERIFIER.REPO_ROOT / VERIFIER.REGISTRY_PATH).read_text(encoding="utf-8")
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "registry.toml"
            path.write_text("unknown = true\n" + text, encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "exactly schema_version"):
                VERIFIER.load_registry(path)

    def test_gap_in_owned_state_ranges_is_rejected(self) -> None:
        text = (VERIFIER.REPO_ROOT / VERIFIER.REGISTRY_PATH).read_text(encoding="utf-8")
        text = text.replace("id_start = 128", "id_start = 129", 1)
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "registry.toml"
            path.write_text(text, encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "contiguous and non-overlapping"):
                VERIFIER.load_registry(path)


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
