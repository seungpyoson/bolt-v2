#!/usr/bin/env python3
"""Self-tests for verify_ra_bi_surface_and_feature_joins.py."""

from __future__ import annotations

import tempfile
import textwrap
import unittest
from pathlib import Path

import verify_ra_bi_surface_and_feature_joins as verifier


COMPLIANT_HELPER = """
use std::collections::BTreeSet;

pub struct NotebookQueryEngine {
    pub engine_key: String,
    pub reads_nt_catalog_arrow: bool,
    pub read_only: bool,
}

pub struct NotebookErgonomics {
    pub read_only: bool,
    pub exposes_arrow_batches: bool,
    pub exposes_sql_examples: bool,
    pub mutation_actions_enabled: bool,
}

pub enum CustomUiDecision {
    NotSelected,
    AllowedAfterProductGate {
        confirmed_requirement_refs: Vec<String>,
        rejected_product_refs: Vec<String>,
    },
}

pub struct NotebookBiSurfaceSpec {
    pub artifact_root: String,
    pub nt_catalog_arrow_uri: String,
    pub query_engines: Vec<NotebookQueryEngine>,
    pub dashboard_product_refs: Vec<String>,
    pub notebook: NotebookErgonomics,
    pub custom_ui: CustomUiDecision,
}

pub struct NotebookBiSurface {
    pub artifact_root: String,
    pub nt_catalog_arrow_uri: String,
    pub query_engines: Vec<NotebookQueryEngine>,
    pub dashboard_product_refs: Vec<String>,
    pub notebook: NotebookErgonomics,
    pub custom_ui: CustomUiDecision,
}

pub fn build_notebook_bi_surface(spec: NotebookBiSurfaceSpec) -> anyhow::Result<NotebookBiSurface> {
    ensure_uri_under_artifact_root(&spec.artifact_root, &spec.nt_catalog_arrow_uri)?;
    validate_notebook_ergonomics(&spec.notebook)?;
    validate_custom_ui_decision(&spec.custom_ui)?;
    Ok(NotebookBiSurface {
        artifact_root: spec.artifact_root,
        nt_catalog_arrow_uri: spec.nt_catalog_arrow_uri,
        query_engines: spec.query_engines,
        dashboard_product_refs: spec.dashboard_product_refs,
        notebook: spec.notebook,
        custom_ui: spec.custom_ui,
    })
}

fn ensure_uri_under_artifact_root(artifact_root: &str, nt_catalog_arrow_uri: &str) -> anyhow::Result<()> {
    anyhow::ensure!(nt_catalog_arrow_uri.starts_with(artifact_root));
    Ok(())
}

fn validate_notebook_ergonomics(notebook: &NotebookErgonomics) -> anyhow::Result<()> {
    anyhow::ensure!(notebook.read_only);
    anyhow::ensure!(notebook.exposes_arrow_batches);
    anyhow::ensure!(notebook.exposes_sql_examples);
    anyhow::ensure!(!notebook.mutation_actions_enabled);
    Ok(())
}

fn validate_custom_ui_decision(custom_ui: &CustomUiDecision) -> anyhow::Result<()> {
    match custom_ui {
        CustomUiDecision::NotSelected => {}
        CustomUiDecision::AllowedAfterProductGate {
            confirmed_requirement_refs,
            rejected_product_refs,
        } => {
            anyhow::ensure!(!confirmed_requirement_refs.is_empty());
            anyhow::ensure!(!rejected_product_refs.is_empty());
        }
    }
    Ok(())
}

pub struct AnalyticsSourceBinding {
    pub source_binding_key: String,
    pub venue_key: String,
    pub provider_key: String,
}

pub struct FeatureJoinSpec {
    pub left_source_binding_key: String,
    pub right_source_binding_key: String,
    pub as_of_column: String,
    pub freshness_column: String,
}

pub fn validate_feature_join_bindings(
    bindings: &[AnalyticsSourceBinding],
    joins: &[FeatureJoinSpec],
) -> anyhow::Result<()> {
    let source_binding_key: BTreeSet<String> =
        bindings.iter().map(|binding| binding.source_binding_key.clone()).collect();
    let venue_key: BTreeSet<String> = bindings.iter().map(|binding| binding.venue_key.clone()).collect();
    let provider_key: BTreeSet<String> =
        bindings.iter().map(|binding| binding.provider_key.clone()).collect();
    for join in joins {
        anyhow::ensure!(source_binding_key.contains(&join.left_source_binding_key));
        anyhow::ensure!(source_binding_key.contains(&join.right_source_binding_key));
        anyhow::ensure!(!venue_key.contains(&join.left_source_binding_key));
        anyhow::ensure!(!provider_key.contains(&join.right_source_binding_key));
        anyhow::ensure!(!join.as_of_column.is_empty());
        anyhow::ensure!(!join.freshness_column.is_empty());
    }
    Ok(())
}
"""

