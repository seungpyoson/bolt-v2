#!/usr/bin/env python3
"""Self-tests for the CI integration-test manifest parser."""

from __future__ import annotations

import pathlib
import sys
import tempfile
import textwrap

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from ci_test_manifest import build_test_manifest


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]


def write_manifest_fixture(root: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path]:
    tests_root = root / "tests"
    tests_root.mkdir()
    manifest_path = root / "Cargo.toml"
    manifest_path.write_text(
        textwrap.dedent(
            """\
            [package]
            name = "ci-test-manifest-fixture"
            version = "0.1.0"
            edition = "2021"
            autotests = false

            [[test]]
            name = "iv"
            path = "tests/iv.rs"

            [[test]]
            name = "foo"
            path = "tests/foo.rs"
            """
        ),
        encoding="utf-8",
    )
    (tests_root / "iv.rs").write_text(
        textwrap.dedent(
            """\
            #[path = "bolt_v3_iv_source_fence.rs"]
            mod bolt_v3_iv_source_fence;
            mod other_iv_member;
            """
        ),
        encoding="utf-8",
    )
    (tests_root / "bolt_v3_iv_source_fence.rs").write_text("", encoding="utf-8")
    (tests_root / "other_iv_member.rs").write_text("", encoding="utf-8")
    (tests_root / "foo.rs").write_text(
        textwrap.dedent(
            """\
            #[test]
            fn standalone_fixture_smoke() {}
            """
        ),
        encoding="utf-8",
    )
    return manifest_path, tests_root


def assert_fixture_manifest_maps_members_to_harnesses() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        manifest_path, tests_root = write_manifest_fixture(pathlib.Path(tmp))
        manifest = build_test_manifest(manifest_path, tests_root)
    expected_member_to_harness = {
        "iv": "iv",
        "bolt_v3_iv_source_fence": "iv",
        "other_iv_member": "iv",
        "foo": "foo",
    }
    if manifest.member_to_harness != expected_member_to_harness:
        raise AssertionError(manifest.member_to_harness)
    expected_harness_to_members = {
        "iv": ("iv", "bolt_v3_iv_source_fence", "other_iv_member"),
        "foo": ("foo",),
    }
    if manifest.harness_to_members != expected_harness_to_members:
        raise AssertionError(manifest.harness_to_members)


def assert_live_top_level_tests_resolve_to_real_harnesses() -> None:
    manifest = build_test_manifest(REPO_ROOT / "Cargo.toml", REPO_ROOT / "tests")
    actual_targets = set(manifest.harness_to_members)
    root_stems = sorted(path.stem for path in (REPO_ROOT / "tests").glob("*.rs"))
    unresolved = [stem for stem in root_stems if stem not in manifest.member_to_harness]
    phantom_targets = [
        (stem, manifest.member_to_harness[stem])
        for stem in root_stems
        if stem in manifest.member_to_harness and manifest.member_to_harness[stem] not in actual_targets
    ]
    if unresolved:
        raise AssertionError(f"unresolved root tests: {unresolved}")
    if phantom_targets:
        raise AssertionError(f"phantom harness targets: {phantom_targets}")
    print(f"OK: live CI test manifest maps {len(root_stems)} root tests to real harness targets.")


def main() -> int:
    assert_fixture_manifest_maps_members_to_harnesses()
    assert_live_top_level_tests_resolve_to_real_harnesses()
    print("OK: CI test manifest self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
