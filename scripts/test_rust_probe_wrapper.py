#!/usr/bin/env python3
"""Self-tests for the managed Rust Probe wrapper and workflow contract."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import types


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "rust_verification.py"
WORKFLOW = REPO_ROOT / ".github" / "workflows" / "rust-probe.yml"
CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"
POLICY = REPO_ROOT / "ci" / "rust-verification.toml"

HEAD = "a" * 40
BRANCH = "codex/rust-probe-wrapper"
LOCAL_BRANCH = "codex/local-rust-probe-wrapper"
UPSTREAM_BRANCH = "codex/upstream-rust-probe-wrapper"


def load_owner_module() -> object:
    spec = importlib.util.spec_from_file_location("rust_verification_rust_probe_under_test", SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError("unable to load rust_verification.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def valid_remote_probe() -> dict:
    return {
        "workflow_name": "Rust Probe",
        "workflow_path": ".github/workflows/rust-probe.yml",
        "poll_interval_seconds": 1,
        "appearance_timeout_seconds": 30,
        "overall_timeout_seconds": 3900,
        "active_run_limit": 4,
        "workflow_runs_per_page": 20,
        "guard_timeout_minutes": 1,
        "allowed_runner_tiers": ["heavy", "light"],
        "mode_runner_tiers": {
            "check-lib": "heavy",
            "check-test-target": "heavy",
            "nextest-no-run-test-target": "heavy",
            "nextest-lib-name": "heavy",
            "nextest-test-target": "heavy",
            "nextest-test-target-name": "heavy",
        },
        "workflow_timeouts": {
            "probe-heavy": 60,
            "probe-light": 60,
        },
        "suggest_base_ref": "origin/main",
        "separate_workspaces": {
            "backtesting_vertical_slice": {
                "path": "crates/backtesting-vertical-slice",
                "message": "backtesting-vertical-slice is a separate workspace; root Rust Probe does not cover it",
                "commands": ["just bte-fmt-check"],
            },
        },
    }


def expect_policy_error(owner: object, remote_probe: dict, fragment: str) -> None:
    try:
        owner.validate_remote_probe_policy({"remote_probe": remote_probe})
    except owner.PolicyError as exc:
        if fragment not in str(exc):
            raise AssertionError(f"expected {fragment!r} in {exc!s}") from exc
        return
    raise AssertionError(f"expected PolicyError containing {fragment!r}")


def assert_remote_probe_policy_validation() -> None:
    owner = load_owner_module()
    owner.validate_remote_probe_policy({"remote_probe": valid_remote_probe()})
    loaded = owner.remote_probe_policy({"remote_probe": valid_remote_probe()})
    if loaded["mode_runner_tiers"]["check-lib"] != "heavy":
        raise AssertionError(loaded)
    if loaded["workflow_timeouts"]["probe-heavy"] != 60:
        raise AssertionError(loaded)
    if loaded["guard_timeout_minutes"] != 1:
        raise AssertionError(loaded)
    if loaded["suggest_base_ref"] != "origin/main":
        raise AssertionError(loaded)
    separate_workspaces = loaded["separate_workspaces"]
    if separate_workspaces["crates/backtesting-vertical-slice"]["commands"] != ["just bte-fmt-check"]:
        raise AssertionError(separate_workspaces)

    heavy_only = valid_remote_probe()
    heavy_only["allowed_runner_tiers"] = ["heavy"]
    del heavy_only["workflow_timeouts"]["probe-light"]
    owner.validate_remote_probe_policy({"remote_probe": heavy_only})

    bad = valid_remote_probe()
    bad["mode_runner_tiers"]["check-lib"] = "turbo"
    expect_policy_error(owner, bad, "mode_runner_tiers.check-lib")

    bad = valid_remote_probe()
    del bad["workflow_timeouts"]["probe-light"]
    expect_policy_error(owner, bad, "workflow_timeouts")

    bad = valid_remote_probe()
    bad["appearance_timeout_seconds"] = 300
    bad["overall_timeout_seconds"] = 30
    expect_policy_error(owner, bad, "appearance_timeout_seconds")

    bad = valid_remote_probe()
    bad["overall_timeout_seconds"] = 3600
    expect_policy_error(owner, bad, "overall_timeout_seconds")

    bad = valid_remote_probe()
    bad["guard_timeout_minutes"] = 0
    expect_policy_error(owner, bad, "guard_timeout_minutes")

    bad = valid_remote_probe()
    bad["suggest_base_ref"] = "origin main"
    expect_policy_error(owner, bad, "suggest_base_ref")
    for ref in ("--octopus", "-rev", "-", "@", "@{u}", "@{1}", "main@{upstream}", "origin/{main}"):
        bad = valid_remote_probe()
        bad["suggest_base_ref"] = ref
        expect_policy_error(owner, bad, "suggest_base_ref")

    bad = valid_remote_probe()
    bad["separate_workspaces"]["backtesting_vertical_slice"]["path"] = "../outside"
    expect_policy_error(owner, bad, "separate_workspaces.backtesting_vertical_slice.path")

    bad = valid_remote_probe()
    bad["separate_workspaces"]["backtesting_vertical_slice"]["commands"] = []
    expect_policy_error(owner, bad, "separate_workspaces.backtesting_vertical_slice.commands")

    for path in (
        "/.github/workflows/rust-probe.yml",
        "../rust-probe.yml",
        ".github/workflows/rust-probe.yaml",
        ".github/workflows/rust-probe.yml/extra",
    ):
        bad = valid_remote_probe()
        bad["workflow_path"] = path
        expect_policy_error(owner, bad, "workflow_path")


def assert_repo_policy_declares_remote_probe() -> None:
    owner = load_owner_module()
    policy = owner.load_policy(REPO_ROOT)
    remote_probe = owner.remote_probe_policy(policy)
    if remote_probe["workflow_name"] != "Rust Probe":
        raise AssertionError(remote_probe)


def workflow_inputs(workflow_text: str) -> set[str]:
    inputs: set[str] = set()
    in_inputs = False
    for line in workflow_text.splitlines():
        if line == "    inputs:":
            in_inputs = True
            continue
        if in_inputs and line.startswith("      ") and not line.startswith("        ") and line.strip().endswith(":"):
            inputs.add(line.strip()[:-1])
            continue
        if in_inputs and line.startswith("  ") and not line.startswith("      "):
            break
    return inputs


def assert_workflow_contract() -> None:
    owner = load_owner_module()
    policy = owner.load_policy(REPO_ROOT)
    remote_probe = owner.remote_probe_policy(policy)
    text = WORKFLOW.read_text(encoding="utf-8")
    expected_inputs = {
        "runner_tier",
        "job_timeout_minutes",
        "ref",
        "expected_sha",
        "probe_id",
        "mode",
        "test_target",
        "test_name",
    }
    actual_inputs = workflow_inputs(text)
    if actual_inputs != expected_inputs:
        raise AssertionError((actual_inputs, expected_inputs))
    if "default: main" in text:
        raise AssertionError("rust-probe ref must not default to main")
    if "group: rust-probe" in text:
        raise AssertionError("rust-probe concurrency must not be a global constant")
    if "group: ${{ github.workflow }}-${{ github.ref }}" not in text:
        raise AssertionError("rust-probe concurrency must be branch-scoped")
    if "run-name:" not in text or "${{ inputs.probe_id }}" not in text:
        raise AssertionError("rust-probe run-name must include probe_id")
    guard_timeout = remote_probe["guard_timeout_minutes"]

    for probe_job in ("probe-heavy", "probe-light"):
        marker = f"  {probe_job}:\n"
        start = text.find(marker)
        if start < 0:
            raise AssertionError(f"rust-probe workflow missing {probe_job}")
        next_probe = text.find("\n  probe-", start + len(marker))
        block = text[start:] if next_probe < 0 else text[start:next_probe]
        expected_key = f"rust_probe.{probe_job}"
        if f"build-jobs-key: {expected_key}" not in block:
            raise AssertionError(
                f"rust-probe {probe_job} must cap cargo jobs through {expected_key}"
            )
        if 'install-rust-linker: "true"' not in block:
            raise AssertionError(
                f"rust-probe {probe_job} must install the configured Rust linker"
            )

    unsupported_marker = "  probe-unsupported-runner-tier:\n"
    unsupported_start = text.find(unsupported_marker)
    if unsupported_start < 0:
        raise AssertionError("rust-probe workflow must fail closed for unsupported runner_tier")
    unsupported_next_job = text.find("\n  probe-", unsupported_start + len(unsupported_marker))
    unsupported_block = text[unsupported_start:] if unsupported_next_job < 0 else text[unsupported_start:unsupported_next_job]
    tier_refusals = " && ".join(f"inputs.runner_tier != '{tier}'" for tier in remote_probe["allowed_runner_tiers"])
    if f"if: ${{{{ {tier_refusals} }}}}" not in unsupported_block:
        raise AssertionError("unsupported runner_tier guard must match [remote_probe].allowed_runner_tiers")
    if f"timeout-minutes: {guard_timeout}" not in unsupported_block:
        raise AssertionError("unsupported runner_tier guard must use the bounded guard timeout")
    if "RUST_PROBE_RUNNER_TIER: ${{ inputs.runner_tier }}" not in unsupported_block:
        raise AssertionError("unsupported runner_tier guard must pass input through env")
    unsupported_run_script = unsupported_block.split("run: |\n", 1)[1]
    if "${{ inputs." in unsupported_run_script:
        raise AssertionError("unsupported runner_tier guard must not interpolate inputs directly into shell")
    if 'printf "Unsupported Rust Probe runner_tier: %s\\n" "$RUST_PROBE_RUNNER_TIER" >&2' not in unsupported_run_script:
        raise AssertionError("unsupported runner_tier guard must print the env value safely")
    if "exit 1" not in unsupported_block:
        raise AssertionError("unsupported runner_tier guard must fail the workflow")

    timeout_marker = "  probe-unsupported-job-timeout:\n"
    timeout_start = text.find(timeout_marker)
    if timeout_start < 0:
        raise AssertionError("rust-probe workflow must fail closed for unsupported job_timeout_minutes")
    timeout_next_job = text.find("\n  probe-", timeout_start + len(timeout_marker))
    timeout_block = text[timeout_start:] if timeout_next_job < 0 else text[timeout_start:timeout_next_job]
    timeout_refusals = " || ".join(
        f"(inputs.runner_tier == '{tier}' && inputs.job_timeout_minutes != '{remote_probe['workflow_timeouts'][f'probe-{tier}']}')"
        for tier in remote_probe["allowed_runner_tiers"]
    )
    if f"if: ${{{{ {timeout_refusals} }}}}" not in timeout_block:
        raise AssertionError("unsupported job_timeout guard must match [remote_probe.workflow_timeouts]")
    if f"timeout-minutes: {guard_timeout}" not in timeout_block:
        raise AssertionError("unsupported job_timeout guard must use the bounded guard timeout")
    if "RUST_PROBE_JOB_TIMEOUT_MINUTES: ${{ inputs.job_timeout_minutes }}" not in timeout_block:
        raise AssertionError("unsupported job_timeout guard must pass input through env")
    timeout_run_script = timeout_block.split("run: |\n", 1)[1]
    if "${{ inputs." in timeout_run_script:
        raise AssertionError("unsupported job_timeout guard must not interpolate inputs directly into shell")
    if 'printf "Unsupported Rust Probe job_timeout_minutes: %s\\n" "$RUST_PROBE_JOB_TIMEOUT_MINUTES" >&2' not in timeout_run_script:
        raise AssertionError("unsupported job_timeout guard must print the env value safely")
    if "exit 1" not in timeout_block:
        raise AssertionError("unsupported job_timeout guard must fail the workflow")

    for job in remote_probe["workflow_timeouts"]:
        marker = f"  {job}:\n"
        start = text.find(marker)
        if start < 0:
            raise AssertionError(f"missing job {job}")
        next_job = text.find("\n  probe-", start + len(marker))
        block = text[start:] if next_job < 0 else text[start:next_job]
        tier = job.removeprefix("probe-")
        expected_timeout = remote_probe["workflow_timeouts"][job]
        expected_if = f"if: ${{{{ inputs.runner_tier == '{tier}' && inputs.job_timeout_minutes == '{expected_timeout}' }}}}"
        if expected_if not in block:
            raise AssertionError(f"{job} must require its TOML-declared timeout before running")
        if f"timeout-minutes: {expected_timeout}" in block:
            raise AssertionError(f"{job} timeout-minutes must not hardcode its TOML-declared timeout")
        if "timeout-minutes: ${{ fromJSON(inputs.job_timeout_minutes) }}" not in block:
            raise AssertionError(f"{job} timeout-minutes must be wrapper-provided from policy")
        if "fetch-depth: 1" not in block:
            raise AssertionError(f"{job} must use shallow checkout")
        if "RUST_PROBE_EXPECTED_SHA: ${{ inputs.expected_sha }}" not in block:
            raise AssertionError(f"{job} must pass expected SHA to runner")
        if "RUST_PROBE_ID: ${{ inputs.probe_id }}" not in block:
            raise AssertionError(f"{job} must pass probe id to runner")


def assert_rust_probe_not_merge_proof() -> None:
    workflow_text = WORKFLOW.read_text(encoding="utf-8")
    ci_text = CI_WORKFLOW.read_text(encoding="utf-8")
    if "pull_request:" in workflow_text or "\npush:" in workflow_text:
        raise AssertionError("Rust Probe must remain workflow_dispatch-only")
    ci_text_without_operator_hint = ci_text.lower().replace("just rust-probe suggest", "")
    if "rust-probe" in ci_text_without_operator_hint:
        raise AssertionError("Rust Probe must not be added to full CI or gate needs")
    policy_text = POLICY.read_text(encoding="utf-8")
    full_ci_index = policy_text.find("[ci_provenance.full_ci]")
    if full_ci_index >= 0 and "rust-probe" in policy_text[full_ci_index:].lower():
        raise AssertionError("Rust Probe must not be a full-CI required job")


def assert_parser_exposes_rust_probe() -> None:
    owner = load_owner_module()
    args = owner.build_parser().parse_args(
        ["rust-probe", "--repo", "/tmp/repo", "nextest-test-target-name", "target_name", "test_name"]
    )
    if args.command_name != "rust-probe":
        raise AssertionError(args)
    if args.mode != "nextest-test-target-name" or args.test_target != "target_name" or args.test_name != "test_name":
        raise AssertionError(args)
    with contextlib.redirect_stderr(io.StringIO()):
        args = owner.build_parser().parse_args(["rust-probe", "--repo", "/tmp/repo", "--runner-tier", "policy-tier", "check-lib"])
    if args.runner_tier != "policy-tier":
        raise AssertionError(args)


def assert_parser_help_exposes_suggest_and_examples() -> None:
    owner = load_owner_module()
    stdout = io.StringIO()
    try:
        with contextlib.redirect_stdout(stdout):
            owner.build_parser().parse_args(["rust-probe", "--help"])
    except SystemExit as exc:
        if exc.code != 0:
            raise AssertionError(exc.code) from exc
    else:
        raise AssertionError("rust-probe --help should exit after printing help")
    help_text = stdout.getvalue()
    for fragment in (
        "suggest",
        "Examples:",
        "just rust-probe suggest",
        "just rust-probe check-test-target <harness_target>",
        "just rust-probe nextest-lib-name <test_name>",
        "just rust-probe nextest-test-target-name <harness_target> <member_stem>::",
    ):
        if fragment not in help_text:
            raise AssertionError(f"rust-probe help missing {fragment!r}:\n{help_text}")


def assert_validation_errors_point_to_suggest() -> None:
    owner = load_owner_module()
    error = owner.validate_rust_probe_selection("nextest-test-target", "", "")
    if error is None or "just rust-probe suggest" not in error or "Examples:" not in error:
        raise AssertionError(error)
    stdout = io.StringIO()
    stderr = io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        result = owner.cmd_rust_probe(
            types.SimpleNamespace(
                repo=str(REPO_ROOT),
                mode="suggest",
                test_target="unexpected-target",
                test_name=None,
                runner_tier=None,
            )
        )
    output = stdout.getvalue() + stderr.getvalue()
    if result != 2 or "suggest does not accept test_target or test_name" not in output:
        raise AssertionError((result, output))


def write_test_manifest_fixture(root: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path]:
    tests_root = root / "tests"
    tests_root.mkdir()
    manifest_path = root / "Cargo.toml"
    manifest_path.write_text(
        textwrap.dedent(
            """\
            [package]
            name = "rust-probe-manifest-fixture"
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
    (tests_root / "foo.rs").write_text("#[test]\nfn foo_works() {}\n", encoding="utf-8")
    return manifest_path, tests_root


