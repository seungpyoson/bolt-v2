#!/usr/bin/env python3
"""Validate the Wave-2 runtime-literal receipt and compatibility inventory."""

from __future__ import annotations

import hashlib
import sys
import tomllib
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any, Iterable


ROOT = Path(__file__).resolve().parents[1]
REGISTRY_REL = Path("docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml")
CROSSWALK_REL = Path("docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-crosswalk.toml")
INVENTORY_REL = Path("docs/bolt-v3/research/runtime-literals/bolt-v3-compatibility-inventory.toml")
PINNED_REGISTRY_SHA256 = "8c8be8c3f5a533d87fc474080a25569c6a316a94e91c861ac8ec1560386c2b18"
STABLE_ID_ALGORITHM = "sha256-length-framed-path-kind-literal-context-v1+duplicate-ordinal"

DISPOSITIONS = {
    "DELETE_CODE_OWNED",
    "DELETE_DIAGNOSTIC",
    "COMPAT_FIXTURE",
    "VALUE_TEST",
    "TYPED_OWNER",
    "DENY_LIST",
    "SOURCE_CONTRACT",
}
PHASES = {"planned", "done"}
RATIONALES = {
    "CODE_OWNED_IDENTIFIER",
    "DIAGNOSTIC_ONLY",
    "TEST_ONLY",
    "PERSISTED_COMPATIBILITY",
    "WIRE_COMPATIBILITY",
    "HASH_COMPATIBILITY",
    "RUNTIME_VALUE_INVARIANT",
    "IDENTITY_KEY",
    "FORBIDDEN_ENVIRONMENT_NAME",
    "UPSTREAM_CONTRACT",
    "UNCERTAIN_KEEP",
}
REQUIRED_FIELD = {
    "DELETE_CODE_OWNED": None,
    "DELETE_DIAGNOSTIC": None,
    "COMPAT_FIXTURE": "fixture_path",
    "VALUE_TEST": "test_symbol",
    "TYPED_OWNER": "owner_type",
    "DENY_LIST": "deny_list_const",
    "SOURCE_CONTRACT": "source_contract",
}
REFERENCE_FIELDS = {field for field in REQUIRED_FIELD.values() if field is not None}
ENTRY_FIELDS = {"id", "disposition", "phase", "rationale"} | REFERENCE_FIELDS
CROSSWALK_HEADER_FIELDS = {
    "schema_version",
    "registry_path",
    "registry_sha256",
    "registry_row_count",
    "stable_id_algorithm",
    "entry",
}

INVENTORY_CATEGORIES = {
    "PERSISTED_SERDE",
    "OPERATOR_JSON",
    "JSON_RPC",
    "NT_CATALOG_METADATA",
    "CANONICAL_HASH",
}
INVENTORY_STATUSES = {"PROTECTED", "UNPROTECTED"}
INVENTORY_PROTECTIONS = {
    "NONE",
    "VERSIONED_OLD_BYTES",
    "OLD_BYTES",
    "EXACT_WIRE",
    "EXACT_JSON_SHAPE",
    "FIXED_METADATA",
    "FIXED_DIGEST",
}
INVENTORY_FIELDS = {
    "id",
    "category",
    "owner_path",
    "owner_symbol",
    "shape",
    "status",
    "protection",
    "evidence_path",
    "evidence_symbol",
    "notes_code",
}
INVENTORY_NOTES = {
    "LEGACY_BYTES_READABLE",
    "EXTERNAL_WIRE_CONTRACT",
    "MACHINE_READABLE_OPERATOR_CONTRACT",
    "NAUTILUS_METADATA_CONTRACT",
    "HASH_PREIMAGE_CONTRACT",
    "VERSIONED_DECISION_EVIDENCE_MODEL",
    "PENDING_COMPATIBILITY_EVIDENCE",
}


def _length_frame(parts: Iterable[str]) -> bytes:
    framed = bytearray()
    for part in parts:
        encoded = part.encode("utf-8")
        framed.extend(len(encoded).to_bytes(8, "big"))
        framed.extend(encoded)
    return bytes(framed)


def _base_row_id(row: dict[str, Any]) -> str:
    fields = tuple(row.get(name) for name in ("path", "kind", "literal", "context"))
    if not all(isinstance(value, str) for value in fields):
        raise ValueError("registry row is missing a string path/kind/literal/context")
    return "rl_" + hashlib.sha256(_length_frame(fields)).hexdigest()


