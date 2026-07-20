#!/usr/bin/env python3
"""Self-tests for the #1354 novelty registry verifier."""

from __future__ import annotations

import dataclasses
import pathlib
import shutil
import tempfile

import verify_bolt_v3_evidence_novelty as verifier


def _registry_text() -> str:
    return (verifier.REPO_ROOT / verifier.REGISTRY_PATH).read_text(encoding="utf-8")


def _load(text: str) -> verifier.Registry:
    with tempfile.TemporaryDirectory() as scratch:
        path = pathlib.Path(scratch) / "registry.toml"
        path.write_text(text, encoding="utf-8")
        return verifier.load_registry(path)


def test_repository_projection_is_fresh_and_complete() -> None:
    assert verifier.repository_errors(verifier.REPO_ROOT) == []


def test_closed_family_and_evidence_census_are_complete() -> None:
    registry = verifier.load_registry(verifier.REPO_ROOT / verifier.REGISTRY_PATH)
    assert tuple(family.name for family in registry.families) == (
        "risk",
        "market",
        "system",
    )
    assert {producer.method for producer in registry.producers} == {
        "record_strategy_input_snapshot",
        "record_order_intent",
        "record_admission_decision",
        "record_basket_admission_decision",
        "record_capital_admission_rebuild_audit",
        "record_submit_reservation_metadata",
        "record_submit_reservation_fill",
        "record_entry_skip",
        "record_exit_decision",
        "record_exit_evaluation",
        "record_loss_governor_halt",
        "record_order_reject",
        "record_order_lifecycle",
        "record_requote_throttle",
        "record_settlement",
        "record_terminal_settlement",
        "record_venue_truth_capture_failure",
        "record_venue_truth_divergence",
    }
    assert {reader.name for reader in registry.readers} == {
        "read_latest_entry_decision_evidence_chain",
        "read_submit_reservation_recovery_evidence",
        "read_exit_evaluation_evidence",
        "read_loss_governor_halt_evidence",
        "read_order_reject_evidence",
        "read_settlement_evidence",
        "read_settlement_evidence_records",
        "read_kind_evidence",
        "read_settlement_booking_error_evidence",
        "read_terminal_settlement_evidence",
        "read_terminal_settlement_keys_for_recovery_scope",
        "read_settlement_keys_for_recovery_scope",
        "read_settlement_booking_error_keys_for_recovery_scope",
        "read_settlement_evidence_for_recovery_scope",
        "shadow_pnl_read_admitted_entry_chains",
        "shadow_pnl_read_settlements",
        "shadow_pnl_read_jsonl_lines",
        "v15_migrator_migrate_file_bytes",
        "v15_migrator_plan_migrations",
    }


def test_unknown_registry_and_state_fields_are_rejected() -> None:
    text = _registry_text()
    for invalid in (
        text + "\nunknown = true\n",
        text.replace("id = 144", "id = 144\nunknown = true", 1),
    ):
        try:
            _load(invalid)
        except ValueError:
            continue
        raise AssertionError("closed registry accepted an unknown field")


def test_duplicate_ids_and_allocation_gaps_are_rejected() -> None:
    text = _registry_text()
    invalid_documents = (
        text.replace("id = 145", "id = 144", 1),
        text.replace("start = 8", "start = 9", 1),
    )
    for invalid in invalid_documents:
        try:
            _load(invalid)
        except ValueError:
            continue
        raise AssertionError("registry accepted an ambiguous finite domain")


def test_frozen_allocations_and_census_roots_cannot_shrink() -> None:
    text = _registry_text()
    invalid_documents = (
        text.replace(
            '{ name = "admission_entry", start = 0, end = 8 },\n'
            '  { name = "order_prepare_submit_fill_terminal", start = 8, end = 24 },',
            '{ name = "admission_entry", start = 0, end = 9 },\n'
            '  { name = "order_prepare_submit_fill_terminal", start = 9, end = 24 },',
            1,
        ),
        text.replace('producer_census_roots = ["src"]', 'producer_census_roots = ["src/strategies"]', 1),
        text.replace(
            'reader_census_roots = ["src/bolt_v3_decision_evidence.rs", "src/shadow_pnl.rs", "scripts/migrate_bolt_v3_decision_evidence_to_v15.py"]',
            'reader_census_roots = ["src/bolt_v3_decision_evidence.rs"]',
            1,
        ),
    )
    for invalid in invalid_documents:
        try:
            _load(invalid)
        except ValueError:
            continue
        raise AssertionError("registry allowed its frozen census authority to shrink")


def test_fresh_render_is_byte_exact() -> None:
    registry = verifier.load_registry(verifier.REPO_ROOT / verifier.REGISTRY_PATH)
    actual = (verifier.REPO_ROOT / verifier.GENERATED_PATH).read_text(encoding="utf-8")
    assert verifier.render_registry(registry) == actual