def assert_fixture_manifest_suggestions_use_harness_and_member_filter() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        manifest_path, tests_root = write_test_manifest_fixture(pathlib.Path(tmp))
        member_suggestions = owner.rust_probe_suggestions(
            ["tests/bolt_v3_iv_source_fence.rs"],
            {},
            manifest_path=manifest_path,
            tests_root=tests_root,
        )
        standalone_suggestions = owner.rust_probe_suggestions(
            ["tests/foo.rs"],
            {},
            manifest_path=manifest_path,
            tests_root=tests_root,
        )
    expected_member = [
        "just rust-probe check-test-target iv",
        "just rust-probe nextest-no-run-test-target iv",
        "just rust-probe nextest-test-target iv",
        "just rust-probe nextest-test-target-name iv bolt_v3_iv_source_fence::",
    ]
    for command in expected_member:
        if command not in member_suggestions:
            raise AssertionError((command, member_suggestions))
    if any("nextest-test-target-name bolt_v3_iv_source_fence " in command for command in member_suggestions):
        raise AssertionError(member_suggestions)
    expected_standalone = [
        "just rust-probe check-test-target foo",
        "just rust-probe nextest-no-run-test-target foo",
        "just rust-probe nextest-test-target foo",
    ]
    for command in expected_standalone:
        if command not in standalone_suggestions:
            raise AssertionError((command, standalone_suggestions))
    if any("nextest-test-target-name foo foo::" in command for command in standalone_suggestions):
        raise AssertionError(("harness-root self-filter must be omitted", standalone_suggestions))


