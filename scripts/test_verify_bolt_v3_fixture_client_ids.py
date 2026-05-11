#!/usr/bin/env python3
"""Self-tests for the Bolt-v3 fixture client-id verifier."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT = REPO_ROOT / "scripts" / "verify_bolt_v3_fixture_client_ids.py"


def load_verifier():
    spec = importlib.util.spec_from_file_location("verify_bolt_v3_fixture_client_ids", SCRIPT)
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


def test_fixture_client_id_literal_is_a_finding() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/fixtures/bolt_v3/root.toml",
            """
[clients.execution_fixture]
venue = "POLYMARKET"
""".lstrip(),
        )
        write_file(
            root,
            "tests/bolt_v3_client_registration.rs",
            'fn probe() { let _ = "execution_fixture"; }\n',
        )

        findings = verifier.scan_root(root)
        assert len(findings) == 1
        assert findings[0].path == "tests/bolt_v3_client_registration.rs"
        assert "fixture client-id literal" in findings[0].message


def test_derived_fixture_lookup_is_clean() -> None:
    verifier = load_verifier()
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        write_file(
            root,
            "tests/fixtures/bolt_v3/root.toml",
            """
[clients.execution_fixture]
venue = "POLYMARKET"
""".lstrip(),
        )
        write_file(
            root,
            "tests/bolt_v3_client_registration.rs",
            "fn probe(client_id: &str) { let _ = client_id; }\n",
        )

        assert verifier.scan_root(root) == []


def test_adapter_mapping_file_is_enforced() -> None:
    verifier = load_verifier()
    if "tests/bolt_v3_adapter_mapping.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("adapter mapping test file must be enforced")


def test_provider_binding_file_is_enforced() -> None:
    verifier = load_verifier()
    if "tests/bolt_v3_provider_binding.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("provider binding test file must be enforced")


def test_market_identity_file_is_enforced() -> None:
    verifier = load_verifier()
    if "tests/bolt_v3_market_identity.rs" not in verifier.ENFORCED_TEST_FILES:
        raise AssertionError("market identity test file must be enforced")


def main() -> int:
    tests = [
        test_fixture_client_id_literal_is_a_finding,
        test_derived_fixture_lookup_is_clean,
        test_adapter_mapping_file_is_enforced,
        test_provider_binding_file_is_enforced,
        test_market_identity_file_is_enforced,
    ]
    for test in tests:
        test()
    print("OK: Bolt-v3 fixture client-id verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
