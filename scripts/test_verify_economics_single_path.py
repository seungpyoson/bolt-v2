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
            path = root / verifier.ADAPTER_ROOT / "synthetic" / "economics.rs"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source, encoding="utf-8")
            for relative in verifier.SEALED_CONSUMER_RULES:
                consumer = root / relative
                consumer.parent.mkdir(parents=True, exist_ok=True)
                consumer.write_text("consume(sealed.full_reservation_liability());\n", encoding="utf-8")
            return verifier.verify(root)

    def verify_sealed_consumer_source(self, relative: pathlib.Path, source: str) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            adapter = root / verifier.ADAPTER_ROOT / "synthetic" / "economics.rs"
            adapter.parent.mkdir(parents=True, exist_ok=True)
            adapter.write_text("quote(closed_plan);\n", encoding="utf-8")
            for consumer_relative in verifier.SEALED_CONSUMER_RULES:
                consumer = root / consumer_relative
                consumer.parent.mkdir(parents=True, exist_ok=True)
                consumer.write_text(
                    source if consumer_relative == relative else "consume(sealed);\n",
                    encoding="utf-8",
                )
            return verifier.verify(root)

    def test_discovers_new_provider_adapter(self) -> None:
        errors = self.verify_adapter_source("let rate = snapshot.rate.unwrap_or(config.rate);\n")
        self.assertTrue(any("synthetic/economics.rs" in error for error in errors), errors)

    def test_accepts_closed_plan_match(self) -> None:
        errors = self.verify_adapter_source(
            "match plan { Plan::Maker(rate) => quote(rate), Plan::Taker(rate) => quote(rate) }\n"
        )
        self.assertEqual(errors, [])

    def test_rejects_conditional_fallback_primitive(self) -> None:
        for primitive in (
            "snapshot.rate.unwrap_or(config.rate)",
            "snapshot.rate.unwrap_or_default()",
            "snapshot.rate.or(config.rate)",
        ):
            with self.subTest(primitive=primitive):
                errors = self.verify_adapter_source(f"let rate = {primitive};\n")
                self.assertTrue(any("conditional fallback" in error for error in errors), errors)

    def test_discovers_nested_economics_module(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            path = root / verifier.ADAPTER_ROOT / "synthetic" / "economics" / "quote.rs"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("let rate = snapshot.rate.unwrap_or_default();\n", encoding="utf-8")
            errors = verifier.verify(root)
        self.assertTrue(any("economics/quote.rs" in error for error in errors), errors)

    def test_rejects_runtime_modifier_chain(self) -> None:
        errors = self.verify_adapter_source("if self.product.hip3 { rate *= scale; }\n")
        self.assertTrue(any("product-modifier branch" in error for error in errors), errors)

    def test_ignores_comments_and_literals(self) -> None:
        errors = self.verify_adapter_source(
            '// snapshot.rate.unwrap_or(config.rate)\nconst NOTE: &str = "if self.product.hip3";\n'
        )
        self.assertEqual(errors, [])

    def test_rejects_scanner_cost_as_basket_authority(self) -> None:
        errors = self.verify_sealed_consumer_source(
            pathlib.Path("src/bolt_v3_basket_admission.rs"),
            "reserve(request.scanner_evidence.total_adjusted_cost);\n",
        )
        self.assertTrue(any("scanner economics" in error for error in errors), errors)

    def test_rejects_capital_price_quantity_recalculation(self) -> None:
        errors = self.verify_sealed_consumer_source(
            pathlib.Path("src/bolt_v3_capital_admission.rs"),
            "let liability = request.limit_price.checked_mul(request.quantity);\n",
        )
        self.assertTrue(any("re-derived price/quantity" in error for error in errors), errors)


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