def assert_changed_files_produce_targeted_suggestions() -> None:
    owner = load_owner_module()
    separate_workspaces = owner.remote_probe_policy({"remote_probe": valid_remote_probe()})["separate_workspaces"]
    suggestions = owner.rust_probe_suggestions(
        [
            "src/lib.rs",
            "tests/build_script_git_head_rerun_paths.rs",
            "Cargo.lock",
            "docs/ci/ubicloud-cost-governance.md",
        ],
        separate_workspaces,
    )
    expected = [
        "just rust-probe check-lib",
        "just rust-probe check-test-target platform_config",
        "just rust-probe nextest-no-run-test-target platform_config",
        "just rust-probe nextest-test-target platform_config",
        "just rust-probe nextest-test-target-name platform_config build_script_git_head_rerun_paths::",
    ]
    for command in expected:
        if command not in suggestions:
            raise AssertionError((command, suggestions))
    bte_suggestions = owner.rust_probe_suggestions(
        ["crates/backtesting-vertical-slice/src/lib.rs"],
        separate_workspaces,
    )
    if any(suggestion == "just rust-probe check-lib" for suggestion in bte_suggestions):
        raise AssertionError(bte_suggestions)
    if not any("backtesting-vertical-slice" in suggestion for suggestion in bte_suggestions):
        raise AssertionError(bte_suggestions)
    if "just bte-fmt-check" not in bte_suggestions:
        raise AssertionError(bte_suggestions)
    generic_suggestions = owner.rust_probe_suggestions([], separate_workspaces)
    if "No Rust source or top-level integration-test target was inferred from changed files." not in generic_suggestions:
        raise AssertionError(generic_suggestions)
    docs_only_suggestions = owner.rust_probe_suggestions(["docs/ci/ubicloud-cost-governance.md"], separate_workspaces)
    if "No targeted Rust Probe command was inferred." not in docs_only_suggestions:
        raise AssertionError(docs_only_suggestions)
    if "just rust-probe check-lib" in docs_only_suggestions:
        raise AssertionError(docs_only_suggestions)
    nested_test_suggestions = owner.rust_probe_suggestions(["tests/support/mod.rs"], separate_workspaces)
    if any("support" in suggestion for suggestion in nested_test_suggestions):
        raise AssertionError(nested_test_suggestions)
    unknown_crate_suggestions = owner.rust_probe_suggestions(["crates/future-workspace/src/lib.rs"], separate_workspaces)
    if "just rust-probe check-lib" in unknown_crate_suggestions:
        raise AssertionError(unknown_crate_suggestions)


