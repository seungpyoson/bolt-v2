#!/usr/bin/env python3
"""Non-compile properties for the finite #1354 production novelty boundary."""

from __future__ import annotations

import itertools
import pathlib
import tomllib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]
REGISTRY = ROOT / "ci/bolt-v3-evidence-registry.toml"
AUTHORITY = ROOT / "src/bolt_v3_decision_evidence.rs"


def non_recovery_domains() -> list[tuple[str, list[tuple[str, ...]], int]]:
    with REGISTRY.open("rb") as handle:
        registry = tomllib.load(handle)
    return [
        (
            row["name"],
            [tuple(axis) for axis in row["canonical_state_axes"]],
            row["max_emissions"],
        )
        for row in registry["producer"]
        if not row["recovery_bearing"]
    ]


class EvidenceNoveltySourceProperties(unittest.TestCase):
    def test_every_non_recovery_domain_has_fixed_large_oscillation_bound(self) -> None:
        for name, axes, maximum in non_recovery_domains():
            with self.subTest(producer=name):
                states = list(itertools.product(*axes))
                self.assertEqual(len(states), maximum)
                sequence = [states[index % len(states)] for index in range(100_000)]
                retained: set[tuple[str, ...]] = set()
                emissions = 0
                for state in sequence:
                    if state not in retained:
                        retained.add(state)
                        emissions += 1
                self.assertEqual(emissions, maximum)
                self.assertEqual(len(retained), maximum)

    def test_a_b_a_and_persistent_writer_failure_claim_each_state_once(self) -> None:
        for name, axes, maximum in non_recovery_domains():
            with self.subTest(producer=name):
                states = list(itertools.product(*axes))
                a = states[0]
                b = states[-1]
                retained: set[tuple[str, ...]] = set()
                failed_write_attempts = 0
                for state in itertools.islice(itertools.cycle((a, b, a)), 100_000):
                    if state in retained:
                        continue
                    retained.add(state)  # production mark occurs before fallible I/O
                    failed_write_attempts += 1
                expected = 1 if maximum == 1 else 2
                self.assertEqual(failed_write_attempts, expected)
                self.assertEqual(len(retained), expected)

    def test_mechanical_maximum_and_production_storage_contract(self) -> None:
        domains = non_recovery_domains()
        self.assertEqual(len(domains), 10)
        self.assertEqual(sum(maximum for _, _, maximum in domains), 117)
        source = AUTHORITY.read_text(encoding="utf-8")
        self.assertIn(
            "const _: [(); 6] = [(); std::mem::size_of::<AtomicU16>() * 3];",
            source,
        )
        self.assertIn(
            "const _: [(); 117] = [(); BOLT_V3_NON_RECOVERY_MAX_EMISSIONS as usize];",
            source,
        )
        self.assertNotIn("#[cfg(test)]\nstatic", source)
        self.assertNotIn("is_power_of_two", source)
        self.assertNotIn("evict_oldest", source)


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
