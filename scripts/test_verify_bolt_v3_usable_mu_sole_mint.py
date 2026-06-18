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


# A minimal gate file whose only `UsableMu::new` mint is inside `usable_mu_for`,
# mirroring the real src/strategies/binary_oracle_maker/mu.rs shape (multi-line
# signature, mint via `.map`).
GATE_FILE_OK = """
impl MakerMuState {
    pub fn usable_mu_for(
        &self,
        instrument_id: &InstrumentId,
        now_ms: u64,
    ) -> Result<UsableMu, MuHealthReason> {
        match self.health_for(instrument_id, now_ms) {
            Some(reason) => Err(reason),
            None => self
                .mu_for(instrument_id, now_ms)
                .map(UsableMu::new)
                .ok_or(MuHealthReason::Absent),
        }
    }
}
"""


class UsableMuSoleMintFenceTests(unittest.TestCase):
    # --- existing coverage (kept green) ---

    def test_gate_function_mint_is_allowed(self) -> None:
        # The mint inside usable_mu_for's body passes.
        violations = VERIFIER.find_violations_in_text(VERIFIER.GATE_PATH, GATE_FILE_OK)
        self.assertEqual(violations, [])

    def test_call_form_mint_in_another_production_file_fails(self) -> None:
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
        violations = VERIFIER.find_violations_in_text(
            "src/bolt_v3_maker_model.rs",
            """
            let wrapped = some_result.map(UsableMu::new);
            """,
        )
        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].line, 2)

    def test_cfg_test_mint_in_another_file_is_allowed(self) -> None:
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

    # --- new differential cases (each FAILS pre-fix; see rationale in commit) ---

    def test_extra_mint_in_gate_file_outside_usable_mu_for_fails(self) -> None:
        # Pre-fix (whole-file `if path == GATE_PATH: return []`) this passed — a
        # rogue mint anywhere in the gate file was exempt. Now the exemption is
        # scoped to the usable_mu_for body span, so a mint OUTSIDE it fails while
        # the real mint inside it still passes.
        gate_with_rogue = (
            GATE_FILE_OK
            + """
            pub fn rogue_mint(raw: f64) -> UsableMu {
                UsableMu::new(raw)
            }
            """
        )
        violations = VERIFIER.find_violations_in_text(VERIFIER.GATE_PATH, gate_with_rogue)
        # Exactly one violation: the rogue mint outside usable_mu_for. The real
        # mint inside usable_mu_for is NOT flagged (differential: same file, only
        # the out-of-span mint fails). The excerpt is the mint line itself.
        self.assertEqual(len(violations), 1)
        self.assertIn("outside the usable_mu_for gate function", violations[0].rule)
        self.assertIn("UsableMu::new(raw)", violations[0].excerpt)
        # The gate mint's line (`.map(UsableMu::new)`) is in the allowed span, so
        # the rogue mint's reported line is strictly after it.
        gate_only = VERIFIER.find_violations_in_text(VERIFIER.GATE_PATH, GATE_FILE_OK)
        self.assertEqual(gate_only, [])

    def test_missing_gate_function_fails_closed(self) -> None:
        # If usable_mu_for cannot be located in the gate file, every mint is
        # unexempt — an unparseable gate is not a license to mint.
        violations = VERIFIER.find_violations_in_text(
            VERIFIER.GATE_PATH,
            """
            pub fn some_other_fn(raw: f64) -> UsableMu {
                UsableMu::new(raw)
            }
            """,
        )
        self.assertEqual(len(violations), 1)

    def test_use_alias_rename_and_alias_mint_fail(self) -> None:
        # Pre-fix the literal `UsableMu::new` regex never matched `U::new`, so a
        # `use … UsableMu as U;` rename + `U::new(raw)` bypassed the gate. Now both
        # the rename declaration AND the alias mint are violations.
        violations = VERIFIER.find_violations_in_text(
            "src/bolt_v3_maker_quote_plan.rs",
            """
            use crate::bolt_v3_maker_mu_estimator::UsableMu as U;
            let bypass = U::new(raw_mu);
            """,
        )
        lines = {v.line for v in violations}
        rules = {v.rule for v in violations}
        self.assertEqual(lines, {2, 3})
        self.assertIn("UsableMu import-renamed (alias evades the gate)", rules)
        self.assertIn("UsableMu minted through an alias", rules)

    def test_type_alias_and_alias_mint_fail(self) -> None:
        # `type Mu = UsableMu;` + `Mu::new(raw)` is the type-alias variant of the
        # rename bypass — both the alias and the mint must fail.
        violations = VERIFIER.find_violations_in_text(
            "src/bolt_v3_maker_model.rs",
            """
            type Mu = UsableMu;
            let bypass = Mu::new(raw_mu);
            """,
        )
        lines = {v.line for v in violations}
        rules = {v.rule for v in violations}
        self.assertEqual(lines, {2, 3})
        self.assertIn("UsableMu type-aliased (alias evades the gate)", rules)
        self.assertIn("UsableMu minted through an alias", rules)

    def test_from_default_deserialize_impls_are_flagged(self) -> None:
        # `new` must stay the only constructor: a structural-mint trait impl for
        # UsableMu is a violation.
        from_v = VERIFIER.find_violations_in_text(
            "src/bolt_v3_maker_mu_estimator.rs",
            "\nimpl From<f64> for UsableMu { fn from(v: f64) -> Self { Self(v) } }\n",
        )
        self.assertTrue(any("From impl mints UsableMu" in v.rule for v in from_v))

        default_v = VERIFIER.find_violations_in_text(
            "src/bolt_v3_maker_mu_estimator.rs",
            "\nimpl Default for UsableMu { fn default() -> Self { Self(0.0) } }\n",
        )
        self.assertTrue(any("Default impl mints UsableMu" in v.rule for v in default_v))

        deser_v = VERIFIER.find_violations_in_text(
            "src/bolt_v3_maker_mu_estimator.rs",
            "\nimpl<'de> Deserialize<'de> for UsableMu { }\n",
        )
        self.assertTrue(
            any("Deserialize impl mints UsableMu" in v.rule for v in deser_v)
        )

    def test_alias_keyword_substrings_do_not_false_positive(self) -> None:
        # `type UsableMuView = SomethingElse;` and an unrelated `as` rename must
        # not trip the UsableMu alias rules.
        violations = VERIFIER.find_violations_in_text(
            "src/probe.rs",
            """
            type UsableMuView = OtherType;
            use crate::foo::OtherType as UsableMu2;
            """,
        )
        self.assertEqual(violations, [])


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
