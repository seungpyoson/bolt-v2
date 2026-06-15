#!/usr/bin/env python3
"""Self-tests for verify_ra_run_pointer_index.py."""

from __future__ import annotations

import tempfile
import textwrap
import unittest
from pathlib import Path

import verify_ra_run_pointer_index as verifier


COMPLIANT_RA = """
use std::collections::{BTreeMap, BTreeSet};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use crate::hashing::sha256_hex;

pub trait BacktestRunCatalogList {
    fn list_backtest_runs(&self) -> anyhow::Result<Vec<String>>;
}

impl BacktestRunCatalogList for ParquetDataCatalog {
    fn list_backtest_runs(&self) -> anyhow::Result<Vec<String>> {
        self.list_backtest_runs().map_err(Into::into)
    }
}

pub struct RunPointerResult {
    pub result_contract_uri: String,
    pub result_contract_hash: String,
}

pub struct RunPointerIndexRecord {
    pub run_id: String,
    pub params: BTreeMap<String, serde_json::Value>,
    pub result: RunPointerResult,
}

pub struct RunPointerIndex {
    pub schema_version: u64,
    pub artifact_root: String,
    pub content_hash: String,
    pub runs: Vec<RunPointerIndexRecord>,
}

impl RunPointerIndex {
    pub fn expected_content_hash(&self) -> anyhow::Result<String> {
        Ok(sha256_hex(&serde_json::to_vec(&self.runs)?))
    }
}

pub fn build_run_pointer_index_from_catalog<C: BacktestRunCatalogList>(
    catalog: &C,
    artifact_root: &str,
    records: Vec<RunPointerIndexRecord>,
) -> anyhow::Result<RunPointerIndex> {
    let listed: BTreeSet<String> = catalog.list_backtest_runs()?.into_iter().collect();
    let indexed: BTreeSet<String> = records.iter().map(|record| record.run_id.clone()).collect();
    anyhow::ensure!(listed == indexed, "records must match catalog.list_backtest_runs");
    Ok(RunPointerIndex {
        schema_version: 1,
        artifact_root: artifact_root.to_string(),
        content_hash: sha256_hex(&serde_json::to_vec(&records)?),
        runs: records,
    })
}
"""

COMPLIANT_TEST = """
struct FakeBacktestRunCatalog;

impl BacktestRunCatalogList for FakeBacktestRunCatalog {
    fn list_backtest_runs(&self) -> anyhow::Result<Vec<String>> {
        Ok(vec!["run-a".to_string()])
    }
}

#[test]
fn run_pointer_index_covers_catalog_runs_with_hash_and_no_lifecycle_or_promotion_state() {
    let serialized = serde_json::to_value(&index).unwrap();
    assert!(serialized.get("lifecycle_state").is_none());
    assert!(serialized.get("promotion_config").is_none());
}

#[test]
fn run_pointer_index_rejects_records_not_backed_by_one_catalog_root() {}
"""


class RunPointerVerifierTests(unittest.TestCase):
    def write_repo(
        self,
        root: Path,
        *,
        ra_text: str = COMPLIANT_RA,
        test_text: str = COMPLIANT_TEST,
        tasks_checked: bool = True,
        just_wired: bool = True,
    ) -> None:
        (root / verifier.RA_PATH).parent.mkdir(parents=True, exist_ok=True)
        (root / verifier.TEST_PATH).parent.mkdir(parents=True, exist_ok=True)
        (root / verifier.TASKS_PATH).parent.mkdir(parents=True, exist_ok=True)
        (root / verifier.JUSTFILE_PATH).parent.mkdir(parents=True, exist_ok=True)
        (root / verifier.RA_PATH).write_text(textwrap.dedent(ra_text), encoding="utf-8")
        (root / verifier.TEST_PATH).write_text(textwrap.dedent(test_text), encoding="utf-8")
        task_box = "x" if tasks_checked else " "
        (root / verifier.TASKS_PATH).write_text(
            f"- [{task_box}] RA-013 Implement run pointer index\\n",
            encoding="utf-8",
        )
        just_text = ""
        if just_wired:
            just_text = (
                "source-fence-static:\\n"
                "    python3 scripts/test_verify_ra_run_pointer_index.py\\n"
                "    python3 scripts/verify_ra_run_pointer_index.py\\n"
            )
        (root / verifier.JUSTFILE_PATH).write_text(just_text, encoding="utf-8")

    def test_compliant_fixture_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_repo(root)
            self.assertEqual(verifier.scan_root(root), [])

    def test_task_checkbox_is_required(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_repo(root, tasks_checked=False)
            findings = verifier.scan_root(root)
            self.assertTrue(any("RA-013" in finding for finding in findings))

    def test_comment_only_catalog_delegate_does_not_pass(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_repo(
                root,
                ra_text='// impl BacktestRunCatalogList for ParquetDataCatalog { self.list_backtest_runs()? }\\n',
            )
            findings = verifier.scan_root(root)
            self.assertTrue(any("ParquetDataCatalog" in finding for finding in findings))

    def test_run_pointer_index_cannot_carry_lifecycle_or_promotion(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_repo(
                root,
                ra_text=COMPLIANT_RA.replace(
                    "pub runs: Vec<RunPointerIndexRecord>,",
                    "pub runs: Vec<RunPointerIndexRecord>,\\n    pub lifecycle_state: LifecycleState,\\n    pub promotion_config: PromotionConfigRef,",
                ),
            )
            findings = verifier.scan_root(root)
            self.assertTrue(any("lifecycle" in finding for finding in findings))
            self.assertTrue(any("promotion" in finding for finding in findings))

    def test_justfile_wiring_is_required(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_repo(root, just_wired=False)
            findings = verifier.scan_root(root)
            self.assertTrue(any("source-fence-static" in finding for finding in findings))


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
