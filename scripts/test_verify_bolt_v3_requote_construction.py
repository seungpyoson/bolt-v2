#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 requote-budget construction fence."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_requote_construction.py")
SPEC = importlib.util.spec_from_file_location(
    "verify_bolt_v3_requote_construction",
    SCRIPT_PATH,
)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


class RequoteConstructionFenceTests(unittest.TestCase):
    def test_detects_direct_pair_construction(self) -> None:
        # The exact bypass the fence guards against: a caller building the pair
        # with literal caps/windows instead of going through the config bridge.
        violations = VERIFIER.find_violations_in_text(
            "src/some_strategy.rs",
            """
            let pair = RequoteBudgetPair::new(submit, rest);
            """,
        )

        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].line, 2)

    def test_detects_inner_budget_construction(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/some_strategy.rs",
            """
            let submit = RequoteBudget::new(40, 60_000, 500);
            let rest = RequoteBudget::new(100, 60_000, 500);
            """,
        )

        self.assertEqual({violation.line for violation in violations}, {2, 3})

    def test_does_not_match_substrings_comments_or_lookalikes(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/some_strategy.rs",
            """
            // RequoteBudgetPair::new(a, b) is the documented way.
            let doc = "RequoteBudget::new is config-sourced";
            let builder = RequoteBudgetPairBuilder::new(submit, rest);
            let other = FakeRequoteBudget::new(cap);
            let typo = RequoteBudget::news(cap, win, iv);
            let trailing = RequoteBudgetPairExtra::new(submit, rest);
            """,
        )

        self.assertEqual(violations, [])

    def test_cfg_test_construction_is_exempt(self) -> None:
        # Tests legitimately build budgets with explicit caps; only the production
        # construction line must be flagged.
        with tempfile.TemporaryDirectory() as temp_dir:
            probe = Path(temp_dir) / "probe.rs"
            probe.write_text(
                """
                #[cfg(test)]
                mod tests {
                    fn helper() -> RequoteBudgetPair {
                        RequoteBudgetPair::new(
                            RequoteBudget::new(2, 60_000, 0),
                            RequoteBudget::new(2, 60_000, 0),
                        )
                    }
                }

                fn production() -> RequoteBudgetPair {
                    RequoteBudgetPair::new(submit, rest)
                }
                """,
                encoding="utf-8",
            )

            violations = VERIFIER.find_violations_in_text(
                "src/probe.rs",
                VERIFIER.production_text(probe),
            )

        self.assertEqual({violation.line for violation in violations}, {13})

    def test_bridge_module_is_exempt_and_the_skip_is_load_bearing(self) -> None:
        bridge = VERIFIER.REPO_ROOT / VERIFIER.BRIDGE_PATH
        self.assertTrue(bridge.is_file(), f"bridge module missing: {bridge}")

        # Non-vacuity: the bridge genuinely constructs the pair, so without the
        # exemption it WOULD be flagged.
        bridge_violations = VERIFIER.find_violations_in_text(
            VERIFIER.BRIDGE_PATH,
            VERIFIER.production_text(bridge),
        )
        self.assertTrue(
            bridge_violations,
            "expected the bridge to construct the requote governor; if it does not, "
            "the exemption test below is vacuous",
        )

        # The fence skips exactly the bridge path, so scanning only the bridge yields
        # no violations despite the construction above.
        self.assertEqual(VERIFIER.collect_violations_from_files([bridge]), [])

    def test_empty_source_file_set_fails_closed(self) -> None:
        violations = VERIFIER.collect_violations_from_files([])
        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0].kind, "source-floor")
        self.assertEqual(violations[0].excerpt, "Rust source files under src: enforcement set is empty")

    def test_main_reports_empty_source_floor_without_defining_module_crash(self) -> None:
        original_root = VERIFIER.REPO_ROOT
        with tempfile.TemporaryDirectory() as temp_dir:
            try:
                VERIFIER.REPO_ROOT = Path(temp_dir)
                stderr = io.StringIO()
                with contextlib.redirect_stderr(stderr):
                    code = VERIFIER.main()
            finally:
                VERIFIER.REPO_ROOT = original_root

        self.assertEqual(code, 1)
        self.assertIn("Rust source files under src: enforcement set is empty", stderr.getvalue())

    def test_current_bolt_src_constructs_only_via_bridge(self) -> None:
        self.assertEqual(VERIFIER.collect_violations(), [])

    def test_detects_whitespace_and_ufcs_construction(self) -> None:
        # rustfmt is not guaranteed before the fence, and UFCS spells the path
        # differently; both must still be caught.
        violations = VERIFIER.find_violations_in_text(
            "src/some_strategy.rs",
            """
            let a = RequoteBudget :: new(1, 2, 3);
            let b = RequoteBudgetPair  ::  new(submit, rest);
            let c = <RequoteBudget>::new(1, 2, 3);
            let d = <RequoteBudgetPair as Gov>::new(submit, rest);
            """,
        )

        self.assertEqual(
            [v.line for v in violations if v.kind == "construct"], [2, 3, 4, 5]
        )

    def test_detects_alias_import_bypass(self) -> None:
        # Aliasing the type defeats the call-site regex (`Foo::new` carries no
        # `RequoteBudget` token), so the alias import itself is the catchable signal.
        violations = VERIFIER.find_violations_in_text(
            "src/some_strategy.rs",
            """
            use crate::bolt_v3_requote_budget::RequoteBudget as Foo;
            let pair = Foo::new(40, 60_000, 500);
            """,
        )

        alias = [v for v in violations if v.kind == "alias"]
        self.assertEqual(len(alias), 1)
        self.assertEqual(alias[0].line, 2)
        # The aliased call-site itself carries no governor token, so it is not (and
        # need not be) flagged as a construction — the import is what the fence trips on.
        self.assertEqual([v for v in violations if v.kind == "construct"], [])

    def test_detects_braced_alias_import(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/some_strategy.rs",
            "use crate::bolt_v3_requote_budget::{RequoteBudgetPair as Bar};\n",
        )

        self.assertEqual([v.kind for v in violations], ["alias"])

    def test_plain_unaliased_use_is_not_flagged(self) -> None:
        # Naming the type in a signature (no `as`) is legitimate and common.
        violations = VERIFIER.find_violations_in_text(
            "src/consumer.rs",
            """
            use crate::bolt_v3_requote_budget::RequoteBudgetPair;
            fn drive(budget: &mut RequoteBudgetPair) -> bool { false }
            """,
        )

        self.assertEqual(violations, [])

    def test_alias_lookalike_is_not_flagged(self) -> None:
        violations = VERIFIER.find_violations_in_text(
            "src/some_strategy.rs",
            "use crate::other::RequoteBudgetPairBuilder as B;\n",
        )

        self.assertEqual(violations, [])

    def test_visibility_guard_flags_bare_pub_new(self) -> None:
        flagged = VERIFIER.find_visibility_violations_in_text(
            "src/bolt_v3_requote_budget.rs",
            "    pub fn new(cap: u64) -> Self { unimplemented!() }\n",
        )
        self.assertEqual([v.kind for v in flagged], ["visibility"])

        permitted = VERIFIER.find_visibility_violations_in_text(
            "src/bolt_v3_requote_budget.rs",
            "    pub(crate) fn new(cap: u64) -> Self { unimplemented!() }\n"
            "    fn new(cap: u64) -> Self { unimplemented!() }\n",
        )
        self.assertEqual(permitted, [])

    def test_defining_module_constructors_remain_pub_crate(self) -> None:
        # The fence's src/-only scope is sound only while the constructors stay
        # pub(crate); the real defining module must satisfy that today.
        self.assertEqual(VERIFIER.collect_visibility_violations(), [])


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