def registry_row_ids(rows: list[dict[str, Any]]) -> list[str]:
    """Return deterministic IDs, retaining duplicate pinned registry rows."""
    bases = [_base_row_id(row) for row in rows]
    totals = Counter(bases)
    seen: defaultdict[str, int] = defaultdict(int)
    result = []
    for base in bases:
        if totals[base] == 1:
            result.append(base)
            continue
        seen[base] += 1
        width = max(2, len(str(totals[base])))
        result.append(f"{base}-{seen[base]:0{width}d}")
    return result


def _read_toml(path: Path, label: str, errors: list[str]) -> dict[str, Any] | None:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        errors.append(f"{label} does not exist: {path}")
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as error:
        errors.append(f"cannot read {label} {path}: {error}")
    return None


def _sha256_file(path: Path, errors: list[str]) -> str | None:
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        errors.append(f"cannot hash registry {path}: {error}")
        return None


def _string(value: Any) -> bool:
    return isinstance(value, str) and bool(value.strip())


class SourceIndex:
    def __init__(self, root: Path) -> None:
        self.root = root
        self._text: str | None = None

    def contains(self, needle: str) -> bool:
        candidate = self.root / needle
        if candidate.is_file():
            return True
        if self._text is None:
            chunks: list[str] = []
            for top in ("src", "tests", "scripts"):
                base = self.root / top
                if not base.exists():
                    continue
                for path in base.rglob("*"):
                    if not path.is_file() or path.suffix not in {
                        ".rs",
                        ".py",
                        ".toml",
                        ".json",
                        ".jsonl",
                        ".txt",
                        ".md",
                    }:
                        continue
                    try:
                        chunks.append(path.read_text(encoding="utf-8"))
                    except (OSError, UnicodeError):
                        continue
            self._text = "\n".join(chunks)
        return needle in self._text


