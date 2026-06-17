#!/usr/bin/env python3
"""Self-tests for Research Analytics point-in-time leakage fixtures."""

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_ra_point_in_time_leakage.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_ra_point_in_time_leakage", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_fixture(root: Path, name: str, body: str) -> Path:
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")
    return path


def fixture_text(
    *,
    expected: str = "pass",
    feature_event_time: int = 900,
    feature_availability_time: int = 1_000,
    observation_event_time: int = 1_000,
    observation_as_of_time: int = 1_100,
    max_staleness_nanos: int = 500,
    source_hash: str = "sha256:source",
    query_hash: str = "sha256:query",
    config_hash: str = "sha256:config",
) -> str:
    return f"""
[case]
expected = "{expected}"

[dataset]
source_hash = "{source_hash}"
query_hash = "{query_hash}"
config_hash = "{config_hash}"

[rules]
join_keys = ["market_id"]
observation_event_time = "signal_event_time_nanos"
observation_as_of_time = "signal_as_of_time_nanos"
feature_event_time = "feature_event_time_nanos"
feature_availability_time = "feature_availability_time_nanos"
feature_source_hash = "feature_source_hash"
max_staleness_nanos = {max_staleness_nanos}

[[observations]]
market_id = "binary-option"
signal_event_time_nanos = {observation_event_time}
signal_as_of_time_nanos = {observation_as_of_time}

[[features]]
market_id = "binary-option"
feature_event_time_nanos = {feature_event_time}
feature_availability_time_nanos = {feature_availability_time}
feature_source_hash = "sha256:feature"
"""


def run_script(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def test_valid_asof_join_has_no_findings() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        path = write_fixture(Path(tmp), "valid.toml", fixture_text())

        findings = verifier.validate_fixture(path)

    assert findings == []


def test_future_availability_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        path = write_fixture(
            Path(tmp),
            "future-availability.toml",
            fixture_text(expected="fail", feature_availability_time=1_200),
        )

        findings = verifier.validate_fixture(path)

    assert any("feature_availability_time_nanos" in finding for finding in findings)


def test_future_event_time_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        path = write_fixture(
            Path(tmp),
            "future-event.toml",
            fixture_text(expected="fail", feature_event_time=1_100),
        )

        findings = verifier.validate_fixture(path)

    assert any("feature_event_time_nanos" in finding for finding in findings)


def test_missing_hashes_fail_closed() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        path = write_fixture(
            Path(tmp),
            "missing-hash.toml",
            fixture_text(expected="fail", source_hash=""),
        )

        findings = verifier.validate_fixture(path)

    assert any("dataset.source_hash" in finding for finding in findings)


def test_stale_feature_fails_freshness_rule() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        path = write_fixture(
            Path(tmp),
            "stale.toml",
            fixture_text(
                expected="fail",
                feature_availability_time=400,
                observation_as_of_time=1_100,
                max_staleness_nanos=500,
            ),
        )

        findings = verifier.validate_fixture(path)

    assert any("max_staleness_nanos" in finding for finding in findings)


def test_fixture_expectations_must_match_actual_result() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(root, "valid.toml", fixture_text(expected="pass"))
        write_fixture(
            root,
            "future-availability.toml",
            fixture_text(expected="fail", feature_availability_time=1_200),
        )

        failures = verifier.validate_fixture_dir(root)

    assert failures == []


def test_cli_fails_when_leaky_fixture_is_labeled_pass() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_fixture(
            root,
            "mislabeled.toml",
            fixture_text(expected="pass", feature_availability_time=1_200),
        )

        result = run_script("--fixture-dir", str(root))

    assert result.returncode == 1
    assert "expected pass but failed" in result.stderr
    assert "feature_availability_time_nanos" in result.stderr


def main() -> int:
    tests = [
        test_valid_asof_join_has_no_findings,
        test_future_availability_is_a_finding,
        test_future_event_time_is_a_finding,
        test_missing_hashes_fail_closed,
        test_stale_feature_fails_freshness_rule,
        test_fixture_expectations_must_match_actual_result,
        test_cli_fails_when_leaky_fixture_is_labeled_pass,
    ]
    for test in tests:
        test()
    print("OK: RA point-in-time leakage verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
