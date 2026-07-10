#!/usr/bin/env python3
"""Generate the pinned Wave-2 disposition receipt from the live registry."""

from __future__ import annotations

import importlib.util
import tomllib
from collections import Counter
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
VERIFIER_PATH = Path(__file__).with_name("verify_bolt_v3_runtime_literal_crosswalk.py")
SPEC = importlib.util.spec_from_file_location("crosswalk_verifier", VERIFIER_PATH)
assert SPEC is not None and SPEC.loader is not None
VERIFIER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFIER)


PERSISTED_PATH_PARTS = (
    "decision_evidence",
    "operator_artifacts",
    "basket_store",
    "kill_switch_store",
    "execution_state",
    "raw_types",
    "shadow_pnl",
)
PERSISTED_TERMS = (
    "record_kind",
    "envelope_kind",
    "gate_id",
    "persisted",
    "artifact",
    "audit_schema",
    "recovery_evidence",
    "operator_artifact_cli_output_schema",
    "sidecar_schema",
    "evidence_source",
    "evidence_reason",
    "evidence_field",
    "evidence_label",
    "reservation_metadata",
    "order_lifecycle",
    "venue_truth",
)
DENY_TERMS = (
    "forbidden_env",
    "forbidden_environment",
    "secret_redaction_guard",
    "credential_env_var",
)
IDENTITY_TERMS = (
    "provider_key",
    "provider_identity",
    "venue_dispatch",
    "venue_identity",
    "venue_id",
    "venue_name",
    "client_provider_key",
    "reference_provider_key",
    "provider_adapter_registry_key",
    "gated_source_registry_key",
    "market_family_dispatch_key",
    "market_family_key",
    "provider_venue_key",
    "provider_kind_key",
)
VALUE_TERMS = (
    "pricing_formula",
    "coefficient",
    "multiplier",
    "basis_point",
    "bps",
    "decimal_scale",
    "decimals",
    "timeout",
    "cadence",
    "duration",
    "protocol",
    "decoder",
    "abi",
    "hmac",
    "domain_separator",
    "signature",
    "selector",
    "magic",
    "byte_width",
    "hex_len",
    "arity",
    "algorithm",
    "formula",
    "threshold",
    "ratio",
    "percentile",
    "cdf",
    "chain_id",
    "http_status",
)
DIAGNOSTIC_TERMS = (
    "diagnostic",
    "error_template",
    "error_message",
    "log_template",
    "log_module",
    "validation_message",
    "cli_help",
    "test_fixture",
    "test_only",
    "display_text",
    "reason_classifier_needle",
)
CODE_OWNED_TERMS = (
    "schema_field",
    "field_name",
    "field_label",
    "evidence_label",
    "config_field",
    "toml_field",
    "output_field",
    "query_param_name",
    "root_key",
    "source_schema_field",
    "validation_field",
    "enum_label",
    "status_label",
    "metric_label",
    "schema_label",
    "schema_literal",
    "schema_token",
    "schema_value",
    "config_literal",
    "config_token",
    "projection_field",
    "provenance_field",
    "outcome_key",
    "reason_key",
    "side_key",
    "health_state_label",
    "surface_label",
    "secret_field",
    "custom_data_field",
    "summary_field",
    "debug_field",
)
SOURCE_TERMS = (
    "source_marker",
    "upstream",
    "nt_",
    "nautilus",
    "content_type",
    "url_scheme",
    "http_header",
    "external",
    "contract",
    "wire",
)


def haystack(row: dict[str, Any]) -> str:
    return " ".join(
        str(row.get(field, "")).lower()
        for field in ("path", "kind", "literal", "context", "classification", "reason")
    )


def has_any(text: str, terms: tuple[str, ...]) -> bool:
    return any(term in text for term in terms)


