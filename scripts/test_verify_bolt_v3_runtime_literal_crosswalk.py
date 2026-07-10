#!/usr/bin/env python3
"""Self-tests for the Wave-2 runtime-literal disposition receipt verifier."""

from __future__ import annotations

import hashlib
import importlib.util
import tempfile
from pathlib import Path


SCRIPT_PATH = Path(__file__).with_name("verify_bolt_v3_runtime_literal_crosswalk.py")
SPEC = importlib.util.spec_from_file_location("crosswalk_verifier", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
VERIFIER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFIER)


REGISTRY_ROWS = [
    ("src/a.rs", "string", '"field"', 'field: "field",'),
    ("src/b.rs", "number", "42", "let answer = 42;"),
    ("src/c.rs", "string", '"venue"', 'venue == "venue"'),
    ("src/d.rs", "string", '"API_KEY"', 'const BAD: &str = "API_KEY";'),
    ("src/e.rs", "string", '"wire"', 'json!({"wire": value})'),
    ("src/f.rs", "string", '"upstream"', 'matches!(value, "upstream")'),
    ("src/g.rs", "string", '"error"', 'bail!("error")'),
]


def registry_text(rows: list[tuple[str, str, str, str]] = REGISTRY_ROWS) -> str:
    blocks = []
    for path, kind, literal, context in rows:
        blocks.append(
            "\n".join(
                [
                    "[[allowed]]",
                    f'path = "{path}"',
                    f'kind = "{kind}"',
                    f"literal = '''{literal}'''",
                    f"context = '''{context}'''",
                    'classification = "fixture"',
                    'reason = "fixture"',
                ]
            )
        )
    return "\n\n".join(blocks) + "\n"


def toml_string(value: str) -> str:
    return "'''" + value.replace("'''", "'\\''") + "'''"


def entry_text(
    row_id: str,
    disposition: str,
    rationale: str,
    required_field: tuple[str, str] | None = None,
    *,
    phase: str = "planned",
    extra: tuple[str, str] | None = None,
) -> str:
    lines = [
        "[[entry]]",
        f'id = "{row_id}"',
        f'disposition = "{disposition}"',
        f'phase = "{phase}"',
        f'rationale = "{rationale}"',
    ]
    if required_field is not None:
        lines.append(f"{required_field[0]} = {toml_string(required_field[1])}")
    if extra is not None:
        lines.append(f"{extra[0]} = {toml_string(extra[1])}")
    return "\n".join(lines)


def good_crosswalk(registry: str, rows=REGISTRY_ROWS) -> str:
    digest = hashlib.sha256(registry.encode()).hexdigest()
    ids = VERIFIER.registry_row_ids(
        [
            {"path": path, "kind": kind, "literal": literal, "context": context}
            for path, kind, literal, context in rows
        ]
    )
    definitions = [
        ("DELETE_CODE_OWNED", "CODE_OWNED_IDENTIFIER", None),
        ("VALUE_TEST", "RUNTIME_VALUE_INVARIANT", ("test_symbol", "value_kat_exists")),
        ("TYPED_OWNER", "IDENTITY_KEY", ("owner_type", "ProviderKeyExists")),
        ("DENY_LIST", "FORBIDDEN_ENVIRONMENT_NAME", ("deny_list_const", "DENY_LIST_EXISTS")),
        ("COMPAT_FIXTURE", "PERSISTED_COMPATIBILITY", ("fixture_path", "tests/fixtures/old.json")),
        ("SOURCE_CONTRACT", "UPSTREAM_CONTRACT", ("source_contract", "upstream_contract_exists")),
        ("DELETE_DIAGNOSTIC", "DIAGNOSTIC_ONLY", None),
    ]
    entries = [
        entry_text(row_id, disposition, rationale, required)
        for row_id, (disposition, rationale, required) in zip(ids, definitions, strict=True)
    ]
    return "\n".join(
        [
            "schema_version = 1",
            f'registry_path = "{VERIFIER.REGISTRY_REL.as_posix()}"',
            f'registry_sha256 = "{digest}"',
            f"registry_row_count = {len(rows)}",
            f'stable_id_algorithm = "{VERIFIER.STABLE_ID_ALGORITHM}"',
            "",
            *entries,
            "",
        ]
    )


