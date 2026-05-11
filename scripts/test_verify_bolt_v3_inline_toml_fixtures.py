#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 inline TOML fixture verifier."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bolt_v3_inline_toml_fixtures.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_bolt_v3_inline_toml_fixtures", SCRIPT)
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


def test_inline_v3_root_toml_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_probe.rs",
            r'''
                fn probe() {
                    let toml_text = r#"
schema_version = 1

[runtime]
mode = "live"

[clients.fixture]
venue = "POLYMARKET"
"#;
                    let _ = toml_text;
                }
            ''',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_probe.rs"
        assert "inline v3 TOML fixture" in findings[0].message


def test_json_raw_strings_and_fixture_file_reads_are_clean() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_probe.rs",
            r'''
                fn probe() {
                    let body = r#"{"base_fee":"0"}"#;
                    let toml_text = std::fs::read_to_string(
                        "tests/fixtures/bolt_v3/root.toml",
                    ).unwrap();
                    let _ = (body, toml_text);
                }
            ''',
        )

        assert verifier.scan_root(root) == []


def main() -> int:
    tests = [
        test_inline_v3_root_toml_is_a_finding,
        test_json_raw_strings_and_fixture_file_reads_are_clean,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 inline TOML fixture verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
