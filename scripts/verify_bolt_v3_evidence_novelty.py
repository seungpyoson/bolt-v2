#!/usr/bin/env python3
"""Verify and deterministically generate the #1354 evidence novelty registry."""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
import tomllib
from dataclasses import dataclass


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
REGISTRY_PATH = pathlib.Path("config/evidence-novelty.toml")
GENERATED_PATH = pathlib.Path("src/bolt_v3_evidence_novelty_generated.rs")
NOVELTY_PATH = pathlib.Path("src/bolt_v3_evidence_novelty.rs")
PRODUCER_PATH = pathlib.Path("src/strategies/binary_oracle_edge_taker/mod.rs")
ENTRY_DECISION_PATH = pathlib.Path(
    "src/strategies/binary_oracle_edge_taker/entry_decision.rs"
)

FROZEN_MARKET_ALLOCATIONS = (
    ("discovery_identity", 0, 32),
    ("lifecycle_rollover", 32, 80),
    ("subscription_book", 80, 144),
    ("strategy_input_pricing_blocker", 144, 208),
    ("dependency_health", 208, 240),
    ("terminal_closed_window_skip", 240, 256),
)
FROZEN_MARKET_FAMILY_CAPACITY = FROZEN_MARKET_ALLOCATIONS[-1][2]
FROZEN_MARKET_STATES = (
    (144, "strategy_input_snapshot.blocked_rv.accepted.watermark_absent"),
    (145, "strategy_input_snapshot.blocked_rv.accepted.watermark_present"),
    (146, "strategy_input_snapshot.blocked_rv.missing_snapshot.watermark_absent"),
    (147, "strategy_input_snapshot.blocked_rv.missing_snapshot.watermark_present"),
    (
        148,
        "strategy_input_snapshot.blocked_rv.missing_evaluation_event_time.watermark_absent",
    ),
    (
        149,
        "strategy_input_snapshot.blocked_rv.missing_evaluation_event_time.watermark_present",
    ),
    (150, "strategy_input_snapshot.blocked_rv.rejected_future_dated.watermark_absent"),
    (151, "strategy_input_snapshot.blocked_rv.rejected_future_dated.watermark_present"),
    (152, "strategy_input_snapshot.blocked_rv.rejected_stale.watermark_absent"),
    (153, "strategy_input_snapshot.blocked_rv.rejected_stale.watermark_present"),
    (154, "strategy_input_snapshot.blocked_rv.rejected_not_ready.watermark_absent"),
    (155, "strategy_input_snapshot.blocked_rv.rejected_not_ready.watermark_present"),
    (156, "entry_skip.strategy_core_not_registered"),
    (157, "entry_skip.entry_gate_blocked"),
    (158, "entry_skip.entry_pricing_blocked"),
    (159, "entry_skip.no_side_selected"),
    (160, "entry_skip.sized_notional_not_positive"),
    (161, "entry_skip.instrument_id_missing"),
    (162, "entry_skip.instrument_missing_from_cache"),
    (163, "entry_skip.entry_price_missing"),
    (164, "entry_skip.quantity_rounding_failed"),
    (165, "entry_skip.limit_notional_exceeds_sized_notional"),
    (166, "entry_skip.entry_quote_notional_below_venue_minimum"),
    (167, "entry_skip.entry_quote_notional_minimum_unmodeled"),
    (168, "entry_skip.quantity_not_positive"),
    (169, "entry_skip.position_contract_invalid"),
    (170, "entry_skip.entry_position_contract_unsupported"),
    (171, "entry_skip.historical_entry_fee_unavailable"),
    (172, "entry_skip.one_position_invariant_violation"),
)
OWNER_BY_PRODUCER = {
    "entry_skip": "EntrySkip",
    "strategy_input_snapshot": "BlockedStrategyInputSnapshot",
}


@dataclass(frozen=True)
class AllocationRow:
    name: str
    id_start: int
    id_end_exclusive: int


@dataclass(frozen=True)
class StateRow:
    rust_variant: str
    producer_kind: str
    semantic_state: str
    allocation: str
    id: int

    @property
    def owner(self) -> str:
        return OWNER_BY_PRODUCER[self.producer_kind]


@dataclass(frozen=True)
class Registry:
    family_name: str
    family_capacity: int
    allocations: tuple[AllocationRow, ...]
    states: tuple[StateRow, ...]