def assert_changed_files_use_integration_base_not_feature_upstream() -> None:
    owner = load_owner_module()
    merge_base = "b" * 40
    outputs = {
        ("diff", "--name-only", "HEAD", "--"): ("scripts/rust_verification.py\n", None),
        ("ls-files", "--others", "--exclude-standard"): ("docs/new-rust-probe-note.md\n", None),
        ("merge-base", "origin/main", "HEAD"): (merge_base, None),
        ("diff", "--name-only", merge_base, "HEAD", "--"): ("src/lib.rs\ntests/config_parsing.rs\n", None),
    }
    calls: list[tuple[str, ...]] = []

    def fake_git_output(_repo: pathlib.Path, *args: str) -> tuple[str | None, str | None]:
        calls.append(args)
        if args not in outputs:
            raise AssertionError(f"unexpected git call: {args}")
        return outputs[args]

    original_git_output = owner.git_output
    try:
        owner.git_output = fake_git_output
        changed, error, notes = owner.rust_probe_changed_files(REPO_ROOT, "origin/main")
    finally:
        owner.git_output = original_git_output
    if error is not None:
        raise AssertionError(error)
    if notes:
        raise AssertionError(notes)
    if changed != [
        "docs/new-rust-probe-note.md",
        "scripts/rust_verification.py",
        "src/lib.rs",
        "tests/config_parsing.rs",
    ]:
        raise AssertionError((changed, calls))
    outputs[("merge-base", "origin/main", "HEAD")] = (None, "git exited 128")
    outputs[("diff", "--name-only", "origin/main", "HEAD", "--")] = ("docs/fallback-diff.md\n", None)
    calls.clear()
    original_git_output = owner.git_output
    try:
        owner.git_output = fake_git_output
        changed, error, notes = owner.rust_probe_changed_files(REPO_ROOT, "origin/main")
    finally:
        owner.git_output = original_git_output
    if error is not None:
        raise AssertionError(error)
    if not any("merge-base" in note and "direct" in note for note in notes):
        raise AssertionError(notes)
    if changed != [
        "docs/fallback-diff.md",
        "docs/new-rust-probe-note.md",
        "scripts/rust_verification.py",
    ]:
        raise AssertionError((changed, error, calls))
    outputs[("diff", "--name-only", "origin/main", "HEAD", "--")] = (None, "git exited 129")
    calls.clear()
    original_git_output = owner.git_output
    try:
        owner.git_output = fake_git_output
        changed, error, notes = owner.rust_probe_changed_files(REPO_ROOT, "origin/main")
    finally:
        owner.git_output = original_git_output
    if changed is not None or error is None or "could not resolve configured base ref 'origin/main'" not in error:
        raise AssertionError((changed, error, notes, calls))


