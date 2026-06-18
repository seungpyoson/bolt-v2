#!/usr/bin/env python3
"""Self-tests for the UsableMu sole-mint fence."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_usable_mu_sole_mint.py")
SPEC = importlib.util.spec_from_file_location(
    "verify_bolt_v3_usable_mu_sole_mint",
    SCRIPT_PATH,
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


class UsableMuSoleMintFenceTests(unittest.TestCase):
    def test_gate_file_mint_is_allowed(self) -> None:
        # The μ health gate is the sole legitimate production mint: minting via
        # `UsableMu::new` (as a call or a `.map` function reference) in the gate
        # file passes.
        violations = VERIFIER.find_violations_in_text(
            VERIFIER.GATE_PATH,
            """
            evaluate_mu_health(&self.health, sample, now_ms)
                .map(|()| estimate_informed_fraction(sample))
                .map(UsableMu::new)
            """,
        )

        self.assertEqual(violations, [])

    def test_call_form_mint_in_another_production_file_fails(self) -> None:
        # A deliberate in-crate mint outside the gate routes an ungated μ around
        # the health checks — the exact bypass the fence seals.
        violations = VERIFIER.find_violations_in_text(
            "src/bolt_v3_maker_quote_plan.rs",
            """
            let bypass = UsableMu::new(raw_mu);
            """,
        )

        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].line, 2)
        self.assertIn("UsableMu::new", violations[0].excerpt)

    def test_function_reference_mint_in_another_production_file_fails(self) -> None:
        # The `.map(UsableMu::new)` function-reference form is also a mint and must
        # fail outside the gate, not only the parenthesised call form.
        violations = VERIFIER.find_violations_in_text(
            "src/bolt_v3_maker_model.rs",
            """
            let wrapped = some_result.map(UsableMu::new);
            """,
        )

        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].line, 2)

    def test_cfg_test_mint_in_another_file_is_allowed(self) -> None:
        # Unit tests legitimately mint a known μ via `UsableMu::new`; `#[cfg(test)]`
        # items are stripped before scanning, so test mints stay legal without a
        # public bypass constructor. A production mint in the same file still fails.
        with tempfile.TemporaryDirectory() as temp_dir:
            probe = Path(temp_dir) / "probe.rs"
            probe.write_text(
                """
                fn production() -> UsableMu {
                    forbidden_helper(UsableMu::new(0.5))
                }

                #[cfg(test)]
                mod tests {
                    fn unit_mint() {
                        let mu = UsableMu::new(0.10);
                        let _wrapped = result.map(UsableMu::new);
                    }
                }
                """,
                encoding="utf-8",
            )

            violations = VERIFIER.find_violations_in_text(
                "src/bolt_v3_maker_quote_plan.rs",
                VERIFIER.production_text(probe),
            )

        self.assertEqual({violation.line for violation in violations}, {3})

    def test_substrings_and_comments_do_not_match(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/probe.rs",
            """
            // UsableMu::new is documented here.
            let s = "UsableMu::new in a string literal";
            let other = UsableMu::new_unchecked(raw);
            let renamed = UsableMuConfig::new(cfg);
            """,
        )

        self.assertEqual(violations, [])

    def test_raw_identifier_mint_is_detected(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/bolt_v3_maker_model.rs",
            """
            let bypass = UsableMu::r#new(raw_mu);
            """,
        )

        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].line, 2)

    def test_empty_source_file_set_fails_closed(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "no Rust source files"):
            VERIFIER.collect_violations_from_files([])

    def test_current_bolt_src_mints_usable_mu_only_in_the_gate(self) -> None:
        self.assertEqual(VERIFIER.collect_violations(), [])


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