def load_registry(path: pathlib.Path) -> Registry:
    document = tomllib.loads(path.read_text(encoding="utf-8"))
    if set(document) != {"schema_version", "family", "allocation", "state"}:
        raise ValueError(
            "registry must contain exactly schema_version, family, allocation, and state"
        )
    if document["schema_version"] != 1:
        raise ValueError("registry schema_version must be 1")
    family = document["family"]
    if not isinstance(family, dict) or set(family) != {"name", "capacity"}:
        raise ValueError("family must contain exactly name and capacity")
    family_name = family["name"]
    family_capacity = family["capacity"]
    if not isinstance(family_name, str) or not re.fullmatch(r"[a-z][a-z0-9_]*", family_name):
        raise ValueError("family.name must be snake_case")
    if (
        type(family_capacity) is not int
        or family_capacity != FROZEN_MARKET_FAMILY_CAPACITY
    ):
        raise ValueError(
            "family.capacity must match frozen market-family capacity "
            f"{FROZEN_MARKET_FAMILY_CAPACITY}"
        )

    raw_allocations = document["allocation"]
    if not isinstance(raw_allocations, list) or not raw_allocations:
        raise ValueError("registry must contain at least one [[allocation]] row")
    allocation_keys = {"name", "id_start", "id_end_exclusive"}
    allocations: list[AllocationRow] = []
    for index, raw in enumerate(raw_allocations):
        if not isinstance(raw, dict) or set(raw) != allocation_keys:
            raise ValueError(
                f"allocation[{index}] must contain exactly {sorted(allocation_keys)}"
            )
        row = AllocationRow(**raw)
        if not re.fullmatch(r"[a-z][a-z0-9_]*", row.name):
            raise ValueError(f"allocation[{index}].name must be snake_case")
        if (
            type(row.id_start) is not int
            or type(row.id_end_exclusive) is not int
            or row.id_start < 0
            or row.id_end_exclusive <= row.id_start
            or row.id_end_exclusive > family_capacity
        ):
            raise ValueError(f"allocation[{index}] has an invalid id range")
        allocations.append(row)
    allocation_names = [row.name for row in allocations]
    if len(set(allocation_names)) != len(allocation_names):
        raise ValueError("allocation names must be unique")
    actual_allocations = tuple(
        (row.name, row.id_start, row.id_end_exclusive) for row in allocations
    )
    if actual_allocations != FROZEN_MARKET_ALLOCATIONS:
        raise ValueError("market allocations must match the frozen family ranges")

    raw_states = document["state"]
    if not isinstance(raw_states, list) or not raw_states:
        raise ValueError("registry must contain at least one [[state]] row")
    states: list[StateRow] = []
    expected_keys = {
        "rust_variant",
        "producer_kind",
        "semantic_state",
        "allocation",
        "id",
    }
    for index, raw in enumerate(raw_states):
        if not isinstance(raw, dict) or set(raw) != expected_keys:
            raise ValueError(f"state[{index}] must contain exactly {sorted(expected_keys)}")
        row = StateRow(**raw)
        if not re.fullmatch(r"[A-Z][A-Za-z0-9]*", row.rust_variant):
            raise ValueError(f"state[{index}].rust_variant must be UpperCamelCase")
        if not re.fullmatch(r"[a-z][a-z0-9_]*", row.producer_kind):
            raise ValueError(f"state[{index}].producer_kind must be snake_case")
        if not re.fullmatch(r"[a-z][a-z0-9_.]*", row.semantic_state):
            raise ValueError(f"state[{index}].semantic_state must be dotted snake_case")
        if row.producer_kind not in OWNER_BY_PRODUCER:
            raise ValueError(f"state[{index}].producer_kind is not a registered owner")
        if row.allocation not in allocation_names:
            raise ValueError(f"state[{index}] names an unknown allocation")
        allocation = allocations[allocation_names.index(row.allocation)]
        if type(row.id) is not int or not allocation.id_start <= row.id < allocation.id_end_exclusive:
            raise ValueError(f"state[{index}].id is outside allocation {row.allocation}")
        states.append(row)

    variants = [row.rust_variant for row in states]
    mappings = [(row.producer_kind, row.semantic_state) for row in states]
    if len(set(variants)) != len(variants):
        raise ValueError("registry rust_variant values must be unique")
    if len(set(mappings)) != len(mappings):
        raise ValueError("registry producer/state mappings must be unique")
    ids = [row.id for row in states]
    if len(set(ids)) != len(ids):
        raise ValueError("registry state ids must be unique")
    ordered = sorted(states, key=lambda row: row.id)
    actual_states = tuple((row.id, row.semantic_state) for row in ordered)
    if actual_states != FROZEN_MARKET_STATES:
        raise ValueError("states must match frozen id-to-semantic mappings")
    return Registry(family_name, family_capacity, tuple(allocations), tuple(ordered))


