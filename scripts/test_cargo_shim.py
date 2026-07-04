#!/usr/bin/env python3
"""Behavior tests for the repo-policy cargo PATH shim."""

import os
import plistlib
import subprocess
import sys
from importlib.machinery import SourceFileLoader
from importlib.util import module_from_spec, spec_from_loader
from pathlib import Path

import pytest
from test_fixtures import write_executable, write_policy

ROOT = Path(__file__).resolve().parents[1]
SHIM = ROOT / "scripts" / "cargo-shim"
INSTALLER = ROOT / "scripts" / "install-cargo-shim"

POLICY = """\
schema_version = 2
project_id = "bolt-v2"

[local_compile_policy]
enabled = true
allowed_ci_env = "GITHUB_ACTIONS"
break_glass_env = "BOLT_ALLOW_LOCAL_RUST"
refused_cargo_subcommands = ["build", "check", "clippy", "nextest", "run", "t", "test", "zigbuild"]
"""

REFUSAL_LINES = [
    "Local compile-heavy Rust is disabled for agent sessions in this repo.",
    "Use this repo's non-compile local checks, then commit/push and use its remote verification workflow.",
    "Human operator break-glass: BOLT_ALLOW_LOCAL_RUST=1 cargo <cmd>",
    "Policy: ci/rust-verification.toml [local_compile_policy]",
]


def _init_repo(path: Path, policy: str = POLICY) -> None:
    write_policy(path, policy_text=policy, write_justfile=False)
    subprocess.run(["git", "init", "-q"], cwd=path, check=True)


def _fake_real_cargo(tmp_path: Path) -> Path:
    real = tmp_path / "real-cargo"
    write_executable(
        real,
        "#!/usr/bin/env sh\n"
        "echo real-cargo \"$@\"\n",
    )
    return real


def _load_shim_module():
    loader = SourceFileLoader("cargo_shim_under_test", str(SHIM))
    spec = spec_from_loader(loader.name, loader)
    assert spec is not None
    module = module_from_spec(spec)
    loader.exec_module(module)
    return module


def _load_installer_module():
    loader = SourceFileLoader("cargo_shim_installer_under_test", str(INSTALLER))
    spec = spec_from_loader(loader.name, loader)
    assert spec is not None
    module = module_from_spec(spec)
    loader.exec_module(module)
    return module