def _validate_crosswalk(
    root: Path,
    registry: dict[str, Any],
    live_sha256: str,
    expected_registry_sha256: str,
    source_index: SourceIndex,
) -> tuple[list[str], Counter[str]]:
    errors: list[str] = []
    data = _read_toml(root / CROSSWALK_REL, "crosswalk", errors)
    if data is None:
        return errors, Counter()

    unknown_header = set(data) - CROSSWALK_HEADER_FIELDS
    missing_header = CROSSWALK_HEADER_FIELDS - set(data)
    if unknown_header:
        errors.append(f"crosswalk has unknown top-level fields: {sorted(unknown_header)}")
    if missing_header:
        errors.append(f"crosswalk is missing top-level fields: {sorted(missing_header)}")
    if data.get("schema_version") != 1:
        errors.append("crosswalk schema_version must be 1")
    if data.get("registry_path") != REGISTRY_REL.as_posix():
        errors.append(f"crosswalk registry_path must be {REGISTRY_REL.as_posix()}")
    embedded_sha = data.get("registry_sha256")
    if embedded_sha != expected_registry_sha256:
        errors.append(
            "crosswalk registry_sha256 does not equal the hard-anchored registry SHA-256: "
            f"expected {expected_registry_sha256}, got {embedded_sha!r}"
        )
    if live_sha256 != expected_registry_sha256:
        errors.append(
            "live registry sha256 does not equal the hard anchor: "
            f"expected {expected_registry_sha256}, got {live_sha256}"
        )
    if embedded_sha != live_sha256:
        errors.append(
            f"crosswalk registry_sha256 {embedded_sha!r} does not match live file {live_sha256}"
        )
    if data.get("stable_id_algorithm") != STABLE_ID_ALGORITHM:
        errors.append(f"crosswalk stable_id_algorithm must be {STABLE_ID_ALGORITHM}")

    rows = registry.get("allowed")
    if not isinstance(rows, list):
        errors.append("registry must contain an allowed array")
        return errors, Counter()
    try:
        expected_ids = registry_row_ids(rows)
    except ValueError as error:
        errors.append(str(error))
        return errors, Counter()
    if data.get("registry_row_count") != len(rows):
        errors.append(
            f"crosswalk registry_row_count must be {len(rows)}, got {data.get('registry_row_count')!r}"
        )

    entries = data.get("entry")
    if not isinstance(entries, list):
        errors.append("crosswalk entry must be an array of tables")
        return errors, Counter()
    ids = [entry.get("id") for entry in entries if isinstance(entry, dict)]
    counts = Counter(ids)
    duplicates = sorted(str(row_id) for row_id, count in counts.items() if count > 1)
    if duplicates:
        errors.append(f"duplicate crosswalk IDs: {duplicates[:10]}")
    expected_set = set(expected_ids)
    actual_set = {row_id for row_id in ids if isinstance(row_id, str)}
    missing = sorted(expected_set - actual_set)
    unknown = sorted(actual_set - expected_set)
    if missing:
        errors.append(f"missing registry row IDs: {missing[:10]} (total {len(missing)})")
    if unknown:
        errors.append(f"unknown crosswalk row IDs: {unknown[:10]} (total {len(unknown)})")

    disposition_counts: Counter[str] = Counter()
    for index, entry in enumerate(entries, 1):
        label = f"crosswalk entry {index}"
        if not isinstance(entry, dict):
            errors.append(f"{label} must be a table")
            continue
        unknown_fields = set(entry) - ENTRY_FIELDS
        if unknown_fields:
            errors.append(f"{label} has unknown fields: {sorted(unknown_fields)}")
        for required in ("id", "disposition", "phase", "rationale"):
            if not _string(entry.get(required)):
                errors.append(f"{label} requires non-empty {required}")
        disposition = entry.get("disposition")
        if disposition not in DISPOSITIONS:
            errors.append(f"{label} has invalid disposition {disposition!r}")
            continue
        disposition_counts[disposition] += 1
        phase = entry.get("phase")
        if phase not in PHASES:
            errors.append(f"{label} has invalid phase {phase!r}")
        rationale = entry.get("rationale")
        if rationale not in RATIONALES:
            errors.append(f"{label} has invalid rationale {rationale!r}")
        required_field = REQUIRED_FIELD[disposition]
        if required_field is not None and not _string(entry.get(required_field)):
            errors.append(f"{label} disposition {disposition} requires {required_field}")
        for field in REFERENCE_FIELDS:
            if field != required_field and field in entry:
                errors.append(f"{label} field {field} is not valid for disposition {disposition}")
        if phase != "done" or required_field is None or not _string(entry.get(required_field)):
            continue
        reference = entry[required_field]
        if required_field == "fixture_path":
            if not (root / reference).is_file():
                errors.append(f"{label} done fixture_path does not exist: {reference}")
        elif not source_index.contains(reference):
            errors.append(f"{label} done reference does not exist in tree: {reference}")

    return errors, disposition_counts