def good_inventory() -> str:
    return """\
schema_version = 1

[[sink]]
id = "launch-identity"
category = "PERSISTED_SERDE"
owner_path = "src/evidence.rs"
owner_symbol = "LaunchIdentityExists"
shape = ["profile", "pid"]
status = "PROTECTED"
protection = "OLD_BYTES"
evidence_path = "tests/fixtures/old.json"
evidence_symbol = "old_bytes_test_exists"

[[sink]]
id = "planned-wire"
category = "JSON_RPC"
owner_path = "src/evidence.rs"
owner_symbol = "wire_owner_exists"
shape = ["jsonrpc", "id"]
status = "UNPROTECTED"
protection = "NONE"
"""


def make_repo() -> tuple[tempfile.TemporaryDirectory[str], Path, str]:
    temp = tempfile.TemporaryDirectory()
    root = Path(temp.name)
    registry = registry_text()
    registry_path = root / VERIFIER.REGISTRY_REL
    registry_path.parent.mkdir(parents=True)
    registry_path.write_text(registry)
    crosswalk_path = root / VERIFIER.CROSSWALK_REL
    crosswalk_path.write_text(good_crosswalk(registry))
    inventory_path = root / VERIFIER.INVENTORY_REL
    inventory_path.write_text(good_inventory())
    (root / "src").mkdir(exist_ok=True)
    (root / "src/evidence.rs").write_text(
        "LaunchIdentityExists ProviderKeyExists DENY_LIST_EXISTS "
        "value_kat_exists upstream_contract_exists old_bytes_test_exists wire_owner_exists "
        "profile pid jsonrpc id"
    )
    fixture = root / "tests/fixtures/old.json"
    fixture.parent.mkdir(parents=True)
    fixture.write_text('{"profile":"p","pid":1}\n')
    return temp, root, hashlib.sha256(registry.encode()).hexdigest()


def assert_error(mutator, expected: str) -> None:
    temp, root, digest = make_repo()
    try:
        mutator(root)
        errors = VERIFIER.validate_repository(root, expected_registry_sha256=digest)
        assert any(expected in error for error in errors), errors
    finally:
        temp.cleanup()


def test_good_crosswalk_passes() -> None:
    temp, root, digest = make_repo()
    try:
        assert VERIFIER.validate_repository(root, expected_registry_sha256=digest) == []
    finally:
        temp.cleanup()


def test_anchor_mismatch_fails() -> None:
    def mutate(root: Path) -> None:
        path = root / VERIFIER.CROSSWALK_REL
        path.write_text(path.read_text().replace("registry_sha256 = \"", "registry_sha256 = \"0"))

    assert_error(mutate, "registry_sha256")


def test_missing_duplicate_and_unknown_rows_fail() -> None:
    def missing(root: Path) -> None:
        path = root / VERIFIER.CROSSWALK_REL
        text = path.read_text()
        path.write_text(text[: text.rfind("[[entry]]")])

    assert_error(missing, "missing registry row IDs")

    def duplicate(root: Path) -> None:
        path = root / VERIFIER.CROSSWALK_REL
        text = path.read_text()
        block = text[text.index("[[entry]]") : text.index("[[entry]]", text.index("[[entry]]") + 1)]
        path.write_text(text + "\n" + block)

    assert_error(duplicate, "duplicate crosswalk IDs")

    def unknown(root: Path) -> None:
        path = root / VERIFIER.CROSSWALK_REL
        path.write_text(path.read_text().replace('id = "rl_', 'id = "rl_deadbeef', 1))

    assert_error(unknown, "unknown crosswalk row IDs")


