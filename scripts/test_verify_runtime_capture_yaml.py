#!/usr/bin/env python3
"""Self-tests for the runtime-capture YAML verifier."""

from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_runtime_capture_yaml.py")
SPEC = importlib.util.spec_from_file_location("verify_runtime_capture_yaml", SCRIPT_PATH)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"failed to load {SCRIPT_PATH}")
VERIFIER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VERIFIER
SPEC.loader.exec_module(VERIFIER)


class RuntimeCaptureYamlVerifierTests(unittest.TestCase):
    def test_accepts_const_owned_risk_jsonl_path(self) -> None:
        source = """
        const RISK_DIR: &str = stringify!(risk);
        const TRADING_STATE_CHANGED_FILE: &str = "trading_state_changed.jsonl";

        let path = spool_root_path
            .join(RISK_DIR)
            .join(TRADING_STATE_CHANGED_FILE);
        """

        self.assertTrue(VERIFIER.has_risk_jsonl_path(source))

    def test_accepts_literal_risk_jsonl_path(self) -> None:
        source = """
        const RISK_DIR: &str = "risk";
        let path = spool_root_path
            .join("risk")
            .join("trading_state_changed.jsonl");
        """

        self.assertTrue(VERIFIER.has_risk_jsonl_path(source))

    def test_rejects_filename_without_risk_path(self) -> None:
        source = """
        const RISK_DIR: &str = stringify!(risk);
        const TRADING_STATE_CHANGED_FILE: &str = "trading_state_changed.jsonl";
        """

        self.assertFalse(VERIFIER.has_risk_jsonl_path(source))

    def test_rejects_risk_const_with_different_join_directory(self) -> None:
        source = """
        const RISK_DIR: &str = stringify!(risk);
        const TRADING_STATE_CHANGED_FILE: &str = "trading_state_changed.jsonl";

        let path = spool_root_path
            .join("other")
            .join(TRADING_STATE_CHANGED_FILE);
        """

        self.assertFalse(VERIFIER.has_risk_jsonl_path(source))

    def test_rejects_join_chain_without_risk_const(self) -> None:
        source = """
        const TRADING_STATE_CHANGED_FILE: &str = "trading_state_changed.jsonl";

        let path = spool_root_path
            .join("risk")
            .join(TRADING_STATE_CHANGED_FILE);
        """

        self.assertFalse(VERIFIER.has_risk_jsonl_path(source))

if __name__ == "__main__":
    unittest.main()