def _run_cargo(repo: Path, real_cargo: Path, *args: str, extra_env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env.pop("GITHUB_ACTIONS", None)
    env["BOLT_CARGO_SHIM_REAL_CARGO"] = str(real_cargo)
    env["AGENT_SECRET_SENTINEL"] = "super-secret-value"
    if extra_env:
        env.update(extra_env)
    return subprocess.run(
        [sys.executable, str(SHIM), *args],
        cwd=repo,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def test_policy_refused_subcommand_is_blocked_without_spawning_real_cargo(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "test")

    assert result.returncode != 0
    combined = result.stdout + result.stderr
    for line in REFUSAL_LINES:
        assert line in combined
    assert "real-cargo test" not in combined
    assert "super-secret-value" not in combined


def test_no_value_global_flags_do_not_hide_refused_subcommand(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "--frozen", "test")

    assert result.returncode != 0
    combined = result.stdout + result.stderr
    assert "Local compile-heavy Rust is disabled" in combined
    assert "real-cargo --frozen test" not in combined


def test_value_taking_short_jobs_flag_does_not_hide_refused_subcommand(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "-j", "4", "test")

    assert result.returncode != 0
    combined = result.stdout + result.stderr
    assert "Local compile-heavy Rust is disabled" in combined
    assert "real-cargo -j 4 test" not in combined


def test_manifest_path_flag_does_not_hide_refused_subcommand(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "--manifest-path", "/tmp/Cargo.toml", "test")

    assert result.returncode != 0
    combined = result.stdout + result.stderr
    assert "Local compile-heavy Rust is disabled" in combined
    assert "real-cargo --manifest-path /tmp/Cargo.toml test" not in combined


@pytest.mark.parametrize(
    "args, real_cargo_line",
    [
        (("--target", "x86_64-unknown-linux-gnu", "test"), "real-cargo --target x86_64-unknown-linux-gnu test"),
        (("--target=x86_64-unknown-linux-gnu", "test"), "real-cargo --target=x86_64-unknown-linux-gnu test"),
        (("--profile", "dev", "test"), "real-cargo --profile dev test"),
        (("--profile=dev", "test"), "real-cargo --profile=dev test"),
    ],
)
def test_target_and_profile_flags_do_not_hide_refused_subcommand(tmp_path, args, real_cargo_line):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, *args)

    assert result.returncode != 0
    combined = result.stdout + result.stderr
    assert "Local compile-heavy Rust is disabled" in combined
    assert real_cargo_line not in combined


def test_argument_separator_does_not_hide_refused_subcommand(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "--", "test")

    assert result.returncode != 0
    combined = result.stdout + result.stderr
    assert "Local compile-heavy Rust is disabled" in combined
    assert "real-cargo -- test" not in combined


def test_policy_alias_subcommand_is_blocked_when_listed(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "t")

    assert result.returncode != 0
    combined = result.stdout + result.stderr
    assert "Local compile-heavy Rust is disabled" in combined
    assert "real-cargo t" not in combined


def test_toolchain_selector_does_not_hide_refused_subcommand(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "+1.95.0", "test")

    assert result.returncode != 0
    combined = result.stdout + result.stderr
    assert "Local compile-heavy Rust is disabled" in combined
    assert "real-cargo +1.95.0 test" not in combined


def test_allowed_subcommand_execs_real_cargo(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "fmt", "--check")

    assert result.returncode == 0
    assert result.stdout.strip() == "real-cargo fmt --check"
    assert result.stderr == ""


def test_break_glass_execs_real_cargo_for_refused_subcommand(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "check", extra_env={"BOLT_ALLOW_LOCAL_RUST": "1"})

    assert result.returncode == 0
    assert result.stdout.strip() == "real-cargo check"
    assert result.stderr == ""


def test_ci_env_execs_real_cargo_for_refused_subcommand(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "check", extra_env={"GITHUB_ACTIONS": "true"})

    assert result.returncode == 0
    assert result.stdout.strip() == "real-cargo check"
    assert result.stderr == ""


def test_truthy_ci_env_value_does_not_bypass_policy(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "check", extra_env={"GITHUB_ACTIONS": "1"})

    assert result.returncode != 0
    combined = result.stdout + result.stderr
    assert "Local compile-heavy Rust is disabled" in combined
    assert "real-cargo check" not in combined


def test_enabled_false_execs_real_cargo_for_refused_subcommand(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(
        repo,
        """\
[local_compile_policy]
enabled = false
refused_cargo_subcommands = ["check"]
""",
    )
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "check")

    assert result.returncode == 0
    assert result.stdout.strip() == "real-cargo check"
    assert result.stderr == ""


def test_missing_enabled_warns_and_execs_real_cargo(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(
        repo,
        """\
[local_compile_policy]
refused_cargo_subcommands = ["check"]
""",
    )
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "check")

    assert result.returncode == 0
    assert result.stdout.strip() == "real-cargo check"
    assert "[local_compile_policy].enabled is missing; local cargo guard disabled" in result.stderr


def test_empty_local_compile_policy_warns_and_execs_real_cargo(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo, "[local_compile_policy]\n")
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "check")

    assert result.returncode == 0
    assert result.stdout.strip() == "real-cargo check"
    assert "[local_compile_policy].enabled is missing; local cargo guard disabled" in result.stderr


def test_fallback_policy_parser_preserves_empty_local_compile_policy(tmp_path):
    policy_path = tmp_path / "rust-verification.toml"
    policy_path.write_text("[local_compile_policy]\n", encoding="utf-8")
    shim = _load_shim_module()
    shim.tomllib = None

    policy = shim.load_policy(policy_path)

    assert policy == {}


def test_dynamic_break_glass_env_appears_in_refusal(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(
        repo,
        """\
[local_compile_policy]
enabled = true
break_glass_env = "CUSTOM_ALLOW_RUST"
refused_cargo_subcommands = ["check"]
""",
    )
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "check")

    assert result.returncode != 0
    assert "Human operator break-glass: CUSTOM_ALLOW_RUST=1 cargo <cmd>" in result.stderr
    assert "BOLT_ALLOW_LOCAL_RUST=1 cargo <cmd>" not in result.stderr
    assert "real-cargo check" not in result.stdout + result.stderr


