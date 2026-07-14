#!/usr/bin/env python3
"""Regression tests for the closed #1354 evidence registry verifier."""

from __future__ import annotations

import pathlib
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]
VERIFIER = ROOT / "scripts/verify_bolt_v3_evidence_registry.py"
REGISTRY = ROOT / "ci/bolt-v3-evidence-registry.toml"


class EvidenceRegistryVerifierTests(unittest.TestCase):
    def run_verifier(self, registry_text: str | None = None) -> subprocess.CompletedProcess[str]:
        if registry_text is None:
            path = REGISTRY
            temp = None
        else:
            temp = tempfile.TemporaryDirectory()
            path = pathlib.Path(temp.name) / "registry.toml"
            path.write_text(registry_text, encoding="utf-8")
        try:
            return subprocess.run(
                ["python3", str(VERIFIER), "--registry", str(path)],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )
        finally:
            if temp is not None:
                temp.cleanup()

    def test_repository_registry_and_source_census_are_complete(self) -> None:
        result = self.run_verifier()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("evidence registry verified", result.stdout)

    def test_unknown_row_field_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        mutated = source.replace(
            'schema_version = 1\n',
            'schema_version = 1\nunknown_authority = "forbidden"\n',
            1,
        )
        result = self.run_verifier(mutated)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unknown registry keys", result.stderr)

    def test_unknown_producer_row_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        mutated = source.replace(
            'method = "record_order_intent"',
            'method = "record_unknown_evidence"',
            1,
        )
        self.assertNotEqual(source, mutated)
        result = self.run_verifier(mutated)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("producer census mismatch", result.stderr)

    def test_duplicate_family_id_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        first = source.index("[[producer]]")
        second = source.index("[[producer]]", first + 1)
        first_row = source[first:second]
        family = next(line for line in first_row.splitlines() if line.startswith("family = "))
        state_id = next(line for line in first_row.splitlines() if line.startswith("state_id = "))
        tail = source[second:]
        tail = tail.replace(
            next(line for line in tail.splitlines() if line.startswith("family = ")),
            family,
            1,
        ).replace(
            next(line for line in tail.splitlines() if line.startswith("state_id = ")),
            state_id,
            1,
        )
        result = self.run_verifier(source[:second] + tail)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("duplicate family/id", result.stderr)

    def test_recovery_bearing_suppression_is_rejected(self) -> None:
        source = REGISTRY.read_text(encoding="utf-8")
        marker = 'recovery_bearing = true\nsuppression = "unsuppressed"'
        self.assertIn(marker, source)
        mutated = source.replace(
            marker,
            'recovery_bearing = true\nsuppression = "finite-episode"',
            1,
        )
        result = self.run_verifier(mutated)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("recovery-bearing producer", result.stderr)

    def test_identity_type_excludes_forbidden_volatile_fields(self) -> None:
        result = self.run_verifier()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        identity_source = (ROOT / "src/bolt_v3_evidence_identity.rs").read_text(
            encoding="utf-8"
        )
        episode_struct = identity_source.split("pub struct EvidenceEpisodeId", 1)[1].split(
            "}", 1
        )[0]
        for forbidden in (
            "price",
            "timestamp",
            "slug",
            "window",
            "diagnostic",
            "retry",
            "schema",
            "config",
            "deployment",
        ):
            self.assertNotIn(forbidden, episode_struct.lower())

    def test_nested_identity_type_rejects_forbidden_volatile_field(self) -> None:
        identity = (ROOT / "src/bolt_v3_evidence_identity.rs").read_text(
            encoding="utf-8"
        )
        mutated_identity = identity.replace(
            "    condition_id: NonEmptyEvidenceIdentity,",
            "    condition_id: NonEmptyEvidenceIdentity,\n    observed_price: u64,",
            1,
        )
        self.assertNotEqual(identity, mutated_identity)
        with tempfile.TemporaryDirectory() as temp_dir:
            identity_path = pathlib.Path(temp_dir) / "identity.rs"
            identity_path.write_text(mutated_identity, encoding="utf-8")
            registry = REGISTRY.read_text(encoding="utf-8").replace(
                'identity_module = "src/bolt_v3_evidence_identity.rs"',
                f'identity_module = "{identity_path}"',
                1,
            )
            result = self.run_verifier(registry)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("forbidden volatile fields", result.stderr)


if __name__ == "__main__":
    unittest.main()