COMPLIANT_TEST = """
#[test]
fn notebook_bi_surface_exposes_duckdb_and_polars_over_nt_catalog_arrow_without_custom_ui() {
    let duck = "duckdb";
    let polars = "polars";
    assert_ne!(duck, polars);
}

#[test]
fn notebook_bi_surface_requires_product_gate_before_custom_ui() {}

#[test]
fn analytics_feature_joins_use_source_binding_keys_not_venue_or_provider_literals() {
    assert!("source_binding_key".contains("source_binding_key"));
}
"""


class RaBiSurfaceVerifierTests(unittest.TestCase):
    def write_repo(
        self,
        root: Path,
        *,
        helper_text: str = COMPLIANT_HELPER,
        test_text: str = COMPLIANT_TEST,
        just_wired: bool = True,
    ) -> None:
        (root / verifier.HELPER_PATH).parent.mkdir(parents=True, exist_ok=True)
        (root / verifier.TEST_PATH).parent.mkdir(parents=True, exist_ok=True)
        (root / verifier.JUSTFILE_PATH).parent.mkdir(parents=True, exist_ok=True)
        (root / verifier.HELPER_PATH).write_text(textwrap.dedent(helper_text), encoding="utf-8")
        (root / verifier.TEST_PATH).write_text(textwrap.dedent(test_text), encoding="utf-8")
        just_text = ""
        if just_wired:
            just_text = (
                "source-fence-static:\n"
                "    python3 scripts/test_verify_ra_bi_surface_and_feature_joins.py\n"
                "    python3 scripts/verify_ra_bi_surface_and_feature_joins.py\n"
            )
        (root / verifier.JUSTFILE_PATH).write_text(just_text, encoding="utf-8")

    def test_compliant_fixture_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_repo(root)
            self.assertEqual(verifier.scan_root(root), [])

    def test_comment_only_symbols_do_not_pass(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_repo(root, helper_text="// pub struct NotebookBiSurfaceSpec {}\n")
            findings = verifier.scan_root(root)
            self.assertTrue(any("NotebookBiSurfaceSpec" in finding for finding in findings))
            self.assertTrue(any("validate_feature_join_bindings" in finding for finding in findings))

    def test_custom_ui_gate_validation_is_required(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            helper_text = COMPLIANT_HELPER.replace(
                "CustomUiDecision::AllowedAfterProductGate {\n"
                "            confirmed_requirement_refs,\n"
                "            rejected_product_refs,\n"
                "        } => {\n"
                "            anyhow::ensure!(!confirmed_requirement_refs.is_empty());\n"
                "            anyhow::ensure!(!rejected_product_refs.is_empty());\n"
                "        }",
                "CustomUiDecision::AllowedAfterProductGate { .. } => {}",
            )
            self.write_repo(root, helper_text=helper_text)
            findings = verifier.scan_root(root)
            self.assertTrue(any("custom UI product-gate" in finding for finding in findings))

    def test_duckdb_and_polars_test_values_are_required(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.write_repo(root, test_text=COMPLIANT_TEST.replace('"polars"', '"sql-only"'))
            findings = verifier.scan_root(root)
            self.assertTrue(any("Polars" in finding for finding in findings))

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