def assert_cmd_rust_probe_suggest_reports_policy_and_rejects_runner_tier() -> None:
    owner = load_owner_module()
    changed_calls: list[tuple[pathlib.Path, str]] = []

    def fake_load_policy(_repo: pathlib.Path) -> dict:
        return {"remote_probe": valid_remote_probe()}

    def fake_changed_files(repo: pathlib.Path, suggest_base_ref: str) -> tuple[list[str] | None, str | None, list[str]]:
        changed_calls.append((repo, suggest_base_ref))
        return ["tests/config_parsing.rs"], None, ["using direct base-to-HEAD tree diff"]

    original_load_policy = owner.load_policy
    original_changed_files = owner.rust_probe_changed_files
    try:
        owner.load_policy = fake_load_policy
        owner.rust_probe_changed_files = fake_changed_files
        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = owner.cmd_rust_probe_suggest(
                types.SimpleNamespace(repo=str(REPO_ROOT), runner_tier=None)
            )
        if result != 0:
            raise AssertionError((result, stdout.getvalue(), stderr.getvalue()))
        output = stdout.getvalue()
        if "tests/config_parsing.rs" not in output:
            raise AssertionError(output)
        if "just rust-probe check-test-target platform_config" not in output:
            raise AssertionError(output)
        if "just rust-probe nextest-test-target-name platform_config config_parsing::" not in output:
            raise AssertionError(output)
        if "Rust Probe is not merge proof" not in output:
            raise AssertionError(output)
        if "base ref: origin/main" not in output or "fetched and current" not in output:
            raise AssertionError(output)
        if "using direct base-to-HEAD tree diff" not in output:
            raise AssertionError(output)
        if changed_calls != [(REPO_ROOT, "origin/main")]:
            raise AssertionError(changed_calls)
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            result = owner.cmd_rust_probe_suggest(
                types.SimpleNamespace(repo=str(REPO_ROOT), runner_tier="")
            )
        if result != 2 or "suggest does not accept --runner-tier" not in stderr.getvalue():
            raise AssertionError((result, stderr.getvalue()))
        owner.load_policy = lambda _repo: (_ for _ in ()).throw(owner.PolicyError("policy is invalid"))
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            result = owner.cmd_rust_probe_suggest(
                types.SimpleNamespace(repo=str(REPO_ROOT), runner_tier=None)
            )
        if result != 2 or "policy is invalid" not in stderr.getvalue():
            raise AssertionError((result, stderr.getvalue()))
    finally:
        owner.load_policy = original_load_policy
        owner.rust_probe_changed_files = original_changed_files


