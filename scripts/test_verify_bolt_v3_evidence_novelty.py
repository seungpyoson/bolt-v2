#!/usr/bin/env python3
"""Self-tests for the #1354 novelty registry verifier."""

from __future__ import annotations

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
        text.replace("id_start = 32", "id_start = 33", 1),
    )
    for invalid in invalid_documents:
        try:
            _load(invalid)
        except ValueError:
            continue
        raise AssertionError("registry accepted an ambiguous finite domain")


def test_fresh_render_is_byte_exact() -> None:
    registry = verifier.load_registry(verifier.REPO_ROOT / verifier.REGISTRY_PATH)
    actual = (verifier.REPO_ROOT / verifier.GENERATED_PATH).read_text(encoding="utf-8")
    assert verifier.render_registry(registry) == actual


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
