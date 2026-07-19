"""Shared fixtures for CI workflow hygiene analyzer tests."""
from __future__ import annotations
import contextlib
import importlib.util
import io
import json
import pathlib
import subprocess
import sys
import tempfile
from ci_test_manifest import CiTestManifest
from git_maintenance import GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG
REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
VERIFIER_PATH = REPO_ROOT / 'scripts' / 'verify_ci_workflow_hygiene.py'
GIT_AUTO_MAINTENANCE_SUPPRESSION_ARGS = tuple((arg for key, value in GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG for arg in ('-c', f'{key}={value}')))

def _read_trace2_events(trace_path: pathlib.Path) -> list[dict[str, object]]:
    if not trace_path.exists():
        raise AssertionError(f"Trace2 event log was not produced: {trace_path}")
    lines = trace_path.read_text(encoding="utf-8", errors="strict").splitlines()
    if not lines:
        raise AssertionError(f"Trace2 event log is empty: {trace_path}")
    events: list[dict[str, object]] = []
    for line_number, line in enumerate(lines, start=1):
        try:
            event = json.loads(line)
        except ValueError as exc:
            raise AssertionError(
                f"Trace2 event log has invalid JSON at line {line_number}: {trace_path}"
            ) from exc
        if not isinstance(event, dict):
            raise AssertionError(
                f"Trace2 event log line {line_number} is not an object: {trace_path}"
            )
        events.append(event)
    return events

def count_trace2_children(trace_path: pathlib.Path) -> int:
    return sum(
        event.get("event") == "child_start" for event in _read_trace2_events(trace_path)
    )

def count_trace2_maintenance_children(trace_path: pathlib.Path) -> int:
    child_argv: list[list[str]] = []
    for line_number, event in enumerate(_read_trace2_events(trace_path), start=1):
        if event.get("event") != "child_start":
            continue
        argv = event.get("argv")
        if not isinstance(argv, list) or not all(isinstance(arg, str) for arg in argv):
            raise AssertionError(
                f"Trace2 child event line {line_number} has invalid argv: {trace_path}"
            )
        child_argv.append(argv)
    return sum(
        "maintenance" in " ".join(argv) or " ".join(argv).startswith("git gc")
        for argv in child_argv
    )

