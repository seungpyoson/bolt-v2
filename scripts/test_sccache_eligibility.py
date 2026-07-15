#!/usr/bin/env python3
"""Self-tests for the governed sccache eligibility resolver."""

from __future__ import annotations

import hashlib
import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import tomllib

from sccache_eligibility import (
    SccacheConfigError,
    parse_sccache_config,
    resolve_sccache_eligibility,
)


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
CONFIG_TEXT = (REPO_ROOT / "ci" / "sccache-location.toml").read_text(encoding="utf-8")
LOCATION = tomllib.loads(CONFIG_TEXT)["location"]
OWNER = REPO_ROOT / "scripts" / "sccache_eligibility.py"
RUST_OWNER = REPO_ROOT / "scripts" / "rust_verification.py"


def write_executable(path: pathlib.Path, text: str) -> None:
    path.write_text(text, encoding="utf-8")
    path.chmod(0o755)


def installer_config(*, digest: str, asset_target: str = "aarch64-unknown-linux-musl") -> str:
    return textwrap.dedent(
        f"""\
        schema_version = 2

        [location]
        bucket = "bucket"
        region = "region"
        key_prefix = "prefix/"

        [installer]
        version = "v0.10.0"
        asset_target = "{asset_target}"
        executable = "sccache"
        version_output = "sccache 0.10.0"
        executable_sha256 = "{digest}"
        """
    )


def run_owner(command: str, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(OWNER), command],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def parse_outputs(path: pathlib.Path) -> dict[str, str]:
    return dict(line.split("=", 1) for line in path.read_text(encoding="utf-8").splitlines())


