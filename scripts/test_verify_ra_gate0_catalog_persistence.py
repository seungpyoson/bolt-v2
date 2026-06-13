#!/usr/bin/env python3
"""Self-tests for the RA Gate-0 catalog persistence verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_ra_gate0_catalog_persistence.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_ra_gate0_catalog_persistence", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_file(root: Path, rel: str, text: str) -> Path:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def compliant_artifact_store() -> str:
    return """
pub async fn persist_catalog_projection_for_source_binding() {
    let _dispatch: CatalogDispatchConfig;
    let _root = dispatch.catalog_root_for(source_binding, artifact_root)?;
    let writer = CreateOnlyArtifactWriter::new(store);
    let bytes = fs::read(path)?;
    writer.put_create_idempotent(path, bytes).await?;
}
"""


def compliant_operator() -> str:
    return f"""
use crate::artifact_store::{{ArtifactStoreConfig, CatalogDispatchConfig}};

pub struct RunSpec {{
    pub artifact_store: ArtifactStoreConfig,
    pub catalog_dispatch: CatalogDispatchConfig,
}}

pub fn run_from_run_spec_with_artifact_store() {{
    persist_catalog_projection_for_source_binding();
}}
"""


def compliant_test() -> str:
    return """
fn persists_catalog_projection_directory_with_create_only_dispatch() {
    let _store = InMemory::new();
    persist_catalog_projection_for_source_binding();
}

fn rejects_duplicate_catalog_projection_bytes() {}
"""


def write_compliant_tree(root: Path) -> None:
    write_file(
        root,
        "crates/backtesting-vertical-slice/src/artifact_store.rs",
        compliant_artifact_store(),
    )
    write_file(
        root,
        "crates/backtesting-vertical-slice/src/operator.rs",
        compliant_operator(),
    )
    write_file(
        root,
        "crates/backtesting-vertical-slice/tests/artifact_store_contract.rs",
        compliant_test(),
    )
    write_file(
        root,
        "justfile",
        "verify-ra-gate0-catalog-persistence:\n    python3 scripts/verify_ra_gate0_catalog_persistence.py\n",
    )


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def test_compliant_tree_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_compliant_tree(root)

        assert verifier.scan_root(root) == []


def test_missing_persistence_helper_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_compliant_tree(root)
        write_file(root, "crates/backtesting-vertical-slice/src/artifact_store.rs", "")

        findings = verifier.scan_root(root)

    assert any("persist_catalog_projection_for_source_binding" in finding for finding in findings)


def test_cli_fails_with_actionable_output() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_compliant_tree(root)
        write_file(root, "crates/backtesting-vertical-slice/src/operator.rs", "")

        result = run_script("--root", str(root))

    assert result.returncode == 1
    assert "FAIL:" in result.stderr
    assert "ArtifactStoreConfig" in result.stderr


def main() -> int:
    tests = [
        test_compliant_tree_passes,
        test_missing_persistence_helper_is_a_finding,
        test_cli_fails_with_actionable_output,
    ]
    for test in tests:
        test()
    print("OK: RA Gate-0 catalog persistence verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