def load_verifier(path: pathlib.Path=VERIFIER_PATH, module_name: str='verify_ci_workflow_hygiene'):
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise AssertionError('could not load verify_ci_workflow_hygiene.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    if hasattr(module, 'build_test_manifest'):
        module.build_test_manifest = lambda _manifest_path, _tests_root: all_standalone_live_node_manifest(module)
    return module

def load_provenance(
    path: pathlib.Path = REPO_ROOT / "scripts" / "ci_provenance.py",
    module_name: str = "ci_provenance",
):
    spec = importlib.util.spec_from_file_location(module_name, path)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load ci_provenance.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module

def repo_git_command(*args: str) -> list[str]:
    return ["git", *GIT_AUTO_MAINTENANCE_SUPPRESSION_ARGS, *args]

def run_repo_git(repo: pathlib.Path, *args: str) -> str:
    completed = subprocess.run(
        repo_git_command(*args),
        cwd=repo,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return completed.stdout

def replace_once(text: str, old: str, new: str) -> str:
    if old not in text:
        raise AssertionError(f"fixture fragment not found: {old!r}")
    return text.replace(old, new, 1)

def replace_once_after(text: str, anchor: str, old: str, new: str) -> str:
    index = text.find(anchor)
    if index == -1:
        raise AssertionError(f"fixture anchor not found: {anchor!r}")
    return text[:index] + replace_once(text[index:], old, new)

def yaml_scalar_literal(value: object) -> str:
    if value is None:
        return "null"
    if value is True:
        return "true"
    if value is False:
        return "false"
    return str(value)
BASE_ACTION = '\nname: Setup Environment\ninputs:\n  just-version:\n    required: true\n  include-deny-version:\n    required: false\n    default: "false"\n  include-nextest-version:\n    required: false\n    default: "false"\n  include-build-values:\n    required: false\n    default: "false"\n  lint-workflow-contract:\n    required: false\n    default: "false"\n  include-managed-target-dir:\n    description: Whether to resolve the managed target dir.\n    required: false\n    default: "false"\n  install-rust-linker:\n    description: Whether to install the configured Rust fast linker.\n    required: false\n    default: "false"\n  build-jobs-key:\n    required: false\n    default: ""\noutputs:\n  rust_toolchain:\n    value: ${{ steps.shared.outputs.rust_toolchain }}\n  deny_version:\n    value: ${{ steps.shared.outputs.deny_version }}\n  nextest_version:\n    value: ${{ steps.shared.outputs.nextest_version }}\n  target:\n    value: ${{ steps.shared.outputs.target }}\n  zig_version:\n    value: ${{ steps.shared.outputs.zig_version }}\n  zigbuild_version:\n    value: ${{ steps.shared.outputs.zigbuild_version }}\n  rust_verification_owner:\n    value: ${{ steps.shared.outputs.rust_verification_owner }}\n  managed_target_dir:\n    value: ${{ steps.target_dir.outputs.managed_target_dir }}\n  managed_target_dir_relative:\n    value: ${{ steps.target_dir.outputs.managed_target_dir_relative }}\n  cargo_build_jobs:\n    value: ${{ steps.shared.outputs.cargo_build_jobs }}\nruns:\n  using: composite\n  steps:\n    - name: Install just\n      uses: taiki-e/install-action@e49978b799e49ff429d162b7a30601a569ab6538 # v2.81.1\n      with:\n        tool: just@${{ inputs.just-version }}\n        fallback: none\n    - name: Lint workflow contract\n      if: ${{ inputs.lint-workflow-contract == \'true\' }}\n      shell: bash\n      run: just ci-lint-workflow\n    - name: Read shared values\n      id: shared\n      shell: bash\n      env:\n        BUILD_JOBS_KEY: ${{ inputs.build-jobs-key }}\n      run: |\n        echo "rust_toolchain=$(awk -F\'\\"\' \'/^channel = / {print $2}\' rust-toolchain.toml)" >> "$GITHUB_OUTPUT"\n        echo "rust_verification_owner=$(just --evaluate rust_verification_owner)" >> "$GITHUB_OUTPUT"\n        if [ "${{ inputs.include-deny-version }}" = "true" ]; then\n          echo "deny_version=$(just --evaluate deny_version)" >> "$GITHUB_OUTPUT"\n        fi\n        if [ "${{ inputs.include-nextest-version }}" = "true" ]; then\n          echo "nextest_version=$(just --evaluate nextest_version)" >> "$GITHUB_OUTPUT"\n        fi\n        if [ "${{ inputs.include-build-values }}" = "true" ]; then\n          echo "target=$(just --evaluate target)" >> "$GITHUB_OUTPUT"\n          echo "zig_version=$(just --evaluate zig_version)" >> "$GITHUB_OUTPUT"\n          echo "zigbuild_version=$(just --evaluate zigbuild_version)" >> "$GITHUB_OUTPUT"\n        fi\n        if [ -n "$BUILD_JOBS_KEY" ]; then\n          cargo_build_jobs="$(\n            python3 - ci/github-actions-runners.toml "$BUILD_JOBS_KEY" <<\'PY\'\n        import pathlib\n        import sys\n        import tomllib\n\n        config = tomllib.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))\n        value = config.get("cargo_build_jobs")\n        for part in sys.argv[2].split("."):\n            if not isinstance(value, dict) or part not in value:\n                raise SystemExit(f"cargo_build_jobs.{sys.argv[2]} missing")\n            value = value.get(part)\n        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:\n            raise SystemExit(f"cargo_build_jobs.{sys.argv[2]} must be a positive integer")\n        print(value)\n        PY\n          )"\n          echo "cargo_build_jobs=$cargo_build_jobs" >> "$GITHUB_OUTPUT"\n          echo "CARGO_BUILD_JOBS=$cargo_build_jobs" >> "$GITHUB_ENV"\n        fi\n    - name: Install Rust linker\n      if: ${{ inputs.install-rust-linker == \'true\' }}\n      shell: bash\n      run: |\n        mapfile -t rust_linker_programs < <(python3.12 "${{ steps.shared.outputs.rust_verification_owner }}" fast-linker-programs --repo "$GITHUB_WORKSPACE")\n        if [ "${#rust_linker_programs[@]}" -eq 0 ]; then\n          echo "::error::remote_fast_linker has no configured programs"\n          exit 1\n        fi\n        for rust_linker_program in "${rust_linker_programs[@]}"; do\n          if command -v "$rust_linker_program" >/dev/null; then\n            echo "BOLT_RUST_FAST_LINKER=$rust_linker_program" >> "$GITHUB_ENV"\n            exit 0\n          fi\n        done\n        if sudo apt-get update; then\n          for rust_linker_program in "${rust_linker_programs[@]}"; do\n            if sudo apt-get install -y --no-install-recommends "$rust_linker_program"; then\n              echo "BOLT_RUST_FAST_LINKER=$rust_linker_program" >> "$GITHUB_ENV"\n              exit 0\n            fi\n          done\n        fi\n        echo "::warning::failed to install any configured Rust linker; continuing without fast linker"\n        echo "Rust linker: unavailable; continuing without BOLT_RUST_FAST_LINKER" >> "$GITHUB_STEP_SUMMARY"\n    - name: Resolve managed target dir\n      if: ${{ inputs.include-managed-target-dir == \'true\' }}\n      id: target_dir\n      shell: bash\n      run: |\n        managed_target_dir="$(python3 "${{ steps.shared.outputs.rust_verification_owner }}" target-dir --repo "$GITHUB_WORKSPACE")"\n        managed_target_dir_relative="$(python3 -c \'import os, sys; print(os.path.relpath(sys.argv[2], sys.argv[1]))\' "$GITHUB_WORKSPACE" "$managed_target_dir")"\n        echo "managed_target_dir=$managed_target_dir" >> "$GITHUB_OUTPUT"\n        echo "managed_target_dir_relative=$managed_target_dir_relative" >> "$GITHUB_OUTPUT"\n    - name: Setup Rust toolchain\n      shell: bash\n      run: echo setup\n'
BASE_NEXTEST_CONFIG = "\n[test-groups]\nlive-node = { max-threads = 1 }\n\n[[profile.default.overrides]]\nfilter = 'binary(=bolt_v2) & (test(~bolt_v3_client_registration::tests::) | test(~bolt_v3_live_node::tests::))'\ntest-group = 'live-node'\n\n[[profile.default.overrides]]\nfilter = 'binary(=bolt_v3_adapter_mapping) | binary(=bolt_v3_client_registration) | binary(=bolt_v3_controlled_connect) | binary(=bolt_v3_credential_log_suppression) | binary(=bolt_v3_readiness) | binary(=bolt_v3_strategy_registration) | binary(=bolt_v3_submit_admission) | binary(=chainlink_startup_boot) | binary(=config_parsing) | binary(=lake_batch) | binary(=nt_runtime_capture) | binary(=venue_contract)'\ntest-group = 'live-node'\n"

def all_standalone_live_node_manifest(verifier=None) -> CiTestManifest:
    if verifier is None:
        verifier = load_verifier()
    member_to_harness = {member: member for member in verifier.LIVE_NODE_NEXTEST_BINARIES}
    harness_to_members = {member: (member,) for member in verifier.LIVE_NODE_NEXTEST_BINARIES}
    return CiTestManifest(member_to_harness=member_to_harness, harness_to_members=harness_to_members)
_PINNED_TEST_HARNESS_NAMES: tuple[str, ...] = ('iv', 'pricing', 'maker_taker')
TEST_HARNESS_MEMBER = 'bolt_v3_fixture_member'

def test_harness_names(verifier=None) -> tuple[str, ...]:
    del verifier
    return _PINNED_TEST_HARNESS_NAMES

def base_test_harness_manifest(harness_to_members: dict[str, tuple[str, ...]] | None=None, *, verifier=None) -> CiTestManifest:
    if harness_to_members is None:
        harness_to_members = {harness: (harness, TEST_HARNESS_MEMBER) if harness == 'iv' else (harness,) for harness in test_harness_names(verifier)}
    member_to_harness: dict[str, str] = {}
    for harness, members in harness_to_members.items():
        for member in members:
            member_to_harness.setdefault(member, harness)
    return CiTestManifest(member_to_harness=member_to_harness, harness_to_members=harness_to_members)

def write_test_harness_fixture(root: pathlib.Path, *, manifest: CiTestManifest | None=None, cargo_autotests: str='false', test_files: dict[str, str] | None=None, workflow_text: str='jobs:\n  test:\n    steps:\n      - run: cargo test --test pricing\n', justfile_text: str='ci-test:\n    cargo test --test iv\n', write_workflow: bool=True, write_justfile: bool=True) -> None:
    effective_manifest = manifest if manifest is not None else base_test_harness_manifest()
    harness_names = tuple(effective_manifest.harness_to_members.keys())
    cargo_lines = ['[package]', 'name = "bolt-v2-fixture"', 'version = "0.0.0"', 'edition = "2021"', f'autotests = {cargo_autotests}', '']
    for harness in harness_names:
        cargo_lines.extend(['[[test]]', f'name = "{harness}"', f'path = "tests/{harness}.rs"', ''])
    (root / 'Cargo.toml').write_text('\n'.join(cargo_lines), encoding='utf-8')
    tests_root = root / 'tests'
    tests_root.mkdir()
    fixture_files = {harness: '' for harness in harness_names}
    for harness, members in effective_manifest.harness_to_members.items():
        for member in members:
            if member != harness:
                fixture_files[member] = '#[test]\nfn fixture_member_runs() {}\n'
    if test_files:
        fixture_files.update(test_files)
    for stem, text in fixture_files.items():
        path = tests_root / f'{stem}.rs'
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding='utf-8')
    if write_workflow:
        workflow_path = root / '.github' / 'workflows' / 'ci.yml'
        workflow_path.parent.mkdir(parents=True)
        workflow_path.write_text(workflow_text, encoding='utf-8')
    if write_justfile:
        (root / 'justfile').write_text(justfile_text, encoding='utf-8')
LOCAL_COMPILE_POLICY_TOML = '\n[local_compile_policy]\nenabled = true\nallowed_ci_env = "GITHUB_ACTIONS"\nbreak_glass_env = "BOLT_ALLOW_LOCAL_RUST"\nrefused_managed_commands = ["test", "clippy", "build"]\nrefused_cargo_subcommands = ["b", "bench", "build", "c", "check", "clippy", "d", "doc", "fetch", "install", "nextest", "r", "run", "rustc", "t", "test", "zigbuild"]\n'
LOCAL_LANE_POLICY_TOML = '\n[local_lane_policy]\nenabled = true\nallowed_ci_env = "GITHUB_ACTIONS"\nlock_dir = "/tmp/rust-verification-lanes"\nacquire_timeout_seconds = 1800\nheartbeat_seconds = 15\npoll_interval_seconds = 1\n'
BASE_RUST_VERIFICATION_POLICY = f'\nschema_version = 2\nproject_id = "bolt-v2"\ntarget_namespace = "bolt-v2"\n\n{LOCAL_COMPILE_POLICY_TOML}\n{LOCAL_LANE_POLICY_TOML}\n\n[remote_verification]\npoll_interval_seconds = 15\nchecks_appear_timeout_seconds = 300\noverall_timeout_seconds = 3600\ndiagnostic_log_max_lines = 160\ndiagnostic_log_max_bytes = 20000\ndiagnostic_unavailable_notice_interval_polls = 4\n'
BASE_BVS_RUST_VERIFICATION_POLICY = f'\nschema_version = 2\nproject_id = "backtesting-vertical-slice"\ntarget_namespace = "backtesting-vertical-slice"\n\n{LOCAL_COMPILE_POLICY_TOML}\n\n[remote_compile_cache]\nenabled = true\nenable_env = "BOLT_RUST_VERIFICATION_SCCACHE"\nci_env = "GITHUB_ACTIONS"\nwrapper_env = "SCCACHE_PATH"\nwrapper_program = "sccache"\n\n[remote_fast_linker]\nenabled = true\nci_env = "GITHUB_ACTIONS"\nlinker_env = "BOLT_RUST_FAST_LINKER"\nprograms = ["mold"]\n\n{LOCAL_LANE_POLICY_TOML}\n'

def write_rust_verification_policy_fixtures(root: pathlib.Path) -> None:
    root_policy = root / 'ci' / 'rust-verification.toml'
    root_policy.parent.mkdir(parents=True, exist_ok=True)
    root_policy.write_text(BASE_RUST_VERIFICATION_POLICY, encoding='utf-8')
    bvs_policy = root / 'crates' / 'backtesting-vertical-slice' / 'ci' / 'rust-verification.toml'
    bvs_policy.parent.mkdir(parents=True, exist_ok=True)
    bvs_policy.write_text(BASE_BVS_RUST_VERIFICATION_POLICY, encoding='utf-8')

def write_runner_config_fixture(root: pathlib.Path) -> None:
    runner_config = root / 'ci' / 'github-actions-runners.toml'
    runner_config.parent.mkdir(parents=True, exist_ok=True)
    runner_config.write_text((REPO_ROOT / 'ci' / 'github-actions-runners.toml').read_text(), encoding='utf-8')
    actionlint = root / '.github' / 'actionlint.yaml'
    actionlint.parent.mkdir(parents=True, exist_ok=True)
    actionlint.write_text((REPO_ROOT / '.github' / 'actionlint.yaml').read_text(), encoding='utf-8')

def repo_git_command(*args: str) -> list[str]:
    return ['git', *GIT_AUTO_MAINTENANCE_SUPPRESSION_ARGS, *args]

def suppress_repo_auto_maintenance(repo: pathlib.Path) -> None:
    """Persist the suppression inside `repo`'s own config.

    `repo_git_command` only reaches the git processes this suite launches. Git
    drops the repo-scoped config environment when it runs a command against a
    *different* repository, so `git push` never carries the suppression into
    the remote's `receive-pack`, and git spawned by the code under test never
    sees it either. `maintenance.auto=false` prevents the detached maintenance
    writer; `gc.auto=0` does nothing for that failure mode. Both settings remain
    persisted because git's documented behavior says the legacy `git gc --auto`
    path consults `gc.auto`. A fixture repo that carries the settings in its own
    config is covered whoever runs git against it, bare or not.
    """
    for key, value in GIT_AUTO_MAINTENANCE_SUPPRESSION_CONFIG:
        subprocess.run(repo_git_command('-C', str(repo), 'config', key, value), check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

def init_fixture_repo(repo: pathlib.Path, *init_args: str) -> pathlib.Path:
    """`git init` a fixture repo that never spawns auto-maintenance."""
    subprocess.run(repo_git_command('init', *init_args, str(repo)), check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    suppress_repo_auto_maintenance(repo)
    return repo

def clone_fixture_repo(source: pathlib.Path, destination: pathlib.Path, *clone_args: str) -> pathlib.Path:
    """Clone a fixture repo and persist auto-maintenance suppression.

    `git clone` does not copy `gc.auto`/`maintenance.auto` from the source
    repository — the clone starts with an empty local config — so the
    suppression has to be re-persisted into the clone, or a git process launched
    by anyone else will detach a maintenance writer into it.
    """
    subprocess.run(repo_git_command('clone', *clone_args, str(source), str(destination)), check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    suppress_repo_auto_maintenance(destination)
    return destination

def repo_source_text(path: str | pathlib.Path) -> str:
    source_path = pathlib.Path(path)
    if not source_path.is_absolute():
        source_path = REPO_ROOT / source_path
    return source_path.read_text().replace('\r\n', '\n')

def write_repo_workflows(workflow_dir: pathlib.Path) -> None:
    workflow_dir.mkdir(parents=True)
    for path in sorted((REPO_ROOT / '.github' / 'workflows').glob('*.y*ml')):
        (workflow_dir / path.name).write_text(path.read_text(encoding='utf-8'), encoding='utf-8')

def write_storage_tripwire_policy_fixture(root: pathlib.Path) -> pathlib.Path:
    policy_path = root / 'ci' / 'storage-tripwire.toml'
    policy_path.parent.mkdir(parents=True, exist_ok=True)
    policy_path.write_text((REPO_ROOT / 'ci' / 'storage-tripwire.toml').read_text(encoding='utf-8'), encoding='utf-8')
    return policy_path

def run_verifier_main_with_no_mistakes(no_mistakes_text: str, *, write_mergify_config: bool=True) -> tuple[int, str]:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        verifier_path = tmp_path / 'scripts' / 'verify_ci_workflow_hygiene.py'
        verifier_path.parent.mkdir(parents=True)
        verifier_path.write_text(repo_source_text(VERIFIER_PATH))
        workflow_dir = tmp_path / '.github' / 'workflows'
        write_repo_workflows(workflow_dir)
        write_test_harness_fixture(tmp_path, manifest=base_test_harness_manifest(), write_workflow=False, write_justfile=False)
        action_path = tmp_path / '.github' / 'actions' / 'setup-environment' / 'action.yml'
        action_path.parent.mkdir(parents=True)
        action_path.write_text(BASE_ACTION)
        sccache_action_path = tmp_path / '.github' / 'actions' / 'sccache-setup' / 'action.yml'
        sccache_action_path.parent.mkdir(parents=True)
        sccache_action_path.write_text(repo_source_text('.github/actions/sccache-setup/action.yml'))
        sccache_stats_action_path = tmp_path / '.github' / 'actions' / 'sccache-stats' / 'action.yml'
        sccache_stats_action_path.parent.mkdir(parents=True)
        sccache_stats_action_path.write_text(repo_source_text('.github/actions/sccache-stats/action.yml'))
        sccache_eligibility_path = tmp_path / 'scripts' / 'sccache_eligibility.py'
        sccache_eligibility_path.write_text(repo_source_text('scripts/sccache_eligibility.py'))
        sccache_config_path = tmp_path / 'ci' / 'sccache-location.toml'
        sccache_config_path.parent.mkdir(parents=True, exist_ok=True)
        sccache_config_path.write_text(repo_source_text('ci/sccache-location.toml'))
        nextest_path = tmp_path / '.config' / 'nextest.toml'
        nextest_path.parent.mkdir(parents=True)
        nextest_path.write_text(BASE_NEXTEST_CONFIG)
        (tmp_path / '.no-mistakes.yaml').write_text(no_mistakes_text)
        if write_mergify_config:
            (tmp_path / '.mergify.yml').write_text((REPO_ROOT / '.mergify.yml').read_text())
        write_rust_verification_policy_fixtures(tmp_path)
        write_runner_config_fixture(tmp_path)
        storage_policy_path = write_storage_tripwire_policy_fixture(tmp_path)
        temp_verifier = load_verifier(verifier_path, 'verify_ci_workflow_hygiene_no_mistakes_entrypoint')
        temp_verifier.build_test_manifest = lambda _manifest_path, _tests_root: base_test_harness_manifest()
        original_discover_policy = temp_verifier.ci_storage_tripwire.discover_policy_path
        temp_verifier.ci_storage_tripwire.discover_policy_path = lambda _root: storage_policy_path
        stdout = io.StringIO()
        stderr = io.StringIO()
        try:
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                result = temp_verifier.main()
        finally:
            temp_verifier.ci_storage_tripwire.discover_policy_path = original_discover_policy
        return (result, stdout.getvalue() + stderr.getvalue())
