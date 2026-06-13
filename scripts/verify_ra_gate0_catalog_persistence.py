#!/usr/bin/env python3
"""Verify RA Gate-0 catalog persistence is wired into the operator path."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
ARTIFACT_STORE = Path("crates/backtesting-vertical-slice/src/artifact_store.rs")
OPERATOR = Path("crates/backtesting-vertical-slice/src/operator.rs")
CONTRACT_TEST = Path("crates/backtesting-vertical-slice/tests/artifact_store_contract.rs")
BTE_CARGO_TOML = Path("crates/backtesting-vertical-slice/Cargo.toml")
JUSTFILE = Path("justfile")
RUN_SPEC = Path(
    "specs/023-nt-research-analytics-platform/reference/"
    "backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml"
)

CARGO_TOML_REQUIRED = (
    'object_store = { version = "=0.13.2", default-features = false, features = ["aws"] }',
)
ARTIFACT_STORE_REQUIRED = (
    "object_store::aws::{AmazonS3, AmazonS3Builder, S3ConditionalPut, S3CopyIfNotExists}",
    "pub struct S3ArtifactStoreConfig",
    "pub s3: S3ArtifactStoreConfig",
    "pub fn build_s3_object_store(&self) -> Result<AmazonS3>",
    ".with_bucket_name(",
    ".with_region(",
    ".with_conditional_put(S3ConditionalPut::ETagMatch)",
    ".with_copy_if_not_exists(S3CopyIfNotExists::Multipart)",
    "pub struct CreateOnlyProbeConfig",
    "pub struct CreateOnlyProbeTranscript",
    "copy_source_object_name",
    "copy_dest_object_name",
    "pub async fn persist_catalog_projection_for_source_binding",
    "pub async fn probe_create_only",
    "duplicate_create_rejected",
    "duplicate_copy_rejected",
    "create_only_probe_uri",
    "copy_if_not_exists",
    "CatalogDispatchConfig",
    ".catalog_root_for(",
    "CreateOnlyArtifactWriter::new",
    ".put_create_idempotent(",
    "fs::read(",
)
OPERATOR_REQUIRED = (
    "ArtifactStoreConfig",
    "CatalogDispatchConfig",
    "pub artifact_store: ArtifactStoreConfig",
    "pub catalog_dispatch: CatalogDispatchConfig",
    "pub create_only_probe_id: String",
    "create_only_probe_transcript",
    "run_from_run_spec_with_artifact_store",
    ".probe_create_only(",
    "persist_catalog_projection_for_source_binding",
)
TEST_REQUIRED = (
    "create_only_probe_requires_duplicate_create_rejection",
    "persists_catalog_projection_directory_with_create_only_dispatch",
    "rejects_duplicate_catalog_projection_bytes",
    "InMemory::new",
    "persist_catalog_projection_for_source_binding",
)
RUN_SPEC_REQUIRED = (
    "create_only_probe_id",
    "[artifact_store.s3]",
    "region",
    "conditional_put = \"etag\"",
    "copy_if_not_exists = \"multipart\"",
    "[artifact_store.create_only_probe]",
    "prefix",
    "object_name",
    "copy_source_object_name",
    "copy_dest_object_name",
)


def missing_snippets(path: Path, text: str, snippets: tuple[str, ...]) -> list[str]:
    return [f"{path}: missing `{snippet}`" for snippet in snippets if snippet not in text]


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    cargo_toml = root / BTE_CARGO_TOML
    if not cargo_toml.exists():
        findings.append(f"{BTE_CARGO_TOML}: Cargo.toml is missing")
    else:
        findings.extend(
            missing_snippets(
                BTE_CARGO_TOML,
                cargo_toml.read_text(encoding="utf-8"),
                CARGO_TOML_REQUIRED,
            )
        )

    artifact_store = root / ARTIFACT_STORE
    if not artifact_store.exists():
        findings.append(f"{ARTIFACT_STORE}: artifact_store.rs is missing")
    else:
        artifact_store_text = artifact_store.read_text(encoding="utf-8")
        findings.extend(
            missing_snippets(
                ARTIFACT_STORE,
                artifact_store_text,
                ARTIFACT_STORE_REQUIRED,
            )
        )
        for forbidden in ("AmazonS3Builder::from_env", "parse_url_opts"):
            if forbidden in artifact_store_text:
                findings.append(
                    f"{ARTIFACT_STORE}: forbidden hidden S3 config fallback `{forbidden}`"
                )

    operator = root / OPERATOR
    if not operator.exists():
        findings.append(f"{OPERATOR}: operator.rs is missing")
    else:
        text = operator.read_text(encoding="utf-8")
        findings.extend(missing_snippets(OPERATOR, text, OPERATOR_REQUIRED))

    test_file = root / CONTRACT_TEST
    if not test_file.exists():
        findings.append(f"{CONTRACT_TEST}: artifact store contract test is missing")
    else:
        findings.extend(
            missing_snippets(
                CONTRACT_TEST,
                test_file.read_text(encoding="utf-8"),
                TEST_REQUIRED,
            )
        )

    run_spec = root / RUN_SPEC
    if not run_spec.exists():
        findings.append(f"{RUN_SPEC}: committed run spec is missing")
    else:
        findings.extend(
            missing_snippets(RUN_SPEC, run_spec.read_text(encoding="utf-8"), RUN_SPEC_REQUIRED)
        )

    justfile = root / JUSTFILE
    if not justfile.exists():
        findings.append(f"{JUSTFILE}: justfile is missing")
    elif "verify-ra-gate0-catalog-persistence" not in justfile.read_text(encoding="utf-8"):
        findings.append(f"{JUSTFILE}: missing verify-ra-gate0-catalog-persistence recipe")

    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)

    findings = scan_root(args.root)
    if findings:
        print("FAIL: RA Gate-0 catalog persistence violations:", file=sys.stderr)
        for finding in findings:
            print(f"  {finding}", file=sys.stderr)
        return 1
    print("OK: RA Gate-0 catalog persistence passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