def test_policy_parse_error_fails_closed(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo, "[local_compile_policy\n")
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "fmt")

    assert result.returncode != 0
    assert "cannot load or parse ci/rust-verification.toml [local_compile_policy]" in result.stderr
    assert "real-cargo fmt" not in result.stdout + result.stderr


def test_existing_policy_file_without_local_compile_policy_fails_closed(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(
        repo,
        """\
[remote_verification]
poll_interval_seconds = 15
""",
    )
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "fmt")

    assert result.returncode != 0
    assert "local_compile_policy table is required" in result.stderr
    assert "real-cargo fmt" not in result.stdout + result.stderr


def test_empty_refused_subcommands_fails_closed(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(
        repo,
        """\
[local_compile_policy]
enabled = true
refused_cargo_subcommands = []
""",
    )
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "fmt")

    assert result.returncode != 0
    assert "refused_cargo_subcommands must be a non-empty string list" in result.stderr
    assert "real-cargo fmt" not in result.stdout + result.stderr


def test_fallback_policy_parser_accepts_multiline_refused_subcommands(tmp_path):
    policy_path = tmp_path / "rust-verification.toml"
    policy_path.write_text(
        """\
schema_version = 2

[local_compile_policy]
enabled = true
allowed_ci_env = "GITHUB_ACTIONS"
break_glass_env = "BOLT_ALLOW_LOCAL_RUST"
refused_cargo_subcommands = [
    "build",
    "check",
    "test",
]
""",
        encoding="utf-8",
    )
    shim = _load_shim_module()
    shim.tomllib = None

    policy = shim.load_policy(policy_path)

    assert policy["refused_cargo_subcommands"] == ["build", "check", "test"]


def test_fallback_policy_parser_rejects_malformed_section_header(tmp_path):
    policy_path = tmp_path / "rust-verification.toml"
    policy_path.write_text("[local_compile_policy\n", encoding="utf-8")
    shim = _load_shim_module()
    shim.tomllib = None

    with pytest.raises(ValueError, match="invalid TOML table header"):
        shim.load_policy(policy_path)


def test_fallback_policy_parser_preserves_hash_inside_quoted_strings(tmp_path):
    policy_path = tmp_path / "rust-verification.toml"
    policy_path.write_text(
        """\
[local_compile_policy]
enabled = true
break_glass_env = "BOLT#RUST"
refused_cargo_subcommands = ["test"]
""",
        encoding="utf-8",
    )
    shim = _load_shim_module()
    shim.tomllib = None

    policy = shim.load_policy(policy_path)

    assert policy["break_glass_env"] == "BOLT#RUST"


def test_resolve_real_cargo_falls_back_to_path_when_home_unavailable(tmp_path, monkeypatch):
    shim = _load_shim_module()
    real_dir = tmp_path / "bin"
    real_dir.mkdir()
    real = real_dir / "cargo"
    write_executable(real, "#!/usr/bin/env sh\n")

    def fail_home():
        raise RuntimeError("home unavailable")

    monkeypatch.delenv("BOLT_CARGO_SHIM_REAL_CARGO", raising=False)
    monkeypatch.setenv("PATH", str(real_dir))
    monkeypatch.setattr(shim.Path, "home", staticmethod(fail_home))

    assert shim.resolve_real_cargo() == real


def test_policy_read_error_fails_closed_without_traceback(tmp_path, monkeypatch, capsys):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    shim = _load_shim_module()

    def fail_load_policy(_policy_path):
        raise OSError("permission denied")

    monkeypatch.chdir(repo)
    monkeypatch.setattr(shim, "load_policy", fail_load_policy)

    result = shim.main(["fmt"])

    captured = capsys.readouterr()
    assert result == 101
    assert "cannot load or parse ci/rust-verification.toml [local_compile_policy]" in captured.err
    assert "Traceback" not in captured.err