def classify(row: dict[str, Any], row_id: str) -> tuple[str, str, str | None, str | None]:
    text = haystack(row)
    classification = str(row.get("classification", "")).lower()
    path = str(row["path"])

    if has_any(text, DENY_TERMS):
        return "DENY_LIST", "FORBIDDEN_ENVIRONMENT_NAME", "deny_list_const", "FORBIDDEN_CREDENTIAL_ENV_VARS"

    if has_any(classification, IDENTITY_TERMS) or (
        ("dispatch" in classification or "identity" in classification)
        and ("venue" in classification or "provider" in classification)
    ):
        owner = "ReferenceProviderKey" if "reference" in text else "ProviderKey"
        return "TYPED_OWNER", "IDENTITY_KEY", "owner_type", owner

    if "outcome_group" in path and (
        "evidence_schema_label" in classification
        or "fingerprint" in classification
        or "canonical" in classification
    ):
        return "VALUE_TEST", "HASH_COMPATIBILITY", "test_symbol", "canonical_fingerprint_known_answer_is_stable"

    if path == "src/main.rs" and (
        "output" in classification or "ops_" in classification or "operator" in classification
    ):
        return (
            "COMPAT_FIXTURE",
            "WIRE_COMPATIBILITY",
            "fixture_path",
            "tests/fixtures/bolt_v3/compatibility/operator_cli_shapes.json",
        )

    if has_any(path, PERSISTED_PATH_PARTS) and has_any(text, PERSISTED_TERMS):
        return (
            "COMPAT_FIXTURE",
            "PERSISTED_COMPATIBILITY",
            "fixture_path",
            "tests/fixtures/bolt_v3/compatibility/persisted_shapes.json",
        )

    if has_any(classification, PERSISTED_TERMS):
        return (
            "COMPAT_FIXTURE",
            "PERSISTED_COMPATIBILITY",
            "fixture_path",
            "tests/fixtures/bolt_v3/compatibility/persisted_shapes.json",
        )

    if "collateral_accounting_source.rs" in path and (
        "json_rpc" in classification or "rpc_" in classification or "jsonrpc" in text
    ):
        return (
            "COMPAT_FIXTURE",
            "WIRE_COMPATIBILITY",
            "fixture_path",
            "tests/fixtures/bolt_v3/compatibility/polymarket_eth_call.json",
        )

    if path == "src/lake_batch.rs" and ("type_name" in text or "parquet" in text):
        return (
            "COMPAT_FIXTURE",
            "WIRE_COMPATIBILITY",
            "fixture_path",
            "tests/fixtures/bolt_v3/compatibility/nt_catalog_type_names.json",
        )

    if row.get("kind") == "number" or has_any(classification, VALUE_TERMS):
        return "VALUE_TEST", "RUNTIME_VALUE_INVARIANT", "test_symbol", f"runtime_literal_value_{row_id[3:].replace('-', '_')}"

    if "fingerprint" in classification or "canonical" in classification or "hash_" in classification:
        return "VALUE_TEST", "HASH_COMPATIBILITY", "test_symbol", "canonical_fingerprint_known_answer_is_stable"

    if has_any(classification, DIAGNOSTIC_TERMS) or (
        any(term in classification for term in ("block_reason", "reject_reason", "reason_code"))
        and "evidence" not in classification
    ):
        rationale = "TEST_ONLY" if "test" in classification else "DIAGNOSTIC_ONLY"
        return "DELETE_DIAGNOSTIC", rationale, None, None

    if has_any(classification, CODE_OWNED_TERMS):
        return "DELETE_CODE_OWNED", "CODE_OWNED_IDENTIFIER", None, None

    if has_any(classification, SOURCE_TERMS):
        return "SOURCE_CONTRACT", "UPSTREAM_CONTRACT", "source_contract", path

    return "SOURCE_CONTRACT", "UNCERTAIN_KEEP", "source_contract", path


def toml_string(value: str) -> str:
    if "'''" not in value:
        return "'''" + value + "'''"
    escaped = value.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n")
    return f'"{escaped}"'


def render() -> tuple[str, Counter[str], int]:
    registry_path = ROOT / VERIFIER.REGISTRY_REL
    registry = tomllib.loads(registry_path.read_text(encoding="utf-8"))
    rows = registry["allowed"]
    ids = VERIFIER.registry_row_ids(rows)
    counts: Counter[str] = Counter()
    uncertain = 0
    blocks = [
        "# Generated from the pinned runtime-literal registry by",
        "# scripts/generate_bolt_v3_runtime_literal_crosswalk.py.",
        "schema_version = 1",
        f'registry_path = "{VERIFIER.REGISTRY_REL.as_posix()}"',
        f'registry_sha256 = "{VERIFIER.PINNED_REGISTRY_SHA256}"',
        f"registry_row_count = {len(rows)}",
        f'stable_id_algorithm = "{VERIFIER.STABLE_ID_ALGORITHM}"',
    ]
    for row, row_id in zip(rows, ids, strict=True):
        disposition, rationale, field, reference = classify(row, row_id)
        counts[disposition] += 1
        uncertain += rationale == "UNCERTAIN_KEEP"
        lines = [
            "[[entry]]",
            f'id = "{row_id}"',
            f'disposition = "{disposition}"',
            'phase = "planned"',
            f'rationale = "{rationale}"',
        ]
        if field is not None and reference is not None:
            lines.append(f"{field} = {toml_string(reference)}")
        blocks.append("\n".join(lines))
    return "\n\n".join(blocks) + "\n", counts, uncertain


def main() -> int:
    text, counts, uncertain = render()
    output = ROOT / VERIFIER.CROSSWALK_REL
    output.write_text(text, encoding="utf-8")
    print(f"wrote {output.relative_to(ROOT)}: {dict(sorted(counts.items()))}; UNCERTAIN_KEEP={uncertain}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