def assert_preconditions_are_pr_free_and_exact_upstream() -> None:
    owner = load_owner_module()
    pushed_outputs = {
        ("status", "--porcelain", "--untracked-files=normal"): ("", None),
        ("rev-parse", "HEAD"): (HEAD, None),
        ("branch", "--show-current"): (BRANCH, None),
        ("config", f"branch.{BRANCH}.remote"): ("origin", None),
        ("config", f"branch.{BRANCH}.merge"): (f"refs/heads/{BRANCH}", None),
        ("remote", "get-url", "--push", "--all", "origin"): ("https://example.invalid/push.git", None),
        ("ls-remote", "--heads", "--", "https://example.invalid/push.git", BRANCH): (
            f"{HEAD}\trefs/heads/{BRANCH}",
            None,
        ),
    }

    def run_with_git_outputs(
        outputs: dict[tuple[str, ...], tuple[str | None, str | None]],
    ) -> tuple[str | None, str | None, str | None, list[tuple[str, ...]]]:
        calls: list[tuple[str, ...]] = []

        def fake_git_output(_repo: pathlib.Path, *args: str) -> tuple[str | None, str | None]:
            calls.append(args)
            if args not in outputs:
                raise AssertionError(f"unexpected git call: {args}")
            return outputs[args]

        original_git_output = owner.git_output
        try:
            owner.git_output = fake_git_output
            head, branch, error = owner.ensure_rust_probe_preconditions(REPO_ROOT)
        finally:
            owner.git_output = original_git_output
        return head, branch, error, calls

    head, branch, error, calls = run_with_git_outputs(pushed_outputs)

    if (head, branch, error) != (HEAD, BRANCH, None):
        raise AssertionError((head, branch, error))
    if any(call and call[0] == "pr" for call in calls):
        raise AssertionError(calls)

    local_branch_outputs = {
        ("status", "--porcelain", "--untracked-files=normal"): ("", None),
        ("rev-parse", "HEAD"): (HEAD, None),
        ("branch", "--show-current"): (LOCAL_BRANCH, None),
        ("config", f"branch.{LOCAL_BRANCH}.remote"): ("origin", None),
        ("config", f"branch.{LOCAL_BRANCH}.merge"): (f"refs/heads/{UPSTREAM_BRANCH}", None),
        ("remote", "get-url", "--push", "--all", "origin"): ("https://example.invalid/push.git", None),
        ("ls-remote", "--heads", "--", "https://example.invalid/push.git", UPSTREAM_BRANCH): (
            f"{HEAD}\trefs/heads/{UPSTREAM_BRANCH}",
            None,
        ),
    }
    head, branch, error, _calls = run_with_git_outputs(local_branch_outputs)
    if (head, branch, error) != (HEAD, UPSTREAM_BRANCH, None):
        raise AssertionError((head, branch, error))

    no_local_upstream_outputs = {
        ("status", "--porcelain", "--untracked-files=normal"): ("", None),
        ("rev-parse", "HEAD"): (HEAD, None),
        ("branch", "--show-current"): (BRANCH, None),
        ("config", f"branch.{BRANCH}.remote"): ("", None),
        ("remote",): ("origin", None),
        ("remote", "get-url", "--push", "--all", "origin"): ("https://example.invalid/push.git", None),
        ("ls-remote", "--heads", "--", "https://example.invalid/push.git", BRANCH): (
            f"{HEAD}\trefs/heads/{BRANCH}",
            None,
        ),
    }
    head, branch, error, calls = run_with_git_outputs(no_local_upstream_outputs)
    if (head, branch, error) != (HEAD, BRANCH, None):
        raise AssertionError((head, branch, error))
    if ("ls-remote", "--heads", "--", "https://example.invalid/push.git", BRANCH) not in calls:
        raise AssertionError(calls)

    refusal_cases = [
        (
            "dirty worktree",
            {("status", "--porcelain", "--untracked-files=normal"): ("?? scratch.rs", None)},
            "rust-probe requires a clean worktree",
        ),
        (
            "missing same-name remote branch",
            {
                ("config", f"branch.{BRANCH}.remote"): ("", None),
                ("remote",): ("origin", None),
                ("remote", "get-url", "--push", "--all", "origin"): ("https://example.invalid/push.git", None),
                ("ls-remote", "--heads", "--", "https://example.invalid/push.git", BRANCH): ("", None),
            },
            "just sandbox-safe-push",
        ),
        (
            "unpushed head",
            {
                ("ls-remote", "--heads", "--", "https://example.invalid/push.git", BRANCH): (
                    f"{'b' * 40}\trefs/heads/{BRANCH}",
                    None,
                )
            },
            "rust-probe requires HEAD to be pushed to the upstream branch",
        ),
    ]
    for label, overrides, fragment in refusal_cases:
        outputs = dict(pushed_outputs)
        outputs.update(overrides)
        head, branch, error, _calls = run_with_git_outputs(outputs)
        if head is not None or branch is not None or error is None or fragment not in error:
            raise AssertionError((label, head, branch, error))


