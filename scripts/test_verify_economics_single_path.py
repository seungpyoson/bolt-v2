#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import tempfile
import unittest

import verify_economics_single_path as verifier


class EconomicsSinglePathTest(unittest.TestCase):
    def verify_adapter_source(self, source: str) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for relative in verifier.ADAPTER_PATHS:
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(source, encoding="utf-8")
            return verifier.verify(root)

    def test_accepts_closed_plan_match(self) -> None:
        errors = self.verify_adapter_source(
            "match plan { Plan::Maker(rate) => quote(rate), Plan::Taker(rate) => quote(rate) }\n"
        )
        self.assertEqual(errors, [])

    def test_rejects_conditional_fallback_primitive(self) -> None:
        errors = self.verify_adapter_source("let rate = snapshot.rate.unwrap_or(config.rate);\n")
        self.assertTrue(any("conditional fallback" in error for error in errors), errors)

    def test_rejects_runtime_modifier_chain(self) -> None:
        errors = self.verify_adapter_source("if self.product.hip3 { rate *= scale; }\n")
        self.assertTrue(any("product-modifier branch" in error for error in errors), errors)

    def test_ignores_comments_and_literals(self) -> None:
        errors = self.verify_adapter_source(
            '// snapshot.rate.unwrap_or(config.rate)\nconst NOTE: &str = "if self.product.hip3";\n'
        )
        self.assertEqual(errors, [])


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