def render_registry(registry: Registry) -> str:
    lines = [
        "// @generated by scripts/verify_bolt_v3_evidence_novelty.py from",
        "// config/evidence-novelty.toml. Do not edit.",
        "",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]",
        "pub enum EvidenceStateOwner {",
    ]
    owners = tuple(dict.fromkeys(row.owner for row in registry.states))
    lines.extend(f"    {owner}," for owner in owners)
    lines.extend(
        [
            "}",
            "",
            "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]",
            "#[repr(u16)]",
            "pub enum EvidenceCanonicalState {",
        ]
    )
    lines.extend(f"    {row.rust_variant} = {row.id}," for row in registry.states)
    lines.extend(
        [
            "}",
            "",
            "#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
            "pub struct EvidenceStateRegistration {",
            "    pub state: EvidenceCanonicalState,",
            "    pub owner: EvidenceStateOwner,",
            "    pub family: &'static str,",
            "    pub producer_kind: &'static str,",
            "    pub semantic_state: &'static str,",
            "    pub id: usize,",
            "}",
            "",
            f"pub const EVIDENCE_NOVELTY_FAMILY_CAPACITY: usize = {registry.family_capacity};",
            f"pub const EVIDENCE_NOVELTY_WORD_COUNT: usize = {registry.family_capacity}.div_ceil(64);",
            "const _: () = assert!(",
            "    EVIDENCE_NOVELTY_WORD_COUNT == EVIDENCE_NOVELTY_FAMILY_CAPACITY.div_ceil(64),",
            '    "EVIDENCE_NOVELTY_WORD_COUNT must cover EVIDENCE_NOVELTY_FAMILY_CAPACITY"',
            ");",
            "",
            "pub const EVIDENCE_STATE_REGISTRATIONS: &[EvidenceStateRegistration] = &[",
        ]
    )
    for row in registry.states:
        lines.extend(
            [
                "    EvidenceStateRegistration {",
                f"        state: EvidenceCanonicalState::{row.rust_variant},",
                f"        owner: EvidenceStateOwner::{row.owner},",
                f"        family: {json.dumps(registry.family_name)},",
                f"        producer_kind: {json.dumps(row.producer_kind)},",
                f"        semantic_state: {json.dumps(row.semantic_state)},",
                f"        id: {row.id},",
                "    },",
            ]
        )
    lines.extend(
        [
            "];",
            "",
            "pub const fn canonical_state_registration(",
            "    state: EvidenceCanonicalState,",
            ") -> &'static EvidenceStateRegistration {",
            "    match state {",
        ]
    )
    for index, row in enumerate(registry.states):
        lines.append(
            f"        EvidenceCanonicalState::{row.rust_variant} => &EVIDENCE_STATE_REGISTRATIONS[{index}],"
        )
    lines.extend(
        [
            "    }",
            "}",
            "",
            "pub const fn evidence_state_registration_by_id(",
            "    id: usize,",
            ") -> Option<&'static EvidenceStateRegistration> {",
            "    match id {",
        ]
    )
    for index, row in enumerate(registry.states):
        lines.append(f"        {row.id} => Some(&EVIDENCE_STATE_REGISTRATIONS[{index}]),")
    lines.extend(["        _ => None,", "    }", "}", ""])
    return "\n".join(lines)