def assert_dispatch_uses_declared_workflow_inputs() -> None:
    owner = load_owner_module()
    policy = owner.remote_probe_policy({"remote_probe": valid_remote_probe()})
    calls: list[list[str]] = []

    def fake_run_capture(argv: list[str], *, repo: pathlib.Path) -> subprocess.CompletedProcess[str]:
        calls.append(argv)
        return subprocess.CompletedProcess(argv, 0, "", "")

    original_run_capture = owner.run_capture
    try:
        owner.run_capture = fake_run_capture
        error = owner.dispatch_rust_probe(
            REPO_ROOT,
            policy,
            branch=UPSTREAM_BRANCH,
            head=HEAD,
            mode="nextest-test-target-name",
            test_target="target_name",
            test_name="case_name",
            runner_tier="heavy",
            job_timeout_minutes=60,
            probe_id="probe-123",
        )
    finally:
        owner.run_capture = original_run_capture

    if error is not None:
        raise AssertionError(error)
    expected = [
        "gh",
        "workflow",
        "run",
        ".github/workflows/rust-probe.yml",
        "--ref",
        UPSTREAM_BRANCH,
        "-f",
        "runner_tier=heavy",
        "-f",
        "job_timeout_minutes=60",
        "-f",
        f"ref={HEAD}",
        "-f",
        f"expected_sha={HEAD}",
        "-f",
        "probe_id=probe-123",
        "-f",
        "mode=nextest-test-target-name",
        "-f",
        "test_target=target_name",
        "-f",
        "test_name=case_name",
    ]
    if calls != [expected]:
        raise AssertionError(calls)


def assert_cancelled_probe_is_superseded_not_code_failure() -> None:
    owner = load_owner_module()
    run = {
        "databaseId": 42,
        "headSha": HEAD,
        "status": "completed",
        "conclusion": "cancelled",
        "displayTitle": "Rust Probe probe-123 check-lib",
        "url": "https://example.invalid/runs/42",
    }
    stderr = io.StringIO()
    with contextlib.redirect_stderr(stderr):
        result = owner.evaluate_rust_probe_run(run, head=HEAD, probe_id="probe-123")
    if result != 2:
        raise AssertionError((result, stderr.getvalue()))
    output = stderr.getvalue()
    if "superseded" not in output or "failed for" in output:
        raise AssertionError(output)


def assert_wrong_head_probe_is_not_success() -> None:
    owner = load_owner_module()
    run = {
        "databaseId": 42,
        "headSha": "b" * 40,
        "status": "completed",
        "conclusion": "success",
        "displayTitle": "Rust Probe probe-123 check-lib",
        "url": "https://example.invalid/runs/42",
    }
    stderr = io.StringIO()
    with contextlib.redirect_stderr(stderr):
        result = owner.evaluate_rust_probe_run(run, head=HEAD, probe_id="probe-123")
    if result != 2:
        raise AssertionError((result, stderr.getvalue()))
    output = stderr.getvalue()
    if "exact-head evidence" not in output or "passed for" in output:
        raise AssertionError(output)


def assert_rust_probe_polling_errors_fail_closed() -> None:
    owner = load_owner_module()
    policy = owner.remote_probe_policy({"remote_probe": valid_remote_probe()})
    original_run_capture = owner.run_capture

    def invalid_json(argv: list[str], *, repo: pathlib.Path) -> subprocess.CompletedProcess[str]:
        return subprocess.CompletedProcess(argv, 0, "{", "")

    def os_error(_argv: list[str], *, repo: pathlib.Path) -> subprocess.CompletedProcess[str]:
        raise OSError("network unavailable")

    try:
        owner.run_capture = invalid_json
        runs, error = owner.rust_probe_run_list(REPO_ROOT, policy, branch=BRANCH)
        if runs is not None or error is None or "returned invalid JSON" not in error:
            raise AssertionError((runs, error))
        run, error = owner.workflow_run_view(REPO_ROOT, 123, command_name="rust-probe")
        if run is not None or error is None or "rust-probe could not inspect" not in error or "returned invalid JSON" not in error:
            raise AssertionError((run, error))

        owner.run_capture = os_error
        runs, error = owner.rust_probe_run_list(REPO_ROOT, policy, branch=BRANCH)
        if runs is not None or error is None or "could not run" not in error:
            raise AssertionError((runs, error))
        run, error = owner.workflow_run_view(REPO_ROOT, 123, command_name="rust-probe")
        if run is not None or error is None or "rust-probe could not inspect" not in error or "could not run" not in error:
            raise AssertionError((run, error))
    finally:
        owner.run_capture = original_run_capture