def assert_strict_and_legacy_behavior() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        bin_dir = root / "bin"
        bin_dir.mkdir()
        wrapper = bin_dir / "sccache"
        log = root / "wrapper.log"
        write_executable(
            wrapper,
            """#!/bin/sh
printf '%s\\n' "$1" >> "$FAKE_SCCACHE_LOG"
case "$1" in
  --version) printf '%s\\n' "${FAKE_SCCACHE_VERSION:-sccache 0.10.0}" ;;
  --start-server) exit "${FAKE_SCCACHE_START_STATUS:-0}" ;;
  --zero-stats) exit 0 ;;
  *) exit 7 ;;
esac
""",
        )
        write_executable(
            bin_dir / "uname",
            "#!/bin/sh\nprintf '%s\\n' \"${FAKE_UNAME_MACHINE:-aarch64}\"\n",
        )
        digest = hashlib.sha256(wrapper.read_bytes()).hexdigest()
        config = root / "sccache.toml"
        config.write_text(installer_config(digest=digest), encoding="utf-8")

        def strict_env(**overrides: str) -> tuple[dict[str, str], pathlib.Path, pathlib.Path]:
            output = root / f"output-{len(list(root.glob('output-*')))}"
            github_env = root / f"env-{len(list(root.glob('env-*')))}"
            output.touch()
            github_env.touch()
            values = {
                **os.environ,
                "PATH": f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '')}",
                "CONFIG_PATH": str(config),
                "GITHUB_OUTPUT": str(output),
                "GITHUB_ENV": str(github_env),
                "ELIGIBILITY_OUTCOME": "success",
                "SCCACHE_ELIGIBLE": "true",
                "AWS_OUTCOME": "success",
                "INSTALL_OUTCOME": "success",
                "SCCACHE_PATH": str(wrapper),
                "FAKE_SCCACHE_LOG": str(log),
            }
            values.update(overrides)
            return values, output, github_env

        build_sentinel = root / "build-sentinel"
        success_env, output, github_env = strict_env()
        success = run_owner("strict-setup", success_env)
        if success.returncode != 0:
            raise AssertionError((success.stdout, success.stderr))
        build_sentinel.write_text("build\n", encoding="utf-8")
        outputs = parse_outputs(output)
        if outputs != {"wrapper_path": str(wrapper.resolve()), "enabled": "true"}:
            raise AssertionError(outputs)
        exported = parse_outputs(github_env)
        if exported.get("SCCACHE_PATH") != str(wrapper.resolve()) or exported.get(
            "BOLT_RUST_VERIFICATION_SCCACHE"
        ) != "1":
            raise AssertionError(exported)
        if build_sentinel.read_text(encoding="utf-8") != "build\n":
            raise AssertionError("strict success did not reach the build sentinel exactly once")

        prebuild_log = root / "prebuild-sentinel"

        def prebuild(action_wrapper: pathlib.Path, ambient_wrapper: pathlib.Path) -> subprocess.CompletedProcess[str]:
            result = subprocess.run(
                [
                    sys.executable,
                    str(RUST_OWNER),
                    "root-artifact-wrapper",
                    "--repo",
                    str(REPO_ROOT),
                    "--action-wrapper",
                    str(action_wrapper),
                    "--sccache-config",
                    str(config),
                ],
                cwd=REPO_ROOT,
                env={
                    **success_env,
                    "SCCACHE_PATH": str(ambient_wrapper),
                    "BOLT_RUST_VERIFICATION_SCCACHE": "1",
                    "GITHUB_ACTIONS": "true",
                },
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            if result.returncode == 0:
                with prebuild_log.open("a", encoding="utf-8") as handle:
                    handle.write("build\n")
            return result

        prebuild_success = prebuild(wrapper, wrapper)
        if prebuild_success.returncode != 0:
            raise AssertionError((prebuild_success.stdout, prebuild_success.stderr))
        other_dir = root / "other"
        other_dir.mkdir()
        other_wrapper = other_dir / "sccache"
        other_wrapper.write_bytes(wrapper.read_bytes())
        other_wrapper.chmod(0o755)
        if prebuild(other_wrapper, wrapper).returncode == 0:
            raise AssertionError("action-output/ambient wrapper mismatch reached the build sentinel")
        original_wrapper = wrapper.read_bytes()
        wrapper.write_bytes(original_wrapper + b"\n# mutated after setup\n")
        wrapper.chmod(0o755)
        if prebuild(wrapper, wrapper).returncode == 0:
            raise AssertionError("wrapper mutation after strict setup reached the build sentinel")
        wrapper.write_bytes(original_wrapper)
        wrapper.chmod(0o755)
        if prebuild_log.read_text(encoding="utf-8").splitlines() != ["build"]:
            raise AssertionError("pre-build wrapper binding did not reach the build sentinel exactly once")

        wrong_name = bin_dir / "wrong-name"
        wrong_name.write_bytes(wrapper.read_bytes())
        wrong_name.chmod(0o755)
        non_executable = root / "sccache"
        non_executable.write_bytes(wrapper.read_bytes())
        non_executable.chmod(0o644)
        wrong_digest_config = root / "wrong-digest.toml"
        wrong_digest_config.write_text(installer_config(digest="0" * 64), encoding="utf-8")
        wrong_target_config = root / "wrong-target.toml"
        wrong_target_config.write_text(
            installer_config(digest=digest, asset_target="x86_64-unknown-linux-musl"),
            encoding="utf-8",
        )
        failures = (
            ("missing path", {"SCCACHE_PATH": ""}),
            ("relative path", {"SCCACHE_PATH": "sccache"}),
            ("non-executable path", {"SCCACHE_PATH": str(non_executable)}),
            ("wrong basename", {"SCCACHE_PATH": str(wrong_name)}),
            ("wrong host target", {"CONFIG_PATH": str(wrong_target_config)}),
            ("wrong version", {"FAKE_SCCACHE_VERSION": "sccache 9.9.9"}),
            ("wrong executable digest", {"CONFIG_PATH": str(wrong_digest_config)}),
            ("credential failure", {"AWS_OUTCOME": "failure"}),
            ("install failure", {"INSTALL_OUTCOME": "failure"}),
            ("server start failure", {"FAKE_SCCACHE_START_STATUS": "9"}),
        )
        for label, overrides in failures:
            case_env, _, _ = strict_env(**overrides)
            result = run_owner("strict-setup", case_env)
            if result.returncode == 0:
                raise AssertionError(f"{label} must stop before the build sentinel")

        legacy_log = root / "legacy.log"
        legacy_output = root / "legacy-output"
        legacy_env = {
            **os.environ,
            "GITHUB_OUTPUT": str(legacy_output),
            "SCCACHE_ELIGIBLE": "true",
            "AWS_OUTCOME": "success",
            "INSTALL_OUTCOME": "success",
            "SCCACHE_PATH": str(wrapper),
            "FAKE_SCCACHE_LOG": str(legacy_log),
            "FAKE_UNAME_MACHINE": "x86_64",
        }
        legacy = run_owner("legacy-enable", legacy_env)
        if legacy.returncode != 0 or parse_outputs(legacy_output).get("enabled") != "true":
            raise AssertionError((legacy.stdout, legacy.stderr))
        legacy_calls = legacy_log.read_text(encoding="utf-8").splitlines()
        if legacy_calls != ["--start-server", "--zero-stats"]:
            raise AssertionError(f"legacy mode invoked strict checks: {legacy_calls!r}")

        degraded_output = root / "legacy-degraded-output"
        degraded_env = {**legacy_env, "GITHUB_OUTPUT": str(degraded_output), "AWS_OUTCOME": "failure"}
        degraded = run_owner("legacy-enable", degraded_env)
        if degraded.returncode != 0 or parse_outputs(degraded_output).get("enabled") != "false":
            raise AssertionError((degraded.stdout, degraded.stderr))


def assert_case(
    label: str,
    *,
    active: bool = True,
    event_name: str,
    github_ref: str,
    read_role_arn: str = "arn:aws:iam::123456789012:role/read",
    write_role_arn: str = "arn:aws:iam::123456789012:role/write",
    location: dict[str, object] | None = None,
    expected_eligible: bool,
    expected_role: str,
    expected_mode: str,
) -> None:
    result = resolve_sccache_eligibility(
        active=active,
        event_name=event_name,
        github_ref=github_ref,
        read_role_arn=read_role_arn,
        write_role_arn=write_role_arn,
        location=LOCATION if location is None else location,
    )
    actual = (result.eligible, result.role_arn, result.cache_mode)
    expected = (expected_eligible, expected_role, expected_mode)
    if actual != expected:
        raise AssertionError(f"{label}: expected {expected!r}, got {actual!r}")


def main() -> int:
    read_role = "arn:aws:iam::123456789012:role/read"
    write_role = "arn:aws:iam::123456789012:role/write"
    assert_case(
        "main push may write",
        event_name="push",
        github_ref="refs/heads/main",
        expected_eligible=True,
        expected_role=write_role,
        expected_mode="read_write",
    )
    assert_case(
        "main dispatch may write",
        event_name="workflow_dispatch",
        github_ref="refs/heads/main",
        expected_eligible=True,
        expected_role=write_role,
        expected_mode="read_write",
    )
    assert_case(
        "pull request reads only",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        expected_eligible=True,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "merge group reads only",
        event_name="merge_group",
        github_ref="refs/heads/gh-readonly-queue/main/pr-1302",
        expected_eligible=True,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "schedule reads only",
        event_name="schedule",
        github_ref="refs/heads/main",
        expected_eligible=True,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "tag push gets no role",
        event_name="push",
        github_ref="refs/tags/v1.2.3",
        expected_eligible=False,
        expected_role="",
        expected_mode="none",
    )
    assert_case(
        "inactive compiles without cache",
        active=False,
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "missing read role fails closed",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        read_role_arn="",
        expected_eligible=False,
        expected_role="",
        expected_mode="read_only",
    )
    assert_case(
        "invalid prefix fails closed",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        location={**LOCATION, "key_prefix": "sccache/bolt-v2"},
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
    )
    assert_case(
        "location newlines are rejected",
        event_name="pull_request",
        github_ref="refs/pull/1302/merge",
        location={**LOCATION, "bucket": "bucket\nSCCACHE_REGION=evil"},
        expected_eligible=False,
        expected_role=read_role,
        expected_mode="read_only",
    )
    strict = resolve_sccache_eligibility(
        active=True,
        event_name="workflow_dispatch",
        github_ref="refs/heads/main",
        read_role_arn=read_role,
        write_role_arn=write_role,
        location=LOCATION,
        required=True,
        operation="root-artifact",
        github_sha="a" * 40,
        expected_sha="a" * 40,
        checked_out_sha="a" * 40,
        remote_main_sha="a" * 40,
    )
    if not strict.eligible or not strict.strict_context_valid or strict.cache_mode != "read_write":
        raise AssertionError(f"exact-main root-artifact must be strictly eligible: {strict!r}")
    for label, overrides in (
        ("wrong operation", {"operation": "root-seed"}),
        ("wrong ref", {"github_ref": "refs/heads/feature"}),
        ("wrong checked-out SHA", {"checked_out_sha": "b" * 40}),
        ("malformed remote SHA", {"remote_main_sha": "not-a-sha"}),
    ):
        values = {
            "event_name": "workflow_dispatch",
            "github_ref": "refs/heads/main",
            "operation": "root-artifact",
            "github_sha": "a" * 40,
            "expected_sha": "a" * 40,
            "checked_out_sha": "a" * 40,
            "remote_main_sha": "a" * 40,
        }
        values.update(overrides)
        rejected = resolve_sccache_eligibility(
            active=True,
            read_role_arn=read_role,
            write_role_arn=write_role,
            location=LOCATION,
            required=True,
            **values,
        )
        if rejected.eligible or rejected.strict_context_valid:
            raise AssertionError(f"strict eligibility must reject {label}: {rejected!r}")

    parsed = parse_sccache_config(CONFIG_TEXT, label="committed config")
    if parsed.asset_target != "aarch64-unknown-linux-musl":
        raise AssertionError("committed sccache asset target is not the published musl target")
    if parsed.executable_sha256 != "8df5d557b50aa19c1c818b1a6465454a9dd807917af678f3feae11ee5c9dbe27":
        raise AssertionError("committed sccache executable digest is not the approved installed-byte value")
    invalid_configs = (
        ("missing installer", CONFIG_TEXT.split("\n[installer]", 1)[0]),
        ("unknown field", CONFIG_TEXT + "\nunknown = true\n"),
        ("duplicate config", CONFIG_TEXT + "\n[location]\nbucket = \"duplicate\"\n"),
    )
    for label, text in invalid_configs:
        try:
            parse_sccache_config(text, label=label)
        except SccacheConfigError:
            pass
        else:
            raise AssertionError(f"{label} must fail strict sccache config parsing")
    assert_strict_and_legacy_behavior()
    print("OK: sccache eligibility self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