def test_producer_source_census_is_exact_and_non_vacuous() -> None:
    registry = verifier.load_registry(verifier.REPO_ROOT / verifier.REGISTRY_PATH)
    discovered = verifier._producer_call_sites(verifier.REPO_ROOT, registry)
    discovered -= set(registry.producer_census_exclusions)
    registered = {call_site for producer in registry.producers for call_site in producer.call_sites}
    assert discovered == registered
    producer = next(producer for producer in registry.producers if producer.call_sites)
    mutated = dataclasses.replace(producer, call_sites=producer.call_sites[1:])
    mutated_registered = {
        call_site
        for candidate in (*registry.producers, mutated)
        if candidate is not producer
        for call_site in candidate.call_sites
    }
    assert discovered != mutated_registered


def test_comments_literals_and_cfg_test_cannot_fake_producer_calls() -> None:
    registry = verifier.load_registry(verifier.REPO_ROOT / verifier.REGISTRY_PATH)
    registry = dataclasses.replace(
        registry,
        producer_census_roots=("src",),
        producer_census_exclusions=(),
    )
    with tempfile.TemporaryDirectory() as scratch:
        root = pathlib.Path(scratch)
        source = root / "src" / "fixture.rs"
        source.parent.mkdir(parents=True)
        source.write_text(
            """
fn production(writer: &dyn Writer, skip: &Skip) {
    writer.record_entry_skip(skip);
    Writer::record_order_intent(writer, intent);
    // writer.record_order_intent(intent);
    let _phantom = "writer.record_admission_decision(decision)";
}

#[cfg(test)]
mod tests {
    fn phantom(writer: &dyn Writer, decision: &Decision) {
        writer.record_basket_admission_decision(decision);
    }
}
""",
            encoding="utf-8",
        )
        assert verifier._producer_call_sites(root, registry) == {
            "src/fixture.rs::production::record_entry_skip::1",
            "src/fixture.rs::production::record_order_intent::1",
        }


def test_recovery_bearing_producer_cannot_enable_suppression() -> None:
    text = _registry_text().replace(
        'recovery_bearing = true\nruntime_status = "implemented"\nrequired_suppression = "unsuppressed"',
        'recovery_bearing = true\nruntime_status = "implemented"\nrequired_suppression = "finite-monotone-mask"',
        1,
    )
    try:
        _load(text)
    except ValueError as error:
        assert "recovery-bearing" in str(error)
    else:
        raise AssertionError("registry allowed suppression of recovery-bearing evidence")


def test_runtime_status_distinguishes_implemented_and_remaining_work() -> None:
    registry = verifier.load_registry(verifier.REPO_ROOT / verifier.REGISTRY_PATH)
    statuses = {producer.name: producer.runtime_status for producer in registry.producers}
    assert "owner-decision-required" not in statuses.values()
    assert not any(producer.owner_decision_required for producer in registry.producers)
    assert statuses["strategy_input_snapshot_blocked_rv"] == "implemented"
    assert statuses["entry_skip"] == "implemented"
    assert statuses["exit_decision"] == "deferred-to-1385"
    assert statuses["exit_evaluation"] == "deferred-to-1385"
    assert statuses["loss_governor_halt"] == "deferred-to-1385"
    assert statuses["order_reject"] == "deferred-to-1385"
    assert statuses["requote_throttle"] == "implemented"
    assert statuses["basket_admission_decision"] == "owner-approved-retention"
    assert statuses["capital_admission_rebuild_audit"] == "owner-approved-retention"
    assert statuses["order_lifecycle"] == "owner-approved-retention"
    assert "settlement_booking_error_legacy_append" not in statuses
    assert statuses["venue_truth_capture_failure"] == "deferred-to-1385"
    assert statuses["venue_truth_divergence"] == "deferred-to-1385"


def test_legacy_booking_error_is_readable_but_not_writable() -> None:
    registry = verifier.load_registry(verifier.REPO_ROOT / verifier.REGISTRY_PATH)
    assert registry.legacy_read_only_record_kinds == ("settlement_booking_error",)
    assert "record_settlement_booking_error" not in {
        producer.method for producer in registry.producers
    }
    assert "read_settlement_booking_error_evidence" in {
        reader.name for reader in registry.readers
    }
    assert "read_settlement_booking_error_keys_for_recovery_scope" in {
        reader.name for reader in registry.readers
    }


def test_legacy_read_only_compatibility_set_cannot_drift() -> None:
    text = _registry_text()
    for invalid in (
        text.replace(
            'legacy_read_only_record_kinds = ["settlement_booking_error"]',
            "legacy_read_only_record_kinds = []",
            1,
        ),
        text.replace(
            'legacy_read_only_record_kinds = ["settlement_booking_error"]',
            'legacy_read_only_record_kinds = ["settlement_booking_error", "settlement"]',
            1,
        ),
        text.replace(
            'legacy_read_only_record_kinds = ["settlement_booking_error"]',
            'legacy_read_only_record_kinds = "settlement_booking_error"',
            1,
        ),
    ):
        try:
            _load(invalid)
        except ValueError:
            continue
        raise AssertionError("legacy read-only compatibility set drifted")


