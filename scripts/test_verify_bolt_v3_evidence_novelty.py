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
    def registry_text(self) -> str:
        return (VERIFIER.REPO_ROOT / VERIFIER.REGISTRY_PATH).read_text(encoding="utf-8")

    def load_text(self, text: str):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "registry.toml"
            path.write_text(text, encoding="utf-8")
            return VERIFIER.load_registry(path)

    def test_repository_registry_and_generated_bytes_match(self) -> None:
        self.assertEqual(VERIFIER.verification_findings(VERIFIER.REPO_ROOT), [])

    def test_generated_word_count_uses_typed_capacity_expression(self) -> None:
        registry = VERIFIER.load_registry(VERIFIER.REPO_ROOT / VERIFIER.REGISTRY_PATH)
        generated = VERIFIER.render_registry(registry)
        self.assertIn(
            "pub const EVIDENCE_NOVELTY_WORD_COUNT: usize = 4;",
            generated,
        )
        self.assertIn(
            "EVIDENCE_NOVELTY_FAMILY_CAPACITY.div_ceil(64)",
            generated,
        )
        self.assertNotIn("256.div_ceil(64)", generated)

    def test_producer_must_map_entry_reason_to_canonical_state_before_claim(self) -> None:
        paths = (
            VERIFIER.REGISTRY_PATH,
            VERIFIER.GENERATED_PATH,
            VERIFIER.PRODUCER_PATH,
            VERIFIER.ENTRY_DECISION_PATH,
            VERIFIER.NOVELTY_PATH,
        )
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for relative_path in paths:
                destination = root / relative_path
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_text(
                    (VERIFIER.REPO_ROOT / relative_path).read_text(encoding="utf-8"),
                    encoding="utf-8",
                )
            producer_path = root / VERIFIER.PRODUCER_PATH
            producer_path.write_text(
                producer_path.read_text(encoding="utf-8").replace(
                    "entry_skip_canonical_state(reason_category)",
                    "Ok(EvidenceCanonicalState::EntrySkipEntryPricingBlocked)",
                    1,
                ),
                encoding="utf-8",
            )
            findings = VERIFIER.verification_findings(root)
        self.assertTrue(
            any("entry-skip novelty/payload/append seam incomplete" in item for item in findings),
            findings,
        )

    def test_blocked_snapshot_payload_must_build_before_claim(self) -> None:
        paths = (
            VERIFIER.REGISTRY_PATH,
            VERIFIER.GENERATED_PATH,
            VERIFIER.PRODUCER_PATH,
            VERIFIER.ENTRY_DECISION_PATH,
            VERIFIER.NOVELTY_PATH,
        )
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for relative_path in paths:
                destination = root / relative_path
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_text(
                    (VERIFIER.REPO_ROOT / relative_path).read_text(encoding="utf-8"),
                    encoding="utf-8",
                )
            producer_path = root / VERIFIER.PRODUCER_PATH
            producer = producer_path.read_text(encoding="utf-8")
            payload = (
                "        let snapshot = self."
                "blocked_entry_strategy_input_evidence_snapshot_at(now_ms, decision)?;\n"
            )
            function_start = producer.index(
                "fn record_blocked_entry_strategy_input_snapshot_once("
            )
            function_end = producer.index("\n    fn ", function_start)
            function = producer[function_start:function_end].replace(payload, "", 1)
            append = "        self.context\n            .decision_evidence()"
            function = function.replace(append, payload + append, 1)
            producer = producer[:function_start] + function + producer[function_end:]
            producer_path.write_text(producer, encoding="utf-8")
            findings = VERIFIER.verification_findings(root)
        self.assertTrue(
            any("duplicate check must precede" in item for item in findings),
            findings,
        )

    def test_repository_registry_preserves_frozen_market_allocations(self) -> None:
        registry = VERIFIER.load_registry(VERIFIER.REPO_ROOT / VERIFIER.REGISTRY_PATH)
        actual = tuple(
            (row.name, row.id_start, row.id_end_exclusive)
            for row in getattr(registry, "allocations", ())
        )
        self.assertEqual(
            actual,
            (
                ("discovery_identity", 0, 32),
                ("lifecycle_rollover", 32, 80),
                ("subscription_book", 80, 144),
                ("strategy_input_pricing_blocker", 144, 208),
                ("dependency_health", 208, 240),
                ("terminal_closed_window_skip", 240, 256),
            ),
        )

    def test_registry_capacity_is_frozen(self) -> None:
        text = self.registry_text().replace("capacity = 256", "capacity = 512", 1)
        with self.assertRaisesRegex(ValueError, "family.capacity must match frozen"):
            self.load_text(text)

    def test_repository_registry_assigns_permanent_canonical_ids(self) -> None:
        registry = VERIFIER.load_registry(VERIFIER.REPO_ROOT / VERIFIER.REGISTRY_PATH)
        ids = tuple(getattr(row, "id", None) for row in registry.states)
        self.assertEqual(ids, tuple(range(144, 173)))
        self.assertEqual(len(registry.states), 29)

    def test_permanent_ids_cannot_swap_semantic_meanings(self) -> None:
        text = self.registry_text()
        text = text.replace("id = 146", "id = 999", 1)
        text = text.replace("id = 147", "id = 146", 1)
        text = text.replace("id = 999", "id = 147", 1)
        with self.assertRaisesRegex(
            ValueError, "states must match frozen id-to-semantic mappings"
        ):
            self.load_text(text)

    def test_unassigned_ids_remain_non_emittable(self) -> None:
        registry = VERIFIER.load_registry(VERIFIER.REPO_ROOT / VERIFIER.REGISTRY_PATH)
        ids = {getattr(row, "id", None) for row in registry.states}
        self.assertNotIn(143, ids)
        self.assertNotIn(173, ids)
        self.assertNotIn(255, ids)

    def test_unknown_registry_key_is_rejected(self) -> None:
        text = self.registry_text()
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "registry.toml"
            path.write_text("unknown = true\n" + text, encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "exactly schema_version"):
                VERIFIER.load_registry(path)

    def test_state_outside_named_allocation_is_rejected(self) -> None:
        text = self.registry_text().replace("id = 144", "id = 143", 1)
        with self.assertRaisesRegex(ValueError, "outside allocation"):
            self.load_text(text)

    def test_duplicate_canonical_id_is_rejected(self) -> None:
        text = self.registry_text().replace("id = 145", "id = 144", 1)
        with self.assertRaisesRegex(ValueError, "state ids must be unique"):
            self.load_text(text)

    def test_duplicate_allocation_name_is_rejected(self) -> None:
        text = self.registry_text().replace(
            'name = "lifecycle_rollover"', 'name = "discovery_identity"', 1
        )
        with self.assertRaisesRegex(ValueError, "allocation names must be unique"):
            self.load_text(text)


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