def test_invalid_enum_and_field_shapes_fail() -> None:
    def invalid_disposition(root: Path) -> None:
        path = root / VERIFIER.CROSSWALK_REL
        path.write_text(path.read_text().replace("DELETE_CODE_OWNED", "DELETE_SOMEDAY", 1))

    assert_error(invalid_disposition, "invalid disposition")

    def invalid_phase(root: Path) -> None:
        path = root / VERIFIER.CROSSWALK_REL
        path.write_text(path.read_text().replace('phase = "planned"', 'phase = "later"', 1))

    assert_error(invalid_phase, "invalid phase")

    def invalid_rationale(root: Path) -> None:
        path = root / VERIFIER.CROSSWALK_REL
        path.write_text(path.read_text().replace("CODE_OWNED_IDENTIFIER", "FREE PROSE", 1))

    assert_error(invalid_rationale, "invalid rationale")

    def missing_required(root: Path) -> None:
        path = root / VERIFIER.CROSSWALK_REL
        path.write_text(path.read_text().replace("test_symbol = '''value_kat_exists'''\n", "", 1))

    assert_error(missing_required, "requires test_symbol")

    def forbidden_extra(root: Path) -> None:
        path = root / VERIFIER.CROSSWALK_REL
        text = path.read_text().replace(
            'rationale = "CODE_OWNED_IDENTIFIER"',
            'rationale = "CODE_OWNED_IDENTIFIER"\ntest_symbol = \'\'\'not_allowed\'\'\'',
            1,
        )
        path.write_text(text)

    assert_error(forbidden_extra, "field test_symbol is not valid")


def test_done_references_must_exist() -> None:
    def missing_symbol(root: Path) -> None:
        path = root / VERIFIER.CROSSWALK_REL
        path.write_text(
            path.read_text()
            .replace('phase = "planned"', 'phase = "done"', 2)
            .replace("value_kat_exists", "absent_value_kat", 1)
        )

    assert_error(missing_symbol, "done reference does not exist")

    def missing_fixture(root: Path) -> None:
        path = root / VERIFIER.CROSSWALK_REL
        path.write_text(
            path.read_text()
            .replace('phase = "planned"', 'phase = "done"', 5)
            .replace("tests/fixtures/old.json", "tests/fixtures/missing.json", 1)
        )

    assert_error(missing_fixture, "done fixture_path does not exist")


def test_duplicate_tuple_ids_are_deterministic_and_distinct() -> None:
    row = {"path": "src/a.rs", "kind": "string", "literal": '"x"', "context": 'x == "x"'}
    ids = VERIFIER.registry_row_ids([row, row, row])
    assert len(set(ids)) == 3
    assert ids[0].endswith("-01")
    assert ids[1].endswith("-02")
    assert ids[2].endswith("-03")


def test_inventory_shape_and_evidence_fail_closed() -> None:
    def invalid_status(root: Path) -> None:
        path = root / VERIFIER.INVENTORY_REL
        path.write_text(path.read_text().replace("PROTECTED", "MAYBE", 1))

    assert_error(invalid_status, "invalid inventory status")

    def missing_shape(root: Path) -> None:
        path = root / VERIFIER.INVENTORY_REL
        path.write_text(path.read_text().replace('shape = ["profile", "pid"]\n', "", 1))

    assert_error(missing_shape, "non-empty shape")

    def stale_shape(root: Path) -> None:
        path = root / VERIFIER.INVENTORY_REL
        path.write_text(path.read_text().replace('"profile"', '"renamed_profile"', 1))

    assert_error(stale_shape, "shape token does not exist")

    def missing_evidence(root: Path) -> None:
        path = root / VERIFIER.INVENTORY_REL
        path.write_text(path.read_text().replace("old_bytes_test_exists", "absent_test", 1))

    assert_error(missing_evidence, "inventory evidence_symbol does not exist")


def main() -> int:
    tests = [value for name, value in sorted(globals().items()) if name.startswith("test_")]
    for test in tests:
        test()
    print(f"runtime literal crosswalk verifier self-test: ok ({len(tests)} tests)")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
