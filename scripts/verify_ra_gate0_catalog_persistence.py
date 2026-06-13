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
JUSTFILE = Path("justfile")

ARTIFACT_STORE_REQUIRED = (
    "pub async fn persist_catalog_projection_for_source_binding",
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
    "run_from_run_spec_with_artifact_store",
    "persist_catalog_projection_for_source_binding",
)
TEST_REQUIRED = (
    "persists_catalog_projection_directory_with_create_only_dispatch",
    "rejects_duplicate_catalog_projection_bytes",
    "InMemory::new",
    "persist_catalog_projection_for_source_binding",
)


def missing_snippets(path: Path, text: str, snippets: tuple[str, ...]) -> list[str]:
    return [f"{path}: missing `{snippet}`" for snippet in snippets if snippet not in text]


def scan_root(root: Path) -> list[str]:
    root = root.resolve()
    findings: list[str] = []

    artifact_store = root / ARTIFACT_STORE
    if not artifact_store.exists():
        findings.append(f"{ARTIFACT_STORE}: artifact_store.rs is missing")
    else:
        findings.extend(
            missing_snippets(
                ARTIFACT_STORE,
                artifact_store.read_text(encoding="utf-8"),
                ARTIFACT_STORE_REQUIRED,
            )
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
