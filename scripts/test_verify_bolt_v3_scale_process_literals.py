#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 scale-process fixture-literal verifier."""

from __future__ import annotations

import importlib.util
import shutil
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bolt_v3_scale_process_literals.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_bolt_v3_scale_process_literals", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError("scale-process verifier module should load")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_fixture(root: Path, files: dict[str, str]) -> None:
    for relative_path, content in files.items():
        path = root / relative_path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")


def test_scale_process_fixture_string_literal_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_scale_process_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        write_fixture(
            root,
            {
                "tests/fixtures/bolt_v3_existing_strategy/scale_process_selection_topic_isolation.toml": """
condition_suffix = "condition"
up_token_suffix = "up"
down_token_suffix = "down"
question_suffix = "question"
price_to_beat = 3100.0
event_settle_milliseconds = 50
delay_post_stop_seconds = 0
timeout_disconnection_seconds = 1
accepting_orders = true
liquidity_num = 0.0
""",
                "tests/bolt_v3_scale_process.rs": 'fn probe() { let _ = "condition"; }\n',
            },
        )

        findings = verifier.scan_root(root)

        assert findings
        assert findings[0].path == "tests/bolt_v3_scale_process.rs"
        assert "scale-process scenario fixture literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_scale_process_fixture_numeric_literal_is_a_finding() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_scale_process_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        write_fixture(
            root,
            {
                "tests/fixtures/bolt_v3_existing_strategy/scale_process_selection_topic_isolation.toml": """
condition_suffix = "condition"
up_token_suffix = "up"
down_token_suffix = "down"
question_suffix = "question"
price_to_beat = 3100.0
event_settle_milliseconds = 50
delay_post_stop_seconds = 0
timeout_disconnection_seconds = 1
accepting_orders = true
liquidity_num = 0.0
""",
                "tests/bolt_v3_scale_process.rs": "fn probe() { let price_to_beat = 3100.0; }\n",
            },
        )

        findings = verifier.scan_root(root)

        assert findings
        assert findings[0].path == "tests/bolt_v3_scale_process.rs"
        assert "scale-process scenario fixture literal" in findings[0].message
    finally:
        shutil.rmtree(root, ignore_errors=True)


def test_scale_process_fixture_helper_is_clean() -> None:
    verifier = load_verifier()
    root = REPO_ROOT / ".tmp_verify_bolt_v3_scale_process_literals"
    shutil.rmtree(root, ignore_errors=True)
    try:
        write_fixture(
            root,
            {
                "tests/fixtures/bolt_v3_existing_strategy/scale_process_selection_topic_isolation.toml": """
condition_suffix = "condition"
up_token_suffix = "up"
down_token_suffix = "down"
question_suffix = "question"
price_to_beat = 3100.0
event_settle_milliseconds = 50
delay_post_stop_seconds = 0
timeout_disconnection_seconds = 1
accepting_orders = true
liquidity_num = 0.0
""",
                "tests/bolt_v3_scale_process.rs": """
fn probe(scenario: &ScaleProcessScenario) {
    let _ = scenario.condition_suffix.as_str();
    let _ = scenario.price_to_beat;
}
""",
            },
        )

        assert verifier.scan_root(root) == []
    finally:
        shutil.rmtree(root, ignore_errors=True)


def main() -> int:
    tests = [
        test_scale_process_fixture_string_literal_is_a_finding,
        test_scale_process_fixture_numeric_literal_is_a_finding,
        test_scale_process_fixture_helper_is_clean,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 scale-process fixture-literal verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
