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
  3. Every captured_now row in nt-msgbus-surfaces.yaml has a reason/evidence
     string mentioning a Bolt capture function/pattern, and that referenced
     pattern helper appears in src/nt_runtime_capture.rs.
  4. Every safe_missing_passive_stream row contains both publisher and
     subscriber evidence in its reason / storage_evidence / topic_evidence /
     nt_path text.
  5. The events.risk / TradingStateChanged surface is bolt_status=captured_now
     AND src/nt_runtime_capture.rs writes to risk/trading_state_changed.jsonl.
  6. OrderBookDeltas recommended_storage in storage-feasibility.yaml is NOT
     feather (the container has no Arrow impl; bolt unwraps per-delta).
  7. recommended_storage values are only:
        feather, jsonl, boundary_wrapper, wrapper_required, skip, none.
  8. api_kind values are only:
        passive_pubsub, publish_helper, endpoint_or_command,
        cleanup_helper, topic_builder.
  9. Every pinned NT `pub fn subscribe_*` msgbus API appears in
     nt-msgbus-surfaces.yaml.

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
SRC_PATH = REPO_ROOT / "src" / "nt_runtime_capture.rs"
TEST_PATH = REPO_ROOT / "tests" / "nt_runtime_capture.rs"
NT_API_PATH_PATTERN = (
    ".cargo/git/checkouts/nautilus_trader-*/48d1c12/"
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
SUBSCRIBE_FN_RE = re.compile(r"^pub fn (subscribe_[a-z0-9_]+)\b", re.MULTILINE)
RISK_JSONL_PATH_FRAGMENT = 'join("risk")'
RISK_JSONL_FILENAME = "trading_state_changed.jsonl"


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def load_yaml(path: Path):
    return yaml.safe_load(read(path))


def find_pinned_nt_api_path(findings: list[tuple[str, str]]) -> Path | None:
    matches = sorted(Path.home().glob(NT_API_PATH_PATTERN))
    if len(matches) != 1:
        findings.append(
            (
                "9.pinned_nt_api_missing",
                f"expected exactly one pinned NT msgbus api.rs at "
                f"~/{NT_API_PATH_PATTERN}, found {len(matches)}",
            )
        )
        return None
    return matches[0]


def collect_failures() -> list[tuple[str, str]]:
    """Return [(check_id, message)] for every detected violation."""
    findings: list[tuple[str, str]] = []

    surfaces_doc = load_yaml(SURFACES_PATH) or {}
    feas_doc = load_yaml(FEAS_PATH) or {}
    src_text = read(SRC_PATH)
    test_text = read(TEST_PATH)
    surfaces_text = read(SURFACES_PATH)
    feas_text = read(FEAS_PATH)

    surfaces = (
        surfaces_doc.get("surfaces", []) if isinstance(surfaces_doc, dict) else []
    )
    feas_types = feas_doc.get("types", []) if isinstance(feas_doc, dict) else []

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

    # Check 3: every captured_now surfaces row mentions a *_pattern() helper
    # (or wire_nt_runtime_capture) that is defined in src/nt_runtime_capture.rs.
    captured_rows = [r for r in surfaces if r.get("bolt_status") == "captured_now"]
    for row in captured_rows:
        text = " ".join(
            str(row.get(k, ""))
            for k in (
                "reason",
                "topic_evidence",
                "storage_evidence",
                "publisher_evidence",
                "subscriber_evidence",
            )
        )
        helpers = PATTERN_HELPER_RE.findall(text)
        if not helpers and "wire_nt_runtime_capture" not in text:
            findings.append(
                (
                    "3.captured_now_no_bolt_ref",
                    f"surfaces row nt_api={row.get('nt_api')!r} "
                    f"(captured_now) reason/evidence does not name any "
                    f"*_pattern() helper or wire_nt_runtime_capture",
                )
            )
            continue
        for helper in helpers:
            if f"fn {helper}()" not in src_text:
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
        if r.get("message_type") == "TradingStateChanged"
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
    nt_api_path = find_pinned_nt_api_path(findings)
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

    return findings


def main() -> int:
    findings = collect_failures()
    if not findings:
        print("OK: all 9 runtime-capture YAML checks passed.")
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
