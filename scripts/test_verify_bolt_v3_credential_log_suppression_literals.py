#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 credential-log suppression test-literal verifier."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bolt_v3_credential_log_suppression_literals.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location(
        "verify_bolt_v3_credential_log_suppression_literals", SCRIPT
    )
    if spec is None or spec.loader is None:
        raise AssertionError(f"failed to load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def write_file(root: Path, relative_path: str, text: str) -> None:
    path = root / relative_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def test_credential_log_suppression_sleep_duration_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_credential_log_suppression.rs",
            "fn probe() { std::thread::sleep(std::time::Duration::from_millis(500)); }\n",
        )

        findings = verifier.scan_root(root)

        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_credential_log_suppression.rs"
        assert "credential-log suppression timing literal" in findings[0].message


def main() -> int:
    tests = [test_credential_log_suppression_sleep_duration_literal_is_a_finding]
    for test in tests:
        test()
    print("OK: Bolt-v3 credential-log suppression literal verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
