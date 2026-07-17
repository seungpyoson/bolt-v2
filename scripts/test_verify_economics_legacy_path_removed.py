#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import tempfile
import unittest

import verify_economics_legacy_path_removed as verifier


class EconomicsLegacyPathRemovedTest(unittest.TestCase):
    def verify_source(self, source: str) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            path = root / "src/strategies/example.rs"
            path.parent.mkdir(parents=True)
            path.write_text(source, encoding="utf-8")
            return verifier.verify(root)

    def test_accepts_sealed_economics_admission(self) -> None:
        errors = self.verify_source(
            "fn submit(admission: EconomicsAdmission) { route(admission); }\n"
        )
        self.assertEqual(errors, [])

    def test_rejects_each_retired_identifier(self) -> None:
        for identifier in verifier.FORBIDDEN_IDENTIFIERS:
            with self.subTest(identifier=identifier):
                errors = self.verify_source(f"fn use_it() {{ {identifier}(); }}\n")
                self.assertTrue(any(identifier in error for error in errors), errors)

    def test_ignores_comments_and_literals(self) -> None:
        errors = self.verify_source(
            '// FeeProvider\nconst NOTE: &str = "resolve_fee_provider";\n'
        )
        self.assertEqual(errors, [])

    def test_rejects_checked_internal_arithmetic_name(self) -> None:
        errors = self.verify_source(
            "checked_fee_inclusive_admission_notional(base, rate);\n"
        )
        self.assertTrue(any("checked_fee_inclusive_admission_notional" in error for error in errors))

    def test_rejects_strategy_owned_fee_state_and_math(self) -> None:
        for source in (
            "let historical_entry_fee_bps = rate;\n",
            "let fee_cost_cents = gross * rate;\n",
            "refresh_fee_readiness();\n",
            "let net = gross * (1.0 - fee_bps / 10_000.0);\n",
        ):
            with self.subTest(source=source):
                errors = self.verify_source(source)
                self.assertTrue(
                    any("strategy-owned economics" in error for error in errors),
                    errors,
                )


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
