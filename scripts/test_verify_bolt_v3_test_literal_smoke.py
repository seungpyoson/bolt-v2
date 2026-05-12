#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 broad test-literal smoke verifier."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bolt_v3_test_literal_smoke.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location(
        "verify_bolt_v3_test_literal_smoke", SCRIPT
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


def test_obvious_runtime_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_probe.rs",
            "fn probe() { std::time::Duration::from_millis(5); }\n",
        )

        findings = verifier.scan_root(root)

        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_probe.rs"
        assert "literal matched smoke pattern" in findings[0].message


def test_market_identity_boundary_guard_sentinel_is_allowed() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_market_identity.rs",
            """
fn core_market_identity_module_must_be_market_family_neutral() {
    let src = include_str!("../src/bolt_v3_market_identity.rs");
    let forbidden = [
        // Updown family identifier and prose forms.
        "updown",
        "Updown",
    ];
    for symbol in forbidden {
        assert!(!src.contains(symbol));
    }
}
""".lstrip(),
        )

        findings = verifier.scan_root(root)

        assert findings == []


def test_provider_env_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/bolt_v3_probe.rs",
            'fn probe() { let _ = "POLYMARKET_PK"; }\n',
        )

        findings = verifier.scan_root(root)

        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_probe.rs"


def main() -> int:
    tests = [
        test_obvious_runtime_literal_is_a_finding,
        test_market_identity_boundary_guard_sentinel_is_allowed,
        test_provider_env_literal_is_a_finding,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 test-literal smoke verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
