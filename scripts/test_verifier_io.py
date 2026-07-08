#!/usr/bin/env python3
"""Self-tests for shared verifier I/O contracts."""

from __future__ import annotations

import ast
from collections import Counter
from pathlib import Path

import verifier_io


SCRIPTS_DIR = Path(__file__).resolve().parent
DISCOVERY_CONTRACT_HELPERS = {"require_nonempty", "require_declared_source_files"}


def module_string_constants(tree: ast.Module) -> dict[str, str]:
    constants: dict[str, str] = {}
    for node in tree.body:
        if not isinstance(node, ast.Assign) or len(node.targets) != 1:
            continue
        target = node.targets[0]
        if (
            isinstance(target, ast.Name)
            and isinstance(node.value, ast.Constant)
            and isinstance(node.value.value, str)
        ):
            constants[target.id] = node.value.value
    return constants


def require_nonempty_contract_counts() -> Counter[tuple[str, str]]:
    counts: Counter[tuple[str, str]] = Counter()
    for path in sorted(SCRIPTS_DIR.glob("verify_*.py")):
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
        constants = module_string_constants(tree)
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call):
                continue
            function = node.func
            name = None
            if isinstance(function, ast.Name):
                name = function.id
            elif isinstance(function, ast.Attribute):
                name = function.attr
            if name not in DISCOVERY_CONTRACT_HELPERS:
                continue
            if len(node.args) < 2:
                raise AssertionError(f"{path.name}:{node.lineno}: discovery contract helper missing label")
            label_node = node.args[1]
            if isinstance(label_node, ast.Constant) and isinstance(label_node.value, str):
                label = label_node.value
            elif isinstance(label_node, ast.Name) and label_node.id in constants:
                label = constants[label_node.id]
            else:
                raise AssertionError(
                    f"{path.name}:{node.lineno}: discovery contract helper label must be a string "
                    "literal or module-level string constant"
                )
            counts[(path.name, label)] += 1
    return counts


def test_required_discovery_floor_invariant_is_marked_next_to_helper() -> None:
    invariant = verifier_io.REQUIRED_DISCOVERY_FLOOR_INVARIANT
    required_terms = (
        "preflight-terminal",
        "only the relevant floor finding",
        "scan work",
        "stale",
        "allowlist",
        "config validation",
        "ledger",
        "source-fence wiring",
        "missing-file",
        "supplemental",
    )
    missing = [term for term in required_terms if term not in invariant]
    if missing:
        raise AssertionError(f"invariant text is missing required term(s): {missing}")
    if "preflight-terminal" not in (verifier_io.require_nonempty.__doc__ or ""):
        raise AssertionError("require_nonempty docstring must name the preflight-terminal invariant")


def test_required_discovery_floor_contract_registry_matches_live_call_sites() -> None:
    actual = require_nonempty_contract_counts()
    registered = Counter(
        (contract.verifier, contract.label)
        for contract in verifier_io.REQUIRED_DISCOVERY_FLOOR_CONTRACTS
        for _ in range(contract.call_count)
    )
    if registered != actual:
        missing = actual - registered
        extra = registered - actual
        raise AssertionError(
            "required discovery floor contract registry drifted: "
            f"missing={dict(missing)}, extra={dict(extra)}"
        )


def test_required_discovery_floor_contracts_are_classified_and_proven() -> None:
    allowed_classifications = {
        "entrypoint-terminal",
        "aggregate-then-terminal",
        "helper-and-entrypoint-terminal",
        "helper-terminal",
    }
    for contract in verifier_io.REQUIRED_DISCOVERY_FLOOR_CONTRACTS:
        if contract.classification not in allowed_classifications:
            raise AssertionError(f"{contract}: invalid classification")
        if not contract.entrypoint.strip():
            raise AssertionError(f"{contract}: missing entrypoint")
        if not contract.proof.strip():
            raise AssertionError(f"{contract}: missing proof")
        if contract.call_count <= 0:
            raise AssertionError(f"{contract}: call_count must be positive")


def test_rust_snippet_requirements_ignore_comments_and_literals() -> None:
    findings: list[str] = []
    verifier_io.require_rust_snippets(
        Path("src/probe.rs"),
        "\n".join(
            (
                "// fn required_runtime_call() {}",
                'const _LABEL: &str = "RequiredType";',
                "fn real_runtime_call() {}",
            )
        ),
        ("required_runtime_call", "RequiredType", "real_runtime_call"),
        findings,
    )
    expected = [
        "src/probe.rs: missing `required_runtime_call`",
        "src/probe.rs: missing `RequiredType`",
    ]
    if findings != expected:
        raise AssertionError(f"Rust snippet stripping did not reject comments/literals: {findings}")


def main() -> int:
    tests = [
        test_required_discovery_floor_invariant_is_marked_next_to_helper,
        test_required_discovery_floor_contract_registry_matches_live_call_sites,
        test_required_discovery_floor_contracts_are_classified_and_proven,
        test_rust_snippet_requirements_ignore_comments_and_literals,
    ]
    for test in tests:
        test()
    print("OK: verifier_io self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
