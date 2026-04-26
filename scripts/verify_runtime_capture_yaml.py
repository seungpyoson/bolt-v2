#!/usr/bin/env python3
"""Mechanical verifier for bolt-v3 runtime-capture research YAMLs.

Inputs:
  docs/bolt-v3/research/runtime-capture/nt-msgbus-surfaces.yaml
  docs/bolt-v3/research/runtime-capture/storage-feasibility.yaml
  src/nt_runtime_capture.rs
  tests/nt_runtime_capture.rs

Checks:
  1. No stale pre-rename runtime capture references in any input.
  2. Every nt_path in storage-feasibility.yaml has a `:N` or `:M-N` line ref.
  3. Every captured_now row in nt-msgbus-surfaces.yaml has explicit
     source_subscribe_fn / bolt_pattern_helper fields, and that helper appears
     in src/nt_runtime_capture.rs.
  4. Every safe_missing_passive_stream row contains both publisher and
     subscriber evidence in its reason / storage_evidence / topic_evidence /
     nt_path text.
  5. The events.risk / TradingStateChanged surface is bolt_status=captured_now
     AND src/nt_runtime_capture.rs writes to risk/trading_state_changed.jsonl.
  6. OrderBookDeltas recommended_storage in storage-feasibility.yaml is NOT
     feather (the container has no Arrow impl; bolt unwraps per-delta).
  7. recommended_storage values are only:
        boundary_wrapper, feather, jsonl, none, skip, unwrap_to_orderbookdelta,
        wrapper_required.
  8. api_kind values are only:
        cleanup_helper, command_endpoint, endpoint_or_command, passive_pubsub,
        publish_helper, request_response, topic_builder.
  9. Every pinned NT `pub fn subscribe_*` msgbus API appears in
     nt-msgbus-surfaces.yaml.
 10. Runtime-capture surface storage recommendations match
     storage-feasibility.yaml for the same message_type.
 11. Every subscribe_* call in src/nt_runtime_capture.rs is represented by a
     captured_now YAML row with the same source_subscribe_fn and
     bolt_pattern_helper.
 12. Every captured_now row's capture_stream / storage_format matches
     bolt-current-capture.yaml.
 13. Cargo.toml, the naming audit, the runtime contract, and the pinned NT
     checkout path agree on the NautilusTrader revision.
 14. Every captured_now stream in bolt-current-capture.yaml is represented
     by nt-msgbus-surfaces.yaml and its listed tests exist.

Run:
  python3 scripts/verify_runtime_capture_yaml.py

Exit 0 if all checks pass; exit 1 with a per-finding report otherwise.
Stdlib only, plus PyYAML.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

try:
    import yaml
except ImportError:
    sys.stderr.write(
        "ERROR: PyYAML is required. Install with `python3 -m pip install pyyaml`.\n"
    )
    sys.exit(2)


REPO_ROOT = Path(__file__).resolve().parent.parent
SURFACES_PATH = (
    REPO_ROOT
    / "docs"
    / "bolt-v3"
    / "research"
    / "runtime-capture"
    / "nt-msgbus-surfaces.yaml"
)
FEAS_PATH = (
    REPO_ROOT
    / "docs"
    / "bolt-v3"
    / "research"
    / "runtime-capture"
    / "storage-feasibility.yaml"
)
CURRENT_CAPTURE_PATH = (
    REPO_ROOT
    / "docs"
    / "bolt-v3"
    / "research"
    / "runtime-capture"
    / "bolt-current-capture.yaml"
)
NAMING_AUDIT_PATH = (
    REPO_ROOT
    / "docs"
    / "bolt-v3"
    / "research"
    / "naming"
    / "nt-owned-name-audit.yaml"
)
RUNTIME_CONTRACTS_PATH = (
    REPO_ROOT / "docs" / "bolt-v3" / "2026-04-25-bolt-v3-runtime-contracts.md"
)
SRC_PATH = REPO_ROOT / "src" / "nt_runtime_capture.rs"
TEST_PATH = REPO_ROOT / "tests" / "nt_runtime_capture.rs"
NT_API_PATH_TEMPLATE = (
    ".cargo/git/checkouts/nautilus_trader-*/*/"
    "crates/common/src/msgbus/api.rs"
)

ALLOWED_STORAGE = {
    "feather",
    "jsonl",
    "boundary_wrapper",
    "unwrap_to_orderbookdelta",
    "wrapper_required",
    "skip",
    "none",
}
ALLOWED_API_KIND = {
    "passive_pubsub",
    "publish_helper",
    "command_endpoint",
    "request_response",
    "endpoint_or_command",
    "cleanup_helper",
    "topic_builder",
}

STALE_REFS = (
    "normalized" + "_" + "s" + "ink",
    "wire_" + "normalized" + "_" + "s" + "inks",
)
PUB_EVIDENCE_KEYWORDS = (
    "publish_",
    "broadcasts",
    "emitted",
    "emits",
    "publishes",
    "produces",
    "produced",
    "engine/mod.rs",
    "RiskEngine",
)
SUB_EVIDENCE_KEYWORDS = (
    "subscribe_",
    "TypedHandler",
    "ShareableMessageHandler",
    "api.rs:",
)
LINE_REF_RE = re.compile(r":\d+(?:-\d+)?\b")
PATTERN_HELPER_RE = re.compile(r"\b([a-z][a-z0-9_]*_pattern)\(\)")
SUBSCRIBE_CALL_RE = re.compile(r"\b(subscribe_[a-z0-9_]+)\s*\(", re.MULTILINE)
SUBSCRIBE_FN_RE = re.compile(r"^pub fn (subscribe_[a-z0-9_]+)\b", re.MULTILINE)
TEST_FN_RE_TEMPLATE = r"\b(?:async\s+)?fn\s+{}\s*\("
RISK_JSONL_PATH_FRAGMENT = 'join("risk")'
RISK_JSONL_FILENAME = "trading_state_changed.jsonl"


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def load_yaml(path: Path):
    return yaml.safe_load(read(path))


def cargo_nautilus_revision(findings: list[tuple[str, str]]) -> str | None:
    cargo_text = read(REPO_ROOT / "Cargo.toml")
    dependency_rows = re.findall(
        r'^\s*(nautilus-[\w-]+)\s*=\s*\{([^\n]*)\}',
        cargo_text,
        re.MULTILINE,
    )
    if not dependency_rows:
        findings.append(
            (
                "13.pin_revision_missing",
                "Cargo.toml has no nautilus-* git dependencies to verify",
            )
        )
        return None

    revs: set[str] = set()
    for crate, body in dependency_rows:
        if "nautechsystems/nautilus_trader.git" not in body:
            findings.append(
                (
                    "13.pin_revision_mismatch",
                    f"Cargo.toml dependency {crate} is not sourced from NautilusTrader git",
                )
            )
            continue
        match = re.search(r'\brev\s*=\s*"([0-9a-f]{40})"', body)
        if not match:
            findings.append(
                (
                    "13.pin_revision_missing",
                    f"Cargo.toml dependency {crate} lacks a 40-character rev pin",
                )
            )
            continue
        revs.add(match.group(1))
    if len(revs) != 1:
        findings.append(
            (
                "13.pin_revision_mismatch",
                f"expected exactly one NautilusTrader rev in Cargo.toml, found {sorted(revs)}",
            )
        )
        return None
    return next(iter(revs))


def strip_rust_comments_and_strings(text: str) -> str:
    result: list[str] = []
    i = 0
    while i < len(text):
        if text.startswith("//", i):
            end = text.find("\n", i)
            if end == -1:
                break
            result.append("\n")
            i = end + 1
            continue
        if text.startswith("/*", i):
            end = text.find("*/", i + 2)
            if end == -1:
                break
            result.append("\n" * text[i : end + 2].count("\n"))
            i = end + 2
            continue
        char = text[i]
        if char == '"':
            result.append('""')
            i += 1
            while i < len(text):
                if text[i] == "\\":
                    i += 2
                    continue
                if text[i] == '"':
                    i += 1
                    break
                i += 1
            continue
        if char == "'":
            result.append("''")
            i += 1
            while i < len(text):
                if text[i] == "\\":
                    i += 2
                    continue
                if text[i] == "'":
                    i += 1
                    break
                i += 1
            continue
        result.append(char)
        i += 1
    return "".join(result)


def first_top_level_arg(call_body: str) -> str:
    depth = 0
    for idx, char in enumerate(call_body):
        if char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
        elif char == "," and depth == 0:
            return call_body[:idx]
    return call_body


def find_pinned_nt_api_path(
    findings: list[tuple[str, str]], nautilus_revision: str | None
) -> Path | None:
    if not nautilus_revision:
        return None
    short_rev = nautilus_revision[:7]
    matches = sorted(
        path
        for path in Path.home().glob(NT_API_PATH_TEMPLATE)
        if path.parents[4].name == short_rev
    )
    if len(matches) != 1:
        findings.append(
            (
                "9.pinned_nt_api_missing",
                f"expected exactly one pinned NT msgbus api.rs at "
                f"~/{NT_API_PATH_TEMPLATE} with short rev {short_rev}, found {len(matches)}",
            )
        )
        return None
    return matches[0]


def source_subscribe_calls(
    src_text: str, findings: list[tuple[str, str]]
) -> set[tuple[str, str]]:
    src_text = strip_rust_comments_and_strings(src_text)
    calls: set[tuple[str, str]] = set()
    for match in SUBSCRIBE_CALL_RE.finditer(src_text):
        fn_name = match.group(1)
        open_idx = src_text.find("(", match.start())
        if open_idx == -1:
            continue
        depth = 0
        close_idx = None
        for idx in range(open_idx, len(src_text)):
            char = src_text[idx]
            if char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
                if depth == 0:
                    close_idx = idx
                    break
        if close_idx is None:
            findings.append(
                (
                    "11.source_subscribe_parse_failed",
                    f"could not find closing parenthesis for {fn_name} call near byte {match.start()}",
                )
            )
            continue
        body = src_text[open_idx + 1 : close_idx]
        pattern_arg = first_top_level_arg(body)
        if "MStr::pattern" in pattern_arg:
            findings.append(
                (
                    "11.source_subscribe_literal_pattern",
                    f"src/nt_runtime_capture.rs calls {fn_name} with inline MStr::pattern; "
                    f"use a named *_pattern() helper and declare it in YAML",
                )
            )
        helpers = [
            helper.removesuffix("()")
            for helper in PATTERN_HELPER_RE.findall(pattern_arg)
        ]
        if len(helpers) != 1:
            findings.append(
                (
                    "11.source_subscribe_no_pattern_helper",
                    f"src/nt_runtime_capture.rs calls {fn_name} with "
                    f"{len(helpers)} *_pattern() helpers in the first argument; expected exactly one",
                )
            )
            continue
        calls.add((fn_name, helpers[0]))
    return calls


def collect_failures() -> list[tuple[str, str]]:
    """Return [(check_id, message)] for every detected violation."""
    findings: list[tuple[str, str]] = []

    surfaces_doc = load_yaml(SURFACES_PATH) or {}
    feas_doc = load_yaml(FEAS_PATH) or {}
    current_capture_doc = load_yaml(CURRENT_CAPTURE_PATH) or {}
    naming_audit_doc = load_yaml(NAMING_AUDIT_PATH) or {}
    src_text = read(SRC_PATH)
    test_text = read(TEST_PATH)
    surfaces_text = read(SURFACES_PATH)
    feas_text = read(FEAS_PATH)
    runtime_contracts_text = read(RUNTIME_CONTRACTS_PATH)

    surfaces = (
        surfaces_doc.get("surfaces", []) if isinstance(surfaces_doc, dict) else []
    )
    feas_types = feas_doc.get("types", []) if isinstance(feas_doc, dict) else []
    current_streams = (
        current_capture_doc.get("captured_streams", [])
        if isinstance(current_capture_doc, dict)
        else []
    )
    nautilus_revision = cargo_nautilus_revision(findings)

    # Check 1: No stale pre-rename runtime capture references.
    for label, content in (
        ("nt-msgbus-surfaces.yaml", surfaces_text),
        ("storage-feasibility.yaml", feas_text),
        ("src/nt_runtime_capture.rs", src_text),
        ("tests/nt_runtime_capture.rs", test_text),
    ):
        for ref in STALE_REFS:
            if ref in content:
                findings.append(
                    (
                        "1.stale_ref",
                        f"{label} contains stale reference {ref!r}",
                    )
                )

    # Check 2: every nt_path in storage-feasibility.yaml has a :N or :M-N line ref.
    for row in feas_types:
        nt_path = str(row.get("nt_path", ""))
        if not LINE_REF_RE.search(nt_path):
            findings.append(
                (
                    "2.nt_path_line_ref",
                    f"storage-feasibility row message_type="
                    f"{row.get('message_type')!r} nt_path lacks :N or :M-N "
                    f"line reference: {nt_path!r}",
                )
            )

    # Check 3: every captured_now surfaces row explicitly names its source
    # subscribe function and pattern helper, and that helper is defined in
    # src/nt_runtime_capture.rs.
    captured_rows = [r for r in surfaces if r.get("bolt_status") == "captured_now"]
    for row in captured_rows:
        source_subscribe_fn = row.get("source_subscribe_fn")
        nt_api_base = str(row.get("nt_api", "")).split("(")[0]
        helper = row.get("bolt_pattern_helper")
        capture_stream = row.get("capture_stream")
        storage_format = row.get("storage_format")
        if not source_subscribe_fn:
            findings.append(
                (
                    "3.captured_now_missing_source_subscribe_fn",
                    f"surfaces row nt_api={row.get('nt_api')!r} "
                    f"(captured_now) lacks source_subscribe_fn",
                )
            )
        elif nt_api_base != source_subscribe_fn:
            findings.append(
                (
                    "3.captured_now_nt_api_mismatch",
                    f"surfaces row nt_api={row.get('nt_api')!r} has "
                    f"source_subscribe_fn={source_subscribe_fn!r}; expected {nt_api_base!r}",
                )
            )
        if not helper:
            findings.append(
                (
                    "3.captured_now_missing_pattern_helper",
                    f"surfaces row nt_api={row.get('nt_api')!r} "
                    f"(captured_now) lacks bolt_pattern_helper",
                )
            )
        if not capture_stream:
            findings.append(
                (
                    "3.captured_now_missing_capture_stream",
                    f"surfaces row nt_api={row.get('nt_api')!r} "
                    f"(captured_now) lacks capture_stream",
                )
            )
        if not storage_format:
            findings.append(
                (
                    "3.captured_now_missing_storage_format",
                    f"surfaces row nt_api={row.get('nt_api')!r} "
                    f"(captured_now) lacks storage_format",
                )
            )
        if helper and f"fn {helper}()" not in src_text:
            findings.append(
                (
                    "3.captured_now_pattern_missing_in_src",
                    f"surfaces row nt_api={row.get('nt_api')!r} "
                    f"references {helper}() but no `fn {helper}()` "
                    f"is defined in src/nt_runtime_capture.rs",
                )
            )

    # Check 4: safe_missing_passive_stream rows must carry publisher AND
    # subscriber evidence somewhere in their text fields.
    safe_rows = [
        r for r in surfaces if r.get("bolt_status") == "safe_missing_passive_stream"
    ]
    for row in safe_rows:
        blob = " ".join(
            str(row.get(k, ""))
            for k in (
                "reason",
                "storage_evidence",
                "topic_evidence",
                "publisher_evidence",
                "subscriber_evidence",
                "nt_path",
            )
        )
        if not any(kw in blob for kw in PUB_EVIDENCE_KEYWORDS):
            findings.append(
                (
                    "4.safe_missing_no_publisher_evidence",
                    f"surfaces row nt_api={row.get('nt_api')!r} "
                    f"(safe_missing_passive_stream) lacks publisher evidence "
                    f"(none of {list(PUB_EVIDENCE_KEYWORDS)} found)",
                )
            )
        if not any(kw in blob for kw in SUB_EVIDENCE_KEYWORDS):
            findings.append(
                (
                    "4.safe_missing_no_subscriber_evidence",
                    f"surfaces row nt_api={row.get('nt_api')!r} "
                    f"(safe_missing_passive_stream) lacks subscriber evidence "
                    f"(none of {list(SUB_EVIDENCE_KEYWORDS)} found)",
                )
            )

    # Check 5: events.risk / TradingStateChanged is captured_now and writes
    # risk/trading_state_changed.jsonl.
    risk_rows = [
        r
        for r in surfaces
        if r.get("storage_message_type") == "TradingStateChanged"
        or r.get("message_type") == "TradingStateChanged"
        or r.get("topic_pattern") == "events.risk"
    ]
    if not risk_rows:
        findings.append(
            (
                "5.risk_row_missing",
                "no surfaces row found for TradingStateChanged or topic_pattern=events.risk",
            )
        )
    else:
        for row in risk_rows:
            if row.get("bolt_status") != "captured_now":
                findings.append(
                    (
                        "5.risk_not_captured_now",
                        f"surfaces row nt_api={row.get('nt_api')!r} "
                        f"(message_type=TradingStateChanged) "
                        f"bolt_status={row.get('bolt_status')!r}, "
                        f"expected captured_now",
                    )
                )
    if (
        RISK_JSONL_FILENAME not in src_text
        or RISK_JSONL_PATH_FRAGMENT not in src_text
    ):
        findings.append(
            (
                "5.risk_jsonl_path_missing_in_src",
                f"src/nt_runtime_capture.rs does not write to risk/"
                f"{RISK_JSONL_FILENAME} (expected join(\"risk\") "
                f"+ join(\"{RISK_JSONL_FILENAME}\"))",
            )
        )

    # Check 6: OrderBookDeltas recommended_storage must NOT be feather.
    deltas_rows = [
        r for r in feas_types if r.get("message_type") == "OrderBookDeltas"
    ]
    if not deltas_rows:
        findings.append(
            (
                "6.deltas_row_missing",
                "no storage-feasibility row found for OrderBookDeltas",
            )
        )
    for row in deltas_rows:
        v = row.get("recommended_storage")
        if v == "feather":
            findings.append(
                (
                    "6.deltas_storage_feather",
                    f"OrderBookDeltas recommended_storage={v!r}; "
                    f"the container has no Arrow impl, bolt unwraps "
                    f"per-delta — must not be 'feather'",
                )
            )

    # Check 7: recommended_storage values bounded.
    for row in feas_types:
        v = row.get("recommended_storage")
        if v is None:
            findings.append(
                (
                    "7.storage_missing",
                    f"storage-feasibility row message_type="
                    f"{row.get('message_type')!r} has no recommended_storage",
                )
            )
            continue
        if v not in ALLOWED_STORAGE:
            findings.append(
                (
                    "7.storage_not_allowed",
                    f"storage-feasibility row message_type="
                    f"{row.get('message_type')!r} recommended_storage={v!r} "
                    f"not in {sorted(ALLOWED_STORAGE)}",
                )
            )
    for row in surfaces:
        v = row.get("suggested_capture_storage")
        if v is None:
            continue
        if v not in ALLOWED_STORAGE:
            findings.append(
                (
                    "7.storage_not_allowed",
                    f"surfaces row nt_api={row.get('nt_api')!r} "
                    f"suggested_capture_storage={v!r} not in "
                    f"{sorted(ALLOWED_STORAGE)}",
                )
            )

    # Check 8: api_kind values bounded.
    for row in surfaces:
        v = row.get("api_kind")
        if v is None:
            continue
        if v not in ALLOWED_API_KIND:
            findings.append(
                (
                    "8.api_kind_not_allowed",
                    f"surfaces row nt_api={row.get('nt_api')!r} "
                    f"api_kind={v!r} not in {sorted(ALLOWED_API_KIND)}",
                )
            )

    # Check 9: every pinned NT subscribe_* API appears in the surfaces YAML.
    nt_api_path = find_pinned_nt_api_path(findings, nautilus_revision)
    if nt_api_path is not None:
        pinned_subscribe_apis = set(SUBSCRIBE_FN_RE.findall(read(nt_api_path)))
        yaml_nt_apis = {str(row.get("nt_api", "")).split("(")[0] for row in surfaces}
        missing = sorted(pinned_subscribe_apis - yaml_nt_apis)
        extra_passive = sorted(
            str(row.get("nt_api", ""))
            for row in surfaces
            if row.get("api_kind") == "passive_pubsub"
            and str(row.get("nt_api", "")).split("(")[0] not in pinned_subscribe_apis
        )
        for nt_api in missing:
            findings.append(
                (
                    "9.pinned_subscribe_api_missing",
                    f"pinned NT exposes {nt_api} in {nt_api_path}, "
                    f"but nt-msgbus-surfaces.yaml has no row for it",
                )
            )
        for nt_api in extra_passive:
            findings.append(
                (
                    "9.extra_passive_api_not_in_pinned_nt",
                    f"nt-msgbus-surfaces.yaml marks {nt_api!r} as passive_pubsub, "
                    f"but no matching pinned NT subscribe_* API exists",
                )
            )

    # Check 10: surface storage recommendations match the feasibility table.
    feasibility_by_type = {
        str(row.get("message_type")): str(row.get("recommended_storage"))
        for row in feas_types
        if row.get("message_type") and row.get("recommended_storage")
    }
    storage_equivalents = {
        ("none", "skip"),
        ("skip", "none"),
    }
    for row in surfaces:
        message_type = str(row.get("storage_message_type") or row.get("message_type", ""))
        suggested = row.get("suggested_capture_storage")
        if row.get("bolt_status") == "captured_now" and (
            suggested is None or suggested == "none"
        ):
            findings.append(
                (
                    "10.captured_now_storage_missing",
                    f"captured_now surfaces row nt_api={row.get('nt_api')!r} "
                    f"must have an explicit non-none suggested_capture_storage",
                )
            )
            continue
        if suggested is None or suggested == "none":
            continue
        if message_type not in feasibility_by_type:
            if row.get("bolt_status") != "captured_now":
                continue
            findings.append(
                (
                    "10.surface_storage_missing_feasibility",
                    f"surfaces row nt_api={row.get('nt_api')!r} "
                    f"message_type={message_type!r} has suggested_capture_storage="
                    f"{suggested!r}, but storage-feasibility has no row for this message_type",
                )
            )
            continue
        expected = feasibility_by_type[message_type]
        if suggested != expected and (str(suggested), expected) not in storage_equivalents:
            findings.append(
                (
                    "10.surface_storage_mismatch",
                    f"surfaces row nt_api={row.get('nt_api')!r} "
                    f"message_type={message_type!r} suggested_capture_storage="
                    f"{suggested!r}, but storage-feasibility recommends "
                    f"{expected!r}",
                )
            )

    # Check 11: every source subscribe_* call's function/helper pair has a
    # matching captured_now row. This closes the source -> YAML direction
    # without relying on prose evidence strings.
    source_subscribe_pairs = source_subscribe_calls(src_text, findings)
    captured_pairs: set[tuple[str, str]] = set()
    for row in captured_rows:
        source_subscribe_fn = row.get("source_subscribe_fn")
        helper = row.get("bolt_pattern_helper")
        if source_subscribe_fn and helper:
            captured_pairs.add((str(source_subscribe_fn), str(helper)))
    missing_captured_rows = sorted(source_subscribe_pairs - captured_pairs)
    stale_captured_rows = sorted(captured_pairs - source_subscribe_pairs)
    for fn_name, helper in missing_captured_rows:
        findings.append(
            (
                "11.source_subscribe_not_captured_now",
                f"src/nt_runtime_capture.rs calls {fn_name} with {helper}(), "
                f"but no captured_now surfaces row declares that function/helper pair",
            )
        )
    for fn_name, helper in stale_captured_rows:
        findings.append(
            (
                "11.captured_now_not_in_source",
                f"nt-msgbus-surfaces.yaml declares captured_now "
                f"source_subscribe_fn={fn_name!r} bolt_pattern_helper={helper!r}, "
                f"but src/nt_runtime_capture.rs has no matching subscribe call",
            )
        )

    # Check 12: every captured_now row's stream and storage format agree with
    # bolt-current-capture.yaml.
    current_by_stream = {
        str(row.get("stream")): row
        for row in current_streams
        if isinstance(row, dict) and row.get("stream")
    }
    expected_format_by_storage = {
        "feather": "Feather",
        "jsonl": "JSONL",
        "unwrap_to_orderbookdelta": "Feather",
        "boundary_wrapper": "JSONL",
    }
    surface_streams = {str(row.get("capture_stream")) for row in captured_rows}
    for row in captured_rows:
        stream = row.get("capture_stream")
        storage_format = row.get("storage_format")
        suggested = row.get("suggested_capture_storage")
        if not stream:
            continue
        current = current_by_stream.get(str(stream))
        if current is None:
            findings.append(
                (
                    "12.current_capture_stream_missing",
                    f"captured_now surfaces row nt_api={row.get('nt_api')!r} "
                    f"declares capture_stream={stream!r}, but bolt-current-capture.yaml "
                    f"has no matching stream",
                )
            )
            continue
        current_storage_format = current.get("storage_format")
        if storage_format != current_storage_format:
            findings.append(
                (
                    "12.current_capture_storage_mismatch",
                    f"capture_stream={stream!r} storage_format={storage_format!r}, "
                    f"but bolt-current-capture.yaml says {current_storage_format!r}",
                )
            )
        expected_format = expected_format_by_storage.get(str(suggested))
        if expected_format is not None and storage_format != expected_format:
            findings.append(
                (
                    "12.suggested_storage_format_mismatch",
                    f"capture_stream={stream!r} suggested_capture_storage={suggested!r} "
                    f"requires storage_format={expected_format!r}, got {storage_format!r}",
                )
            )

    # Check 13: all documented NT pins agree with Cargo.toml.
    if nautilus_revision:
        audit_revision = naming_audit_doc.get("nautilus_trader_revision")
        if audit_revision != nautilus_revision:
            findings.append(
                (
                    "13.pin_revision_mismatch",
                    f"nt-owned-name-audit.yaml nautilus_trader_revision={audit_revision!r}, "
                    f"but Cargo.toml pins {nautilus_revision!r}",
                )
            )
        runtime_revisions = re.findall(
            r"current value:\s*`([0-9a-f]{40})`", runtime_contracts_text
        )
        if not runtime_revisions:
            findings.append(
                (
                    "13.runtime_contract_pin_missing",
                    "runtime contracts doc does not expose a `current value: <40-char rev>` pin",
                )
            )
        elif set(runtime_revisions) != {nautilus_revision}:
            findings.append(
                (
                    "13.pin_revision_mismatch",
                    f"runtime contracts current values={sorted(set(runtime_revisions))!r}, "
                    f"but Cargo.toml pins {nautilus_revision!r}",
                )
            )
        if nautilus_revision in surfaces_text or nautilus_revision[:7] in surfaces_text:
            findings.append(
                (
                    "13.surfaces_yaml_pin_literal",
                    "nt-msgbus-surfaces.yaml must not hardcode the NautilusTrader pin; "
                    "the verifier derives it from Cargo.toml",
                )
            )

    # Check 14: bolt-current-capture.yaml must not retain stale streams, and
    # its listed tests must exist.
    stale_current_streams = sorted(set(current_by_stream) - surface_streams)
    for stream in stale_current_streams:
        findings.append(
            (
                "14.current_capture_stale_stream",
                f"bolt-current-capture.yaml declares stream={stream!r}, "
                "but no captured_now surface declares that capture_stream",
            )
        )
    for stream, row in current_by_stream.items():
        for test_name in row.get("test_coverage") or []:
            if not re.search(
                TEST_FN_RE_TEMPLATE.format(re.escape(str(test_name))), test_text
            ):
                findings.append(
                    (
                        "14.current_capture_missing_test",
                        f"bolt-current-capture.yaml stream={stream!r} lists "
                        f"test {test_name!r}, but tests/nt_runtime_capture.rs has no such test fn",
                    )
                )

    return findings


def main() -> int:
    findings = collect_failures()
    if not findings:
        print("OK: all 14 runtime-capture YAML checks passed.")
        return 0

    by_check: dict[str, list[str]] = {}
    for check_id, msg in findings:
        by_check.setdefault(check_id, []).append(msg)

    sys.stderr.write(f"FAIL: {len(findings)} verification finding(s)\n")
    for check_id in sorted(by_check):
        sys.stderr.write(f"\n[{check_id}]\n")
        for msg in by_check[check_id]:
            sys.stderr.write(f"  - {msg}\n")
    return 1


if __name__ == "__main__":
    sys.exit(main())
