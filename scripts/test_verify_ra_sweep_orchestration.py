#!/usr/bin/env python3
"""Self-tests for the RA-008 sweep orchestration verifier."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_ra_sweep_orchestration.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_ra_sweep_orchestration", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_file(root: Path, rel: str, text: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def write_common(
    root: Path,
    *,
    ra_source: str | None = None,
    operator_source: str | None = None,
    test_source: str | None = None,
    justfile: str | None = None,
    tasks: str | None = None,
) -> None:
    write_file(
        root,
        "crates/backtesting-vertical-slice/src/research_analytics.rs",
        ra_source if ra_source is not None else compliant_ra_source(),
    )
    write_file(
        root,
        "crates/backtesting-vertical-slice/src/operator.rs",
        operator_source
        if operator_source is not None
        else "#[derive(Debug, Clone, Serialize, Deserialize)]\n#[serde(deny_unknown_fields)]\npub struct RunSpec {}\n",
    )
    write_file(
        root,
        "crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_research_analytics.rs",
        test_source
        if test_source is not None
        else """
fn sweep_orchestration_writes_typed_run_specs_invokes_bte_and_reads_contracts() { run_backtest_sweep_with_executor(); }
fn sweep_orchestration_rejects_existing_run_spec_file_before_executor() {}
fn sweep_orchestration_rejects_existing_output_dir_before_executor() {}
fn sweep_orchestration_rejects_duplicate_materialization_paths_before_executor() {}
fn sweep_orchestration_rejects_contract_not_bound_to_run_spec() {}
""",
    )
    write_file(
        root,
        "justfile",
        justfile
        if justfile is not None
        else """source-fence-static-inner:
    python3 scripts/test_verify_ra_sweep_orchestration.py
    python3 scripts/verify_ra_sweep_orchestration.py
""",
    )
    write_file(
        root,
        "specs/023-nt-research-analytics-platform/2-research-analytics/tasks.md",
        tasks if tasks is not None else "- [x] RA-008 Implement sweep orchestration.\n",
    )


def compliant_ra_source() -> str:
    return """
pub struct BacktestSweepPlan;
pub struct BacktestSweepRun {
    accepted_object_bytes: Vec<u8>,
}
pub struct BacktestSweepReport;

pub fn run_backtest_sweep() {
    run_operator_from_run_spec();
}

pub fn run_backtest_sweep_with_executor() {
    let run = BacktestSweepRun { accepted_object_bytes: Vec::new() };
    let mut seen_run_spec_file_names = BTreeSet::new();
    let mut seen_output_dir_names = BTreeSet::new();
    seen_run_spec_file_names.insert(run.run_spec_file_name.clone());
    seen_output_dir_names.insert(run.output_dir_name.clone());
    let run_spec_path = PathBuf::from("first-run.toml");
    let output_dir = PathBuf::from("first-run");
    run_spec_path.try_exists().unwrap();
    output_dir.try_exists().unwrap();
    fs::create_dir(&output_dir).unwrap();
    OpenOptions::new().write(true).create_new(true).open(&run_spec_path).unwrap();
    let _ = toml::to_string_pretty(&run.run_spec);
    let _ = accepted_object_bytes;
    let _path = RESULT_CONTRACT_FILE;
    let contract: BacktestResultContract = read_result_contract();
    let _parsed: BacktestResultContract = serde_json::from_slice(&bytes).unwrap();
    contract.validate();
    validate_result_contract_matches_run(&contract, &run, &result_contract_path);
    let expected_manifest_hash = run.run_spec.manifest.manifest_hash();
    let expected_accepted_object_sha256 = sha256_hex(&run.accepted_object_bytes);
    let expected_converter_config_hash = run.run_spec.converter.content_hash().unwrap();
    let _ = contract.manifest_hash == expected_manifest_hash;
    let _ = contract.accepted_object_sha256 == expected_accepted_object_sha256;
    let _ = run.run_spec.accepted_object.sha256 == expected_accepted_object_sha256;
    let _ = contract.strategy_config_hash == run.run_spec.manifest.strategy_config_hash;
    let _ = contract.converter_config_hash == expected_converter_config_hash;
}
"""


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=60,
    )


def test_compliant_sweep_orchestration_passes() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root)

        assert verifier.scan_root(root) == []


def test_comments_and_strings_do_not_satisfy_shape() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(
            root,
            ra_source='const FAKE: &str = "BacktestSweepPlan run_operator_from_run_spec RESULT_CONTRACT_FILE";\n',
        )

        findings = verifier.scan_root(root)

    assert any("BacktestSweepPlan" in finding for finding in findings)
    assert any("existing BTE operator" in finding for finding in findings)


def test_rejects_second_runner_owned_by_ra() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root, ra_source=compliant_ra_source() + "\nuse nautilus_backtest::BacktestEngine;\n")

        findings = verifier.scan_root(root)

    assert any("must not own runner code" in finding for finding in findings)


def test_rejects_missing_contract_readback() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root, ra_source=compliant_ra_source().replace("read_result_contract", "skip_contract"))

        findings = verifier.scan_root(root)

    assert any("persisted result-contract JSON read" in finding for finding in findings)


def test_rejects_overwrite_prone_materialization() -> None:
    verifier = load_verifier()
    unsafe_source = compliant_ra_source().replace(
        "run_spec_path.try_exists().unwrap();\n    output_dir.try_exists().unwrap();\n    fs::create_dir(&output_dir).unwrap();\n    OpenOptions::new().write(true).create_new(true).open(&run_spec_path).unwrap();",
        "fs::create_dir_all(&output_dir).unwrap();\n    fs::write(&run_spec_path, \"stale\").unwrap();",
    )
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root, ra_source=unsafe_source)

        findings = verifier.scan_root(root)

    assert any("run-spec create-only write" in finding for finding in findings)
    assert any("fresh per-run output-dir create" in finding for finding in findings)
    assert any("overwrite-prone run-spec write" in finding for finding in findings)
    assert any("reuse-prone per-run output-dir mkdir" in finding for finding in findings)


def test_rejects_missing_source_fence_wiring() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_common(root, justfile="source-fence-static-inner:\n    python3 scripts/other.py\n")

        findings = verifier.scan_root(root)

    assert any("source-fence-static must run" in finding for finding in findings)


def test_script_entrypoint_reports_success() -> None:
    result = run_script()

    assert result.returncode == 0, result.stderr
    assert "OK: RA sweep orchestration passed." in result.stdout


def main() -> int:
    tests = [
        test_compliant_sweep_orchestration_passes,
        test_comments_and_strings_do_not_satisfy_shape,
        test_rejects_second_runner_owned_by_ra,
        test_rejects_missing_contract_readback,
        test_rejects_overwrite_prone_materialization,
        test_rejects_missing_source_fence_wiring,
        test_script_entrypoint_reports_success,
    ]
    for test in tests:
        test()
    print("OK: RA sweep orchestration verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