def _validate_inventory(root: Path, source_index: SourceIndex) -> tuple[list[str], Counter[str]]:
    errors: list[str] = []
    data = _read_toml(root / INVENTORY_REL, "compatibility inventory", errors)
    if data is None:
        return errors, Counter()
    if set(data) - {"schema_version", "sink"}:
        errors.append(f"inventory has unknown top-level fields: {sorted(set(data) - {'schema_version', 'sink'})}")
    if data.get("schema_version") != 1:
        errors.append("inventory schema_version must be 1")
    sinks = data.get("sink")
    if not isinstance(sinks, list) or not sinks:
        errors.append("inventory sink must be a non-empty array of tables")
        return errors, Counter()
    ids = [sink.get("id") for sink in sinks if isinstance(sink, dict)]
    duplicates = sorted(str(sink_id) for sink_id, count in Counter(ids).items() if count > 1)
    if duplicates:
        errors.append(f"duplicate inventory IDs: {duplicates}")
    status_counts: Counter[str] = Counter()
    for index, sink in enumerate(sinks, 1):
        label = f"inventory sink {index}"
        if not isinstance(sink, dict):
            errors.append(f"{label} must be a table")
            continue
        unknown = set(sink) - INVENTORY_FIELDS
        if unknown:
            errors.append(f"{label} has unknown fields: {sorted(unknown)}")
        for field in ("id", "category", "owner_path", "owner_symbol", "status", "protection"):
            if not _string(sink.get(field)):
                errors.append(f"{label} requires non-empty {field}")
        shape = sink.get("shape")
        if not isinstance(shape, list) or not shape or not all(_string(value) for value in shape):
            errors.append(f"{label} requires a non-empty shape string array")
        category = sink.get("category")
        if category not in INVENTORY_CATEGORIES:
            errors.append(f"{label} has invalid inventory category {category!r}")
        status = sink.get("status")
        if status not in INVENTORY_STATUSES:
            errors.append(f"{label} has invalid inventory status {status!r}")
        else:
            status_counts[status] += 1
        protection = sink.get("protection")
        if protection not in INVENTORY_PROTECTIONS:
            errors.append(f"{label} has invalid inventory protection {protection!r}")
        notes_code = sink.get("notes_code")
        if notes_code is not None and notes_code not in INVENTORY_NOTES:
            errors.append(f"{label} has invalid notes_code {notes_code!r}")
        owner_path = sink.get("owner_path")
        owner_symbol = sink.get("owner_symbol")
        if _string(owner_path) and not (root / owner_path).is_file():
            errors.append(f"{label} owner_path does not exist: {owner_path}")
        elif _string(owner_path) and _string(owner_symbol):
            try:
                owner_text = (root / owner_path).read_text(encoding="utf-8")
            except (OSError, UnicodeError):
                owner_text = ""
            if owner_symbol not in owner_text:
                errors.append(f"{label} owner_symbol does not exist in owner_path: {owner_symbol}")
            if isinstance(shape, list):
                for token in shape:
                    if _string(token) and token not in owner_text:
                        errors.append(
                            f"{label} shape token does not exist in owner_path: {token}"
                        )
        if status == "UNPROTECTED":
            if protection != "NONE":
                errors.append(f"{label} UNPROTECTED sink must use protection NONE")
            if "evidence_path" in sink or "evidence_symbol" in sink:
                errors.append(f"{label} UNPROTECTED sink must not claim evidence")
        elif status == "PROTECTED":
            if protection == "NONE":
                errors.append(f"{label} PROTECTED sink cannot use protection NONE")
            evidence_path = sink.get("evidence_path")
            evidence_symbol = sink.get("evidence_symbol")
            if not _string(evidence_path) or not (root / evidence_path).is_file():
                errors.append(f"{label} inventory evidence_path does not exist: {evidence_path!r}")
            if not _string(evidence_symbol) or not source_index.contains(evidence_symbol):
                errors.append(f"{label} inventory evidence_symbol does not exist: {evidence_symbol!r}")
    return errors, status_counts


def validate_repository(
    root: Path = ROOT,
    *,
    expected_registry_sha256: str = PINNED_REGISTRY_SHA256,
) -> list[str]:
    errors: list[str] = []
    registry_path = root / REGISTRY_REL
    live_sha256 = _sha256_file(registry_path, errors)
    registry = _read_toml(registry_path, "registry", errors)
    if live_sha256 is None or registry is None:
        return errors
    source_index = SourceIndex(root)
    crosswalk_errors, _ = _validate_crosswalk(
        root, registry, live_sha256, expected_registry_sha256, source_index
    )
    inventory_errors, _ = _validate_inventory(root, source_index)
    return errors + crosswalk_errors + inventory_errors


def main() -> int:
    registry_errors: list[str] = []
    registry_path = ROOT / REGISTRY_REL
    live_sha256 = _sha256_file(registry_path, registry_errors)
    registry = _read_toml(registry_path, "registry", registry_errors)
    if live_sha256 is None or registry is None:
        for error in registry_errors:
            print(f"runtime-literal crosswalk verifier: {error}", file=sys.stderr)
        return 1
    source_index = SourceIndex(ROOT)
    crosswalk_errors, dispositions = _validate_crosswalk(
        ROOT, registry, live_sha256, PINNED_REGISTRY_SHA256, source_index
    )
    inventory_errors, statuses = _validate_inventory(ROOT, source_index)
    errors = registry_errors + crosswalk_errors + inventory_errors
    if errors:
        for error in errors:
            print(f"runtime-literal crosswalk verifier: {error}", file=sys.stderr)
        return 1
    print(
        "runtime-literal crosswalk verifier: ok "
        f"(rows={sum(dispositions.values())}, dispositions={dict(sorted(dispositions.items()))}, "
        f"inventory={dict(sorted(statuses.items()))})"
    )
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