def assert_probe_run_matching_is_prefix_anchored() -> None:
    owner = load_owner_module()
    runs = [
        {
            "displayTitle": "Other Rust Probe probe-123 check-lib",
            "headSha": HEAD,
            "createdAt": "2026-06-15T00:00:03Z",
        },
        {
            "displayTitle": "Rust Probe xprobe-123 check-lib",
            "headSha": HEAD,
            "createdAt": "2026-06-15T00:00:02Z",
        },
        {
            "displayTitle": "Rust Probe probe-123 check-lib @ bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "headSha": "b" * 40,
            "createdAt": "2026-06-15T00:00:04Z",
        },
        {
            "displayTitle": "Rust Probe probe-123 check-lib @ aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "headSha": HEAD,
            "createdAt": "2026-06-15T00:00:01Z",
        },
    ]
    matching = owner.matching_rust_probe_runs(runs, head=HEAD, probe_id="probe-123")
    if matching != [runs[3]]:
        raise AssertionError(matching)


def assert_cmd_rust_probe_dispatches_and_reports_not_proof() -> None:
    owner = load_owner_module()
    calls: list[tuple[str, str, str, str, str, str, int, str]] = []

    original_load_policy = owner.load_policy
    original_preconditions = owner.ensure_rust_probe_preconditions
    original_active = owner.rust_probe_active_run_count
    original_dispatch = owner.dispatch_rust_probe
    original_wait = owner.wait_for_rust_probe_run
    original_probe_id = owner.new_probe_id
    try:
        owner.load_policy = lambda _repo: {"remote_probe": valid_remote_probe()}
        owner.ensure_rust_probe_preconditions = lambda _repo: (HEAD, UPSTREAM_BRANCH, None)
        owner.rust_probe_active_run_count = lambda _repo, _policy: (0, None)
        owner.new_probe_id = lambda: "probe-123"

        def fake_dispatch(
            _repo: pathlib.Path,
            _policy: dict,
            *,
            branch: str,
            head: str,
            mode: str,
            test_target: str,
            test_name: str,
            runner_tier: str,
            job_timeout_minutes: int,
            probe_id: str,
        ) -> str | None:
            calls.append((branch, head, mode, test_target, test_name, runner_tier, job_timeout_minutes, probe_id))
            return None

        owner.dispatch_rust_probe = fake_dispatch
        owner.wait_for_rust_probe_run = lambda **_kwargs: 0
        stdout = io.StringIO()
        stderr = io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            result = owner.cmd_rust_probe(
                types.SimpleNamespace(
                    repo=str(REPO_ROOT),
                    mode="check-lib",
                    test_target=None,
                    test_name=None,
                    runner_tier=None,
                )
            )
    finally:
        owner.load_policy = original_load_policy
        owner.ensure_rust_probe_preconditions = original_preconditions
        owner.rust_probe_active_run_count = original_active
        owner.dispatch_rust_probe = original_dispatch
        owner.wait_for_rust_probe_run = original_wait
        owner.new_probe_id = original_probe_id

    if result != 0:
        raise AssertionError((result, stdout.getvalue(), stderr.getvalue()))
    if calls != [(UPSTREAM_BRANCH, HEAD, "check-lib", "", "", "heavy", 60, "probe-123")]:
        raise AssertionError(calls)
    if "NOT MERGE PROOF" not in stdout.getvalue():
        raise AssertionError(stdout.getvalue())


def main() -> int:
    assert_remote_probe_policy_validation()
    assert_repo_policy_declares_remote_probe()
    assert_workflow_contract()
    assert_rust_probe_not_merge_proof()
    assert_parser_exposes_rust_probe()
    assert_parser_help_exposes_suggest_and_examples()
    assert_validation_errors_point_to_suggest()
    assert_fixture_manifest_suggestions_use_harness_and_member_filter()
    assert_changed_files_produce_targeted_suggestions()
    assert_changed_files_use_integration_base_not_feature_upstream()
    assert_cmd_rust_probe_suggest_reports_policy_and_rejects_runner_tier()
    assert_preconditions_are_pr_free_and_exact_upstream()
    assert_dispatch_uses_declared_workflow_inputs()
    assert_cancelled_probe_is_superseded_not_code_failure()
    assert_wrong_head_probe_is_not_success()
    assert_rust_probe_polling_errors_fail_closed()
    assert_probe_run_matching_is_prefix_anchored()
    assert_cmd_rust_probe_dispatches_and_reports_not_proof()
    print("OK: Rust Probe wrapper self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