def test_owner_approved_retention_is_only_for_unsuppressed_no_reader_rows() -> None:
    text = _registry_text()
    invalid = text.replace(
        'classification = "no-named-reader"\nhandler_reachability = ["timer"]',
        'classification = "event-keyed"\nhandler_reachability = ["timer"]',
        1,
    )
    try:
        _load(invalid)
    except ValueError as error:
        assert "owner-approved retention" in str(error)
    else:
        raise AssertionError("owner-approved retention escaped its no-reader disposition")


def test_requote_throttle_has_closed_market_state_domain() -> None:
    registry = verifier.load_registry(verifier.REPO_ROOT / verifier.REGISTRY_PATH)
    states = [state for state in registry.states if state.producer_kind == "requote_throttle"]
    assert len(states) == 17
    assert {state.id for state in states} == set(range(208, 225))
    producer = next(producer for producer in registry.producers if producer.name == "requote_throttle")
    assert producer.runtime_status == "implemented"
    assert producer.required_suppression == "finite-monotone-mask"


def test_pending_monotone_migration_must_name_finite_required_suppression() -> None:
    text = _registry_text().replace(
        'runtime_status = "implemented"\nrequired_suppression = "finite-monotone-mask"',
        'runtime_status = "monotone-migration-required"\nrequired_suppression = "unsuppressed"',
        1,
    )
    try:
        _load(text)
    except ValueError as error:
        assert "monotone migration" in str(error)
    else:
        raise AssertionError("pending monotone migration allowed an unsuppressed target")


def test_1385_deferral_must_name_finite_required_suppression() -> None:
    text = _registry_text().replace(
        'runtime_status = "deferred-to-1385"\nrequired_suppression = "finite-monotone-mask"',
        'runtime_status = "deferred-to-1385"\nrequired_suppression = "unsuppressed"',
        1,
    )
    try:
        _load(text)
    except ValueError as error:
        assert "#1385 deferral" in str(error)
    else:
        raise AssertionError("#1385 deferral allowed an unsuppressed target")


def test_non_evidence_appender_sweep_detects_production_and_ignores_tests() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = pathlib.Path(scratch)
        production = root / "src" / "strategies" / "fixture.rs"
        ignored = root / "src" / "strategies" / "tests" / "fixture.rs"
        live_node = root / "src" / "bolt_v3_live_node.rs"
        live_node_dir = root / "src" / "bolt_v3_live_node"
        production.parent.mkdir(parents=True)
        ignored.parent.mkdir(parents=True)
        live_node_dir.mkdir(parents=True)
        production.write_text('fn tick() { std::fs::write("x", b"x"); }\n', encoding="utf-8")
        ignored.write_text('fn test_only() { std::fs::write("x", b"x"); }\n', encoding="utf-8")
        live_node.write_text("fn live() {}\n", encoding="utf-8")
        assert verifier._non_evidence_per_tick_appenders(root) == {
            "src/strategies/fixture.rs:1"
        }


def test_comments_and_literals_cannot_fake_registered_producer_states() -> None:
    variant = "EntrySkipStrategyCoreNotRegistered"
    canonical_reference = f"EvidenceCanonicalState::{variant}"
    for phantom in (f"// {canonical_reference}\n", f'const PHANTOM: &str = "{canonical_reference}";\n'):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            for relative in (
                verifier.REGISTRY_PATH,
                verifier.GENERATED_PATH,
                verifier.PRODUCER_PATH,
                verifier.ENTRY_DECISION_PATH,
            ):
                destination = root / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(verifier.REPO_ROOT / relative, destination)
            producer_path = root / verifier.PRODUCER_PATH
            producer = producer_path.read_text(encoding="utf-8")
            producer = producer.replace(canonical_reference, "", 1) + phantom
            producer_path.write_text(producer, encoding="utf-8")
            errors = verifier.repository_errors(root)
            assert any(variant in error and "missing=" in error for error in errors), errors


def test_every_runtime_entry_block_reason_requires_a_mapping() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        root = pathlib.Path(scratch)
        for relative in (
            verifier.REGISTRY_PATH,
            verifier.GENERATED_PATH,
            verifier.PRODUCER_PATH,
            verifier.ENTRY_DECISION_PATH,
        ):
            destination = root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(verifier.REPO_ROOT / relative, destination)
        mapping_path = root / verifier.ENTRY_DECISION_PATH
        mapping = mapping_path.read_text(encoding="utf-8")
        mapping = mapping.replace(
            "        ENTRY_BLOCK_REASON_STRATEGY_CORE_NOT_REGISTERED => {",
            "        _REMOVED_ENTRY_BLOCK_REASON => {",
            1,
        )
        mapping_path.write_text(mapping, encoding="utf-8")
        errors = verifier.repository_errors(root)
        assert any(
            "ENTRY_BLOCK_REASON_STRATEGY_CORE_NOT_REGISTERED" in error and "missing=" in error
            for error in errors
        ), errors


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    tests = [
        value
        for name, value in sorted(globals().items())
        if name.startswith("test_") and callable(value)
    ]
    for test in tests:
        test()
    print(f"OK: {len(tests)} evidence novelty verifier self-tests passed.")
