#!/usr/bin/env python3
"""Self-tests for the repo-local Rust verification owner."""

from __future__ import annotations

import os
import json
import pathlib
import subprocess
import sys
import tempfile
import textwrap


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / "scripts" / "rust_verification.py"


def run_owner(args: list[str], *, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), *args],
        cwd=REPO_ROOT,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def write_executable(path: pathlib.Path, body: str) -> None:
    path.write_text(body, encoding="utf-8")
    path.chmod(0o755)


def write_policy(repo: pathlib.Path) -> None:
    (repo / "ci").mkdir()
    (repo / "ci" / "rust-verification.toml").write_text(
        textwrap.dedent(
            """\
            schema_version = 2
            project_id = "bolt-v2"
            target_namespace = "bolt-v2"

            [local_compile_policy]
            enabled = true
            allowed_ci_env = "GITHUB_ACTIONS"
            break_glass_env = "BOLT_ALLOW_LOCAL_RUST"
            refused_managed_commands = ["test", "clippy", "build"]
            refused_cargo_subcommands = ["bench", "build", "check", "clippy", "doc", "fetch", "install", "nextest", "run", "rustc", "test", "zigbuild"]

            [commands]

            [commands.test]
            recipe = "managed-test"

            [commands.clippy]
            recipe = "managed-clippy"

            [commands.build]
            recipe = "managed-build"
            artifact_layout = "cargo"
            profile = "release"
            target = "aarch64-unknown-linux-gnu"
            """
        ),
        encoding="utf-8",
    )
    (repo / "justfile").write_text("", encoding="utf-8")


def parse_log(path: pathlib.Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        key, value = line.split("=", 1)
        values[key] = value
    return values


def same_path(left: str, right: pathlib.Path) -> bool:
    return pathlib.Path(left).resolve() == right.resolve()


def assert_repo_local_owner_contract() -> None:
    if not SCRIPT.exists():
        raise AssertionError(f"missing repo-local owner script: {SCRIPT}")

    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / "repo"
        repo.mkdir()
        write_policy(repo)

        bin_dir = tmp_path / "bin"
        bin_dir.mkdir()
        cargo_log = tmp_path / "cargo.log"
        just_log = tmp_path / "just.log"
        write_executable(
            bin_dir / "cargo",
            f"""#!/usr/bin/env bash
printf 'cwd=%s\\n' "$PWD" > {cargo_log}
printf 'target=%s\\n' "$CARGO_TARGET_DIR" >> {cargo_log}
printf 'args=%s\\n' "$*" >> {cargo_log}
""",
        )
        write_executable(
            bin_dir / "just",
            f"""#!/usr/bin/env bash
printf 'cwd=%s\\n' "$PWD" > {just_log}
printf 'target=%s\\n' "$CARGO_TARGET_DIR" >> {just_log}
printf 'args=%s\\n' "$*" >> {just_log}
""",
        )

        root_base = tmp_path / "rust-root"
        env = os.environ.copy()
        env.pop("GITHUB_ACTIONS", None)
        env.pop("BOLT_ALLOW_LOCAL_RUST", None)
        env["PATH"] = f"{bin_dir}{os.pathsep}{env['PATH']}"
        env["RUST_VERIFICATION_ROOT_BASE"] = str(root_base)

        target_dir = root_base / "bolt-v2" / "target"
        result = run_owner(["target-dir", "--repo", str(repo)], env=env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        if result.stdout.strip() != str(target_dir):
            raise AssertionError((result.stdout, target_dir))
        if not target_dir.is_dir():
            raise AssertionError(f"target-dir did not create {target_dir}")

        binary = target_dir / "aarch64-unknown-linux-gnu" / "release" / "bolt-v2"
        binary.parent.mkdir(parents=True)
        binary.write_text("binary", encoding="utf-8")
        result = run_owner(["binary-path", "--repo", str(repo), "--bin", "bolt-v2"], env=env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        if result.stdout.strip() != str(binary):
            raise AssertionError((result.stdout, binary))

        result = run_owner(["cargo", "--repo", str(repo), "--", "fmt", "--check"], env=env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        cargo_values = parse_log(cargo_log)
        if not same_path(cargo_values["cwd"], repo) or cargo_values["target"] != str(target_dir) or cargo_values["args"] != "fmt --check":
            raise AssertionError(cargo_values)

        result = run_owner(["run", "--repo", str(repo), "build", "--flag"], env=env)
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        refusal = json.loads(result.stderr)
        if refusal.get("refusal_code") != "local_compile_disabled" or "verify-remote" not in "\n".join(
            refusal.get("next_steps", [])
        ):
            raise AssertionError(refusal)

        allowed_env = env.copy()
        allowed_env["GITHUB_ACTIONS"] = "true"
        result = run_owner(["run", "--repo", str(repo), "build", "--flag"], env=allowed_env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        just_values = parse_log(just_log)
        expected_args = f"-f {repo / 'justfile'} --working-directory {repo} -- managed-build --flag"
        if not same_path(just_values["cwd"], repo) or just_values["target"] != str(target_dir) or just_values["args"] != expected_args:
            raise AssertionError(just_values)

        result = run_owner(["validate-policy", "--repo", str(repo)], env=env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        payload = json.loads(result.stdout)
        expected_payload = {
            "build_profile": "release",
            "build_target": "aarch64-unknown-linux-gnu",
            "policy": str(repo / "ci" / "rust-verification.toml"),
            "project_id": "bolt-v2",
            "status": "ok",
        }
        if payload != expected_payload:
            raise AssertionError(payload)


def assert_system_python_contract() -> None:
    system_python = pathlib.Path("/usr/bin/python3")
    if not system_python.exists():
        return
    result = subprocess.run(
        [str(system_python), "-S", str(SCRIPT), "repo-status", "--repo", str(REPO_ROOT)],
        cwd=REPO_ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        raise AssertionError(result.stderr)
    if result.stdout.strip() != "managed":
        raise AssertionError(result.stdout)


def assert_oversized_policy_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / "repo"
        repo.mkdir()
        write_policy(repo)
        policy = repo / "ci" / "rust-verification.toml"
        policy.write_text("schema_version = 1\n" + ("# padding\n" * 140_000), encoding="utf-8")

        result = run_owner(["validate-policy", "--repo", str(repo)], env=os.environ.copy())
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if "exceeds maximum size" not in result.stderr:
            raise AssertionError(result.stderr)


def main() -> int:
    assert_repo_local_owner_contract()
    assert_system_python_contract()
    assert_oversized_policy_fails_closed()
    print("OK: Rust verification owner self-tests passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
