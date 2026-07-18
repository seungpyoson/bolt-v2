#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import tempfile
import unittest

import verify_economics_dependency_direction as verifier


class EconomicsDependencyDirectionTest(unittest.TestCase):
    def verify_source(self, source: str) -> list[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            economics = root / "src" / "economics"
            economics.mkdir(parents=True)
            (economics / "mod.rs").write_text(source, encoding="utf-8")
            return verifier.verify(root)

    def test_accepts_general_purpose_domain_dependencies(self) -> None:
        errors = self.verify_source(
            "use rust_decimal::Decimal;\nuse std::{error::Error, fmt};\n"
        )
        self.assertEqual(errors, [])

    def test_rejects_execution_substrate_import(self) -> None:
        errors = self.verify_source("use nautilus_model::orders::Order;\n")
        self.assertTrue(any("nautilus_model" in error for error in errors), errors)

    def test_rejects_bolt_runtime_import(self) -> None:
        errors = self.verify_source("use crate::bolt_v3_order_execution::OrderExecutor;\n")
        self.assertTrue(any("bolt_v3" in error for error in errors), errors)

    def test_rejects_parent_escape_and_absolute_crate_imports(self) -> None:
        for source in (
            "use super::super::venue_contract::FeeSchedule;\n",
            "use ::bolt_v2::venue_contract::FeeSchedule;\n",
        ):
            with self.subTest(source=source):
                errors = self.verify_source(source)
                self.assertTrue(any("dependency" in error for error in errors), errors)

    def test_ignores_comments_and_literals(self) -> None:
        errors = self.verify_source(
            '// use nautilus_model::orders::Order;\nconst NOTE: &str = "crate::bolt_v3";\n'
        )
        self.assertEqual(errors, [])

    def test_rejects_venue_runtime_literal(self) -> None:
        errors = self.verify_source('const VENUE: &str = "hyperliquid";\n')
        self.assertTrue(any("venue-specific runtime literal" in error for error in errors), errors)

    def test_rejects_estimate_to_actual_conversion(self) -> None:
        errors = self.verify_source(
            "impl From<EstimatedEconomicComponent> for ActualEconomicEntry {}\n"
        )
        self.assertTrue(any("estimate-to-actual" in error for error in errors), errors)

    def test_requires_shared_domain_sources(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            errors = verifier.verify(pathlib.Path(directory))
        self.assertTrue(any("no Rust sources" in error for error in errors), errors)


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
