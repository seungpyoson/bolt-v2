"""Shared fixtures for repo-local Python self-tests."""

from __future__ import annotations

import importlib.util
import pathlib
import textwrap


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
RUST_VERIFICATION_SCRIPT = REPO_ROOT / "scripts" / "rust_verification.py"


def load_owner_module() -> object:
    # Module-state hygiene: this returns a fresh module object. Tests that mutate
    # module globals must keep the mutation local or restore it before returning.
    spec = importlib.util.spec_from_file_location("rust_verification_under_test", RUST_VERIFICATION_SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError("unable to load rust_verification.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write_executable(path: pathlib.Path, body: str) -> None:
    path.write_text(body, encoding="utf-8")
    path.chmod(0o755)


def rust_verification_policy_text(*, target_namespace: str = "bolt-v2") -> str:
    return textwrap.dedent(
        f"""\
        schema_version = 2
        project_id = "bolt-v2"
        target_namespace = "{target_namespace}"

        [local_compile_policy]
        enabled = true
        allowed_ci_env = "GITHUB_ACTIONS"
        break_glass_env = "BOLT_ALLOW_LOCAL_RUST"
        refused_managed_commands = ["test", "clippy", "build"]
        refused_cargo_subcommands = ["b", "bench", "build", "c", "check", "clippy", "d", "doc", "fetch", "install", "nextest", "r", "run", "rustc", "t", "test", "zigbuild"]

        [local_lane_policy]
        enabled = true
        allowed_ci_env = "GITHUB_ACTIONS"
        lock_dir = "/tmp/rust-verification-lanes"
        acquire_timeout_seconds = 1800
        heartbeat_seconds = 15
        poll_interval_seconds = 1

        [commands]

        [commands.test]
        cargo_args = ["nextest", "run", "--locked"]

        [commands.clippy]
        recipe = "managed-clippy"

        [commands.build]
        recipe = "managed-build"
        artifact_layout = "cargo"
        profile = "release"
        target = "aarch64-unknown-linux-gnu"
        """
    )


def write_policy(
    repo: pathlib.Path,
    *,
    target_namespace: str = "bolt-v2",
    policy_text: str | None = None,
    write_justfile: bool = True,
) -> pathlib.Path:
    (repo / "ci").mkdir(parents=True)
    policy_path = repo / "ci" / "rust-verification.toml"
    text = rust_verification_policy_text(target_namespace=target_namespace) if policy_text is None else policy_text
    policy_path.write_text(text, encoding="utf-8")
    if write_justfile:
        (repo / "justfile").write_text("", encoding="utf-8")
    return policy_path


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    print("OK: shared test fixtures import-only module.")