def test_shim_runs_under_system_python_without_tomllib(tmp_path):
    system_python = Path("/usr/bin/python3")
    if not system_python.exists():
        pytest.skip("/usr/bin/python3 not available on this host")
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(repo)
    real = _fake_real_cargo(tmp_path)
    env = os.environ.copy()
    env.pop("GITHUB_ACTIONS", None)
    env["BOLT_CARGO_SHIM_REAL_CARGO"] = str(real)

    result = subprocess.run(
        [str(system_python), str(SHIM), "test"],
        cwd=repo,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )

    assert result.returncode != 0
    assert "Local compile-heavy Rust is disabled" in result.stdout + result.stderr
    assert "Python runtime does not provide tomllib" not in result.stderr


def test_real_cargo_recursion_guard_refuses_self_exec(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    _init_repo(
        repo,
        """\
[local_compile_policy]
enabled = false
refused_cargo_subcommands = ["check"]
""",
    )

    result = _run_cargo(repo, SHIM, "fmt")

    assert result.returncode == 127
    assert "real cargo resolved to shim; refusing recursion" in result.stderr


def test_no_policy_repo_is_transparent(tmp_path):
    repo = tmp_path / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q"], cwd=repo, check=True)
    real = _fake_real_cargo(tmp_path)

    result = _run_cargo(repo, real, "build")

    assert result.returncode == 0
    assert result.stdout.strip() == "real-cargo build"


def test_installer_is_idempotent_and_prepends_zshenv_path(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    legacy = "\n".join(
        [
            '. "$HOME/.cargo/env"',
            "_rust_verification_env_unset_args() {",
            "  true",
            "}",
            "",
            "cargo() {",
            "  echo stale function",
            "}",
            "",
            "export STILL_PRESENT=1",
            "",
        ]
    )
    (home / ".zshenv").write_text(legacy, encoding="utf-8")
    install_dir = tmp_path / "shim-bin"
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["BOLT_CARGO_SHIM_DIR"] = str(install_dir)

    for _ in range(2):
        result = subprocess.run(
            [sys.executable, str(INSTALLER)],
            cwd=ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert result.returncode == 0, result.stderr

    installed = install_dir / "cargo"
    assert installed.exists()
    assert os.access(installed, os.X_OK)

    zshenv = home / ".zshenv"
    text = zshenv.read_text(encoding="utf-8")
    assert text.count("# BEGIN bolt cargo guard") == 1
    assert "cargo() {" not in text
    assert "_rust_verification_env_unset_args() {" not in text
    assert "export STILL_PRESENT=1" in text
    assert "unfunction cargo" in text
    assert f'export BOLT_CARGO_SHIM_DIR="{install_dir}"' in text
    assert 'export PATH="$BOLT_CARGO_SHIM_DIR:$PATH"' in text


def test_installer_prepends_no_mistakes_launch_agent_path(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    launch_agents = home / "Library" / "LaunchAgents"
    launch_agents.mkdir(parents=True)
    plist_path = launch_agents / "com.kunchenguid.no-mistakes.daemon.test.plist"
    original_path = f"{home}/.local/bin:{home}/.cargo/bin:/usr/bin:/bin"
    plist_payload = {
        "Label": "com.kunchenguid.no-mistakes.daemon.test",
        "ProgramArguments": [str(home / ".local/bin/no-mistakes"), "daemon", "run"],
        "EnvironmentVariables": {
            "HOME": str(home),
            "PATH": original_path,
        },
    }
    plist_path.write_bytes(plistlib.dumps(plist_payload))
    install_dir = tmp_path / "shim-bin"
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["BOLT_CARGO_SHIM_DIR"] = str(install_dir)

    for _ in range(2):
        result = subprocess.run(
            [sys.executable, str(INSTALLER)],
            cwd=ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        assert result.returncode == 0, result.stderr
        assert f"Updated no-mistakes LaunchAgent PATH: {plist_path}" in result.stdout

    updated = plistlib.loads(plist_path.read_bytes())
    path_entries = updated["EnvironmentVariables"]["PATH"].split(os.pathsep)
    assert path_entries[0] == str(install_dir)
    assert path_entries.count(str(install_dir)) == 1
    assert os.pathsep.join(path_entries[1:]) == original_path


@pytest.mark.parametrize(
    ("env_vars", "expected_env_vars"),
    [
        ({"HOME": "placeholder"}, {"HOME": "placeholder"}),
        ({"HOME": "placeholder", "PATH": ""}, {"HOME": "placeholder", "PATH": ""}),
    ],
)
def test_installer_skips_no_mistakes_launch_agent_without_path(tmp_path, env_vars, expected_env_vars):
    home = tmp_path / "home"
    home.mkdir()
    launch_agents = home / "Library" / "LaunchAgents"
    launch_agents.mkdir(parents=True)
    plist_path = launch_agents / "com.kunchenguid.no-mistakes.daemon.test.plist"
    plist_payload = {
        "Label": "com.kunchenguid.no-mistakes.daemon.test",
        "ProgramArguments": [str(home / ".local/bin/no-mistakes"), "daemon", "run"],
        "EnvironmentVariables": env_vars,
    }
    plist_path.write_bytes(plistlib.dumps(plist_payload))
    install_dir = tmp_path / "shim-bin"
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["BOLT_CARGO_SHIM_DIR"] = str(install_dir)

    result = subprocess.run(
        [sys.executable, str(INSTALLER)],
        cwd=ROOT,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    assert f"Skipping no-mistakes LaunchAgent PATH update for {plist_path}" in result.stderr
    assert "Updated no-mistakes LaunchAgent PATH" not in result.stdout
    updated = plistlib.loads(plist_path.read_bytes())
    assert updated["EnvironmentVariables"] == expected_env_vars


def test_installer_creates_zshenv_when_missing(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    install_dir = tmp_path / "shim-bin"
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["BOLT_CARGO_SHIM_DIR"] = str(install_dir)

    result = subprocess.run(
        [sys.executable, str(INSTALLER)],
        cwd=ROOT,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    text = (home / ".zshenv").read_text(encoding="utf-8")
    assert text.count("# BEGIN bolt cargo guard") == 1
    assert f'export BOLT_CARGO_SHIM_DIR="{install_dir}"' in text


def test_zshenv_path_falls_back_when_home_unavailable(tmp_path, monkeypatch):
    installer = _load_installer_module()
    home = tmp_path / "home"
    home.mkdir()

    def fail_home():
        raise RuntimeError("home unavailable")

    monkeypatch.delenv("ZDOTDIR", raising=False)
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setattr(installer.Path, "home", staticmethod(fail_home))

    assert installer.zshenv_path() == home / ".zshenv"


def test_installer_updates_symlink_target_without_replacing_symlink(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    dotfiles = tmp_path / "dotfiles"
    dotfiles.mkdir()
    target = dotfiles / "zshenv"
    target.write_text("export STILL_PRESENT=1\n", encoding="utf-8")
    (home / ".zshenv").symlink_to(target)
    install_dir = tmp_path / "shim-bin"
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["BOLT_CARGO_SHIM_DIR"] = str(install_dir)

    result = subprocess.run(
        [sys.executable, str(INSTALLER)],
        cwd=ROOT,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    assert (home / ".zshenv").is_symlink()
    assert (home / ".zshenv").resolve() == target
    text = target.read_text(encoding="utf-8")
    assert "export STILL_PRESENT=1" in text
    assert text.count("# BEGIN bolt cargo guard") == 1


def test_atomic_write_uses_default_mode_if_target_disappears_before_stat(tmp_path, monkeypatch):
    installer = _load_installer_module()
    target = tmp_path / "target"
    target.write_bytes(b"old")
    real_exists = installer.Path.exists
    real_stat = installer.Path.stat

    def racing_exists(path):
        if path == target:
            return True
        return real_exists(path)

    def racing_stat(path):
        if path == target:
            raise FileNotFoundError(str(path))
        return real_stat(path)

    monkeypatch.setattr(installer.Path, "exists", racing_exists)
    monkeypatch.setattr(installer.Path, "stat", racing_stat)

    installer.atomic_write_bytes(target, b"new")

    assert target.read_bytes() == b"new"


def test_atomic_write_preserves_original_error_when_cleanup_fails(tmp_path, monkeypatch):
    installer = _load_installer_module()
    target = tmp_path / "target"
    target.write_bytes(b"old")

    def fail_replace(_src, _dst):
        raise OSError("replace failed")

    def fail_unlink(_path):
        raise PermissionError("cleanup denied")

    monkeypatch.setattr(installer.os, "replace", fail_replace)
    monkeypatch.setattr(installer.Path, "unlink", fail_unlink)

    with pytest.raises(OSError, match="replace failed"):
        installer.atomic_write_bytes(target, b"new")

    assert target.read_bytes() == b"old"


def test_update_zshenv_treats_disappearing_file_as_missing(tmp_path, monkeypatch):
    installer = _load_installer_module()
    zshenv = tmp_path / ".zshenv"
    zshenv.write_text("export OLD=1\n", encoding="utf-8")
    install_dir = tmp_path / "shim-bin"
    real_exists = installer.Path.exists
    real_read_text = installer.Path.read_text

    def racing_exists(path):
        if path == zshenv:
            return True
        return real_exists(path)

    def racing_read_text(path, *args, **kwargs):
        if path == zshenv:
            raise FileNotFoundError(str(path))
        return real_read_text(path, *args, **kwargs)

    monkeypatch.setattr(installer.Path, "exists", racing_exists)
    monkeypatch.setattr(installer.Path, "read_text", racing_read_text)

    installer.update_zshenv(zshenv, install_dir)

    text = real_read_text(zshenv, encoding="utf-8")
    assert text.count("# BEGIN bolt cargo guard") == 1
    assert "export OLD=1" not in text


def test_update_zshenv_preserves_original_error_when_cleanup_fails(tmp_path, monkeypatch):
    installer = _load_installer_module()
    zshenv = tmp_path / ".zshenv"
    original = "export STILL_PRESENT=1\n"
    zshenv.write_text(original, encoding="utf-8")
    install_dir = tmp_path / "shim-bin"

    def fail_replace(_src, _dst):
        raise OSError("replace failed")

    def fail_unlink(_path):
        raise PermissionError("cleanup denied")

    monkeypatch.setattr(installer.os, "replace", fail_replace)
    monkeypatch.setattr(installer.Path, "unlink", fail_unlink)

    with pytest.raises(OSError, match="replace failed"):
        installer.update_zshenv(zshenv, install_dir)

    assert zshenv.read_text(encoding="utf-8") == original


def test_update_zshenv_preserves_original_when_replace_fails(tmp_path, monkeypatch):
    installer = _load_installer_module()
    home = tmp_path / "home"
    home.mkdir()
    zshenv = home / ".zshenv"
    original = "export STILL_PRESENT=1\n"
    zshenv.write_text(original, encoding="utf-8")
    install_dir = tmp_path / "shim-bin"

    def fail_replace(_src, _dst):
        raise OSError("replace failed")

    monkeypatch.setattr(installer.os, "replace", fail_replace)

    with pytest.raises(OSError, match="replace failed"):
        installer.update_zshenv(zshenv, install_dir)

    assert zshenv.read_text(encoding="utf-8") == original
    assert list(home.glob(".zshenv.tmp-*")) == []


def test_no_mistakes_launch_agent_preserves_original_when_replace_fails(tmp_path, monkeypatch):
    installer = _load_installer_module()
    home = tmp_path / "home"
    launch_agents = home / "Library" / "LaunchAgents"
    launch_agents.mkdir(parents=True)
    plist_path = launch_agents / "com.kunchenguid.no-mistakes.daemon.test.plist"
    plist_payload = {
        "Label": "com.kunchenguid.no-mistakes.daemon.test",
        "ProgramArguments": [str(home / ".local/bin/no-mistakes"), "daemon", "run"],
        "EnvironmentVariables": {
            "PATH": "/usr/bin:/bin",
        },
    }
    plist_path.write_bytes(plistlib.dumps(plist_payload))
    original = plist_path.read_bytes()
    install_dir = tmp_path / "shim-bin"

    def fail_replace(_src, _dst):
        raise OSError("replace failed")

    monkeypatch.setattr(installer.os, "replace", fail_replace)

    with pytest.raises(ValueError, match=r"Failed to update no-mistakes LaunchAgent PATH .* replace failed"):
        installer.update_no_mistakes_launch_agents(home, install_dir)

    assert plist_path.read_bytes() == original
    assert list(launch_agents.glob(f".{plist_path.name}.tmp-*")) == []


def test_installer_reports_unavailable_home_without_traceback(tmp_path, monkeypatch, capsys):
    installer = _load_installer_module()

    def fail_home():
        raise RuntimeError("home unavailable")

    monkeypatch.setenv("BOLT_CARGO_SHIM_DIR", str(tmp_path / "shim-bin"))
    monkeypatch.setattr(installer.Path, "home", staticmethod(fail_home))

    result = installer.main()

    captured = capsys.readouterr()
    assert result == 1
    assert "Failed to resolve home directory: home unavailable" in captured.err
    assert "Traceback" not in captured.err


def test_installer_reports_install_failure_without_traceback(tmp_path, monkeypatch, capsys):
    installer = _load_installer_module()

    def fail_install(_source, _install_dir):
        raise PermissionError("install denied")

    monkeypatch.setenv("BOLT_CARGO_SHIM_DIR", str(tmp_path / "shim-bin"))
    monkeypatch.setattr(installer, "install_shim", fail_install)

    result = installer.main()

    captured = capsys.readouterr()
    assert result == 1
    assert "Failed to install cargo shim: install denied" in captured.err
    assert "Traceback" not in captured.err


def test_installer_reports_zshenv_update_failure_without_traceback(tmp_path, monkeypatch, capsys):
    installer = _load_installer_module()
    home = tmp_path / "home"
    home.mkdir()
    install_dir = tmp_path / "shim-bin"

    def fake_install(_source, _install_dir):
        return install_dir / "cargo"

    def fail_update(_zshenv, _install_dir):
        raise PermissionError("zshenv denied")

    monkeypatch.setenv("BOLT_CARGO_SHIM_DIR", str(install_dir))
    monkeypatch.setattr(installer.Path, "home", staticmethod(lambda: home))
    monkeypatch.setattr(installer, "install_shim", fake_install)
    monkeypatch.setattr(installer, "update_zshenv", fail_update)

    result = installer.main()

    captured = capsys.readouterr()
    assert result == 1
    assert "Failed to update zsh startup: zshenv denied" in captured.err
    assert "Traceback" not in captured.err


@pytest.mark.parametrize(
    "plist_payload",
    [
        b"not a plist",
        plistlib.dumps(
            {
                "Label": "com.kunchenguid.no-mistakes.daemon.test",
                "EnvironmentVariables": ["PATH=/usr/bin:/bin"],
            }
        ),
        plistlib.dumps(
            {
                "Label": "com.kunchenguid.no-mistakes.daemon.test",
                "EnvironmentVariables": {
                    "PATH": ["/usr/bin", "/bin"],
                },
            }
        ),
    ],
)
def test_installer_reports_invalid_no_mistakes_launch_agent_without_traceback(tmp_path, plist_payload):
    home = tmp_path / "home"
    home.mkdir()
    launch_agents = home / "Library" / "LaunchAgents"
    launch_agents.mkdir(parents=True)
    plist_path = launch_agents / "com.kunchenguid.no-mistakes.daemon.test.plist"
    plist_path.write_bytes(plist_payload)
    install_dir = tmp_path / "shim-bin"
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["BOLT_CARGO_SHIM_DIR"] = str(install_dir)

    result = subprocess.run(
        [sys.executable, str(INSTALLER)],
        cwd=ROOT,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )

    assert result.returncode == 1
    assert f"Failed to update no-mistakes LaunchAgent PATH for {plist_path}:" in result.stderr
    assert "Traceback" not in result.stderr


def test_installer_preserves_zshenv_after_malformed_legacy_block(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    (home / ".zshenv").write_text(
        "\n".join(
            [
                '. "$HOME/.cargo/env"',
                "_rust_verification_env_unset_args() {",
                "  true",
                "",
                "export AFTER_MALFORMED_BLOCK=1",
                "",
            ]
        ),
        encoding="utf-8",
    )
    install_dir = tmp_path / "shim-bin"
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["BOLT_CARGO_SHIM_DIR"] = str(install_dir)

    result = subprocess.run(
        [sys.executable, str(INSTALLER)],
        cwd=ROOT,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    text = (home / ".zshenv").read_text(encoding="utf-8")
    assert "_rust_verification_env_unset_args() {" in text
    assert "export AFTER_MALFORMED_BLOCK=1" in text
    assert text.count("# BEGIN bolt cargo guard") == 1


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(pytest.main([__file__]))