def verification_findings(root: pathlib.Path) -> list[str]:
    findings: list[str] = []
    try:
        registry = load_registry(root / REGISTRY_PATH)
    except (OSError, ValueError, tomllib.TOMLDecodeError) as error:
        return [f"{REGISTRY_PATH}: {error}"]
    expected = render_registry(registry)
    try:
        actual = (root / GENERATED_PATH).read_text(encoding="utf-8")
    except OSError as error:
        findings.append(f"{GENERATED_PATH}: {error}")
    else:
        if actual != expected:
            findings.append(f"{GENERATED_PATH}: generated bytes do not match {REGISTRY_PATH}")

    producer_text = (root / PRODUCER_PATH).read_text(encoding="utf-8")
    referenced = set(re.findall(r"EvidenceStateOwner::([A-Z][A-Za-z0-9]*)", producer_text))
    registered = {row.owner for row in registry.states}
    if referenced != registered:
        findings.append(
            f"{PRODUCER_PATH}: producer owner references {sorted(referenced)} "
            f"must equal registered owners {sorted(registered)}"
        )
    if ".record_strategy_input_snapshot(&strategy_input_snapshot)" not in producer_text:
        findings.append(f"{PRODUCER_PATH}: submit-linked strategy snapshot path must remain direct")
    entry_match = re.search(
        r"fn record_entry_skip_once\(.*?\n    \}\n\n    fn ", producer_text, re.S
    )
    if entry_match is None:
        findings.append(f"{PRODUCER_PATH}: record_entry_skip_once function missing")
    else:
        body = entry_match.group(0)
        try:
            mapping = body.index("entry_skip_canonical_state(reason_category)")
            claim = body.index("entry_skip_novelty.claim_once")
            fields = body.index("entry_evaluation_log_fields_at")
            payload = body.index("BoltV3EntrySkipEvidence::from_entry_skip")
            append = body.index(".record_entry_skip(&evidence)")
        except ValueError:
            findings.append(f"{PRODUCER_PATH}: entry-skip novelty/payload/append seam incomplete")
        else:
            if not mapping < claim < fields < payload < append:
                findings.append(
                    f"{PRODUCER_PATH}: canonical entry-skip mapping and duplicate claim must precede fields, payload, and append"
                )
    blocked_match = re.search(
        r"fn record_blocked_entry_strategy_input_snapshot_once\(.*?\n    \}\n\n    fn ",
        producer_text,
        re.S,
    )
    if blocked_match is None:
        findings.append(f"{PRODUCER_PATH}: blocked strategy-input producer function missing")
    else:
        body = blocked_match.group(0)
        try:
            mapping = body.index("blocked_strategy_input_canonical_state")
            duplicate = body.index("blocked_strategy_input_novelty\n            .has_claimed")
            claim = body.index("blocked_strategy_input_novelty", duplicate + 1)
            payload = body.index("blocked_entry_strategy_input_evidence_snapshot_at")
            append = body.index(".record_strategy_input_snapshot(&snapshot)")
        except ValueError:
            findings.append(f"{PRODUCER_PATH}: blocked snapshot novelty/payload/append seam incomplete")
        else:
            if not mapping < duplicate < payload < claim < append:
                findings.append(
                    f"{PRODUCER_PATH}: blocked-snapshot duplicate check must precede payload build, claim, and append"
                )

    entry_decision_text = (root / ENTRY_DECISION_PATH).read_text(encoding="utf-8")
    obsolete_state_types = (
        "EntrySkipSemanticState",
        "BlockedStrategyInputSemanticState",
        "BlockedStrategyInputSourceStateKey",
    )
    present_state_types = sorted(
        name
        for name in obsolete_state_types
        if name in producer_text or name in entry_decision_text
    )
    if present_state_types:
        findings.append(
            f"{ENTRY_DECISION_PATH}: volatile diagnostic novelty types remain {present_state_types}"
        )

    novelty_text = (root / NOVELTY_PATH).read_text(encoding="utf-8")
    match = re.search(r"pub struct EvidenceEpisodeParts \{(?P<body>.*?)\n\}", novelty_text, re.S)
    if match is None:
        findings.append(f"{NOVELTY_PATH}: EvidenceEpisodeParts declaration missing")
    else:
        forbidden = (
            "price",
            "timestamp",
            "time_",
            "age",
            "counter",
            "flag",
            "diagnostic",
            "slug",
            "retry",
            "schema",
            "config",
            "deployment",
        )
        body = match.group("body").lower()
        present = sorted(term for term in forbidden if term in body)
        if present:
            findings.append(f"{NOVELTY_PATH}: episode parts contain forbidden terms {present}")
    return findings


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=pathlib.Path, default=REPO_ROOT)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    root = args.root.resolve()
    if args.write:
        registry = load_registry(root / REGISTRY_PATH)
        (root / GENERATED_PATH).write_text(render_registry(registry), encoding="utf-8")
        return 0
    findings = verification_findings(root)
    if findings:
        for finding in findings:
            print(f"ERROR: {finding}", file=sys.stderr)
        return 1
    print("OK: evidence novelty registry and producer mappings are closed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
