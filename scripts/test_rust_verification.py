"""Self-tests for the repo-local Rust verification owner."""
from __future__ import annotations
import contextlib
import argparse
import io
import os
import json
import importlib.util
import pathlib
import shlex
import subprocess
import sys
import tempfile
import textwrap
import tomllib
from rust_verification_test_fixtures import load_owner_module, rust_verification_policy_text, write_executable, write_policy
REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT = REPO_ROOT / 'scripts' / 'rust_verification.py'
TEST_HEAD = 'a' * 40

def run_owner(args: list[str], *, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run([sys.executable, str(SCRIPT), *args], cwd=REPO_ROOT, env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)

def parse_log(path: pathlib.Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in path.read_text(encoding='utf-8').splitlines():
        key, value = line.split('=', 1)
        values[key] = value
    return values

def assert_minimal_toml_accepts_quoted_keys() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / 'policy.toml'
        path.write_text(textwrap.dedent('                [quoted_keys]\n                "alpha-beta" = "first"\n                "gamma-delta" = "second"\n                '), encoding='utf-8')
        parsed = owner.parse_minimal_toml(path)
    values = parsed['quoted_keys']
    if values != {'alpha-beta': 'first', 'gamma-delta': 'second'}:
        raise AssertionError(values)

def assert_minimal_toml_accepts_multiline_string_arrays() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / 'policy.toml'
        path.write_text(textwrap.dedent('                [arrays]\n                values = [\n                  "scripts",\n                  "justfile",\n                  "ci/rust-verification.toml",\n                ]\n                '), encoding='utf-8')
        parsed = owner.parse_minimal_toml(path)
    pathspecs = parsed['arrays']['values']
    if pathspecs != ['scripts', 'justfile', 'ci/rust-verification.toml']:
        raise AssertionError(pathspecs)

def assert_minimal_toml_matches_tomllib_for_rust_policy() -> None:
    minimal_toml_path = REPO_ROOT / 'scripts' / 'minimal_toml.py'
    spec = importlib.util.spec_from_file_location('minimal_toml_under_test', minimal_toml_path)
    if spec is None or spec.loader is None:
        raise AssertionError('unable to load scripts/minimal_toml.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    policy = REPO_ROOT / 'ci' / 'rust-verification.toml'
    with policy.open('rb') as handle:
        expected = tomllib.load(handle)
    parsed = module.load(policy)
    if parsed != expected:
        raise AssertionError('minimal_toml.py must match tomllib for ci/rust-verification.toml')

def assert_minimal_toml_rejects_non_ascii_bare_digits() -> None:
    minimal_toml_path = REPO_ROOT / 'scripts' / 'minimal_toml.py'
    spec = importlib.util.spec_from_file_location('minimal_toml_under_test', minimal_toml_path)
    if spec is None or spec.loader is None:
        raise AssertionError('unable to load scripts/minimal_toml.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / 'policy.toml'
        path.write_text('schema_version = ²\n', encoding='utf-8')
        try:
            module.load(path, error_cls=RuntimeError)
        except RuntimeError as exc:
            if 'unsupported value' not in str(exc):
                raise AssertionError(f'unexpected minimal TOML error: {exc}') from exc
        else:
            raise AssertionError('non-ASCII bare digits must stay in the parser error path')

def same_path(left: str, right: pathlib.Path) -> bool:
    return pathlib.Path(left).resolve() == right.resolve()

def assert_repo_local_owner_contract() -> None:
    if not SCRIPT.exists():
        raise AssertionError(f'missing repo-local owner script: {SCRIPT}')
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / 'repo'
        repo.mkdir()
        write_policy(repo)
        bin_dir = tmp_path / 'bin'
        bin_dir.mkdir()
        cargo_log = tmp_path / 'cargo.log'
        just_log = tmp_path / 'just.log'
        write_executable(bin_dir / 'cargo', f"""#!/usr/bin/env bash\nprintf 'cwd=%s\\n' "$PWD" > {cargo_log}\nprintf 'target=%s\\n' "$CARGO_TARGET_DIR" >> {cargo_log}\nprintf 'args=%s\\n' "$*" >> {cargo_log}\n""")
        write_executable(bin_dir / 'just', f"""#!/usr/bin/env bash\nprintf 'cwd=%s\\n' "$PWD" > {just_log}\nprintf 'target=%s\\n' "$CARGO_TARGET_DIR" >> {just_log}\nprintf 'args=%s\\n' "$*" >> {just_log}\n""")
        root_base = tmp_path / 'rust-root'
        env = os.environ.copy()
        env.pop('GITHUB_ACTIONS', None)
        env.pop('BOLT_ALLOW_LOCAL_RUST', None)
        env['PATH'] = f"{bin_dir}{os.pathsep}{env['PATH']}"
        env['RUST_VERIFICATION_ROOT_BASE'] = str(root_base)
        target_dir = root_base / 'bolt-v2' / 'target'
        result = run_owner(['target-dir', '--repo', str(repo)], env=env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        if result.stdout.strip() != str(target_dir):
            raise AssertionError((result.stdout, target_dir))
        if not target_dir.is_dir():
            raise AssertionError(f'target-dir did not create {target_dir}')
        binary = target_dir / 'aarch64-unknown-linux-gnu' / 'release' / 'bolt-v2'
        binary.parent.mkdir(parents=True)
        binary.write_text('binary', encoding='utf-8')
        result = run_owner(['binary-path', '--repo', str(repo), '--bin', 'bolt-v2'], env=env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        if result.stdout.strip() != str(binary):
            raise AssertionError((result.stdout, binary))
        result = run_owner(['cargo', '--repo', str(repo), '--', 'fmt', '--check'], env=env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        cargo_values = parse_log(cargo_log)
        if not same_path(cargo_values['cwd'], repo) or cargo_values['target'] != '' or cargo_values['args'] != 'fmt --check':
            raise AssertionError(cargo_values)
        result = run_owner(['run', '--repo', str(repo), 'build', '--flag'], env=env)
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        refusal = json.loads(result.stderr)
        next_steps = '\n'.join(refusal.get('next_steps', []))
        if refusal.get('refusal_code') != 'local_compile_disabled' or 'just rust-probe suggest' not in next_steps or 'publish the exact branch head with just sandbox-safe-push' not in next_steps or ('invoke just final-review <PR> exactly once' not in next_steps):
            raise AssertionError(refusal)
        allowed_env = env.copy()
        allowed_env['GITHUB_ACTIONS'] = 'true'
        result = run_owner(['run', '--repo', str(repo), 'build', '--flag'], env=allowed_env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        just_values = parse_log(just_log)
        expected_args = f"-f {repo / 'justfile'} --working-directory {repo} -- managed-build --flag"
        if not same_path(just_values['cwd'], repo) or just_values['target'] != str(target_dir) or just_values['args'] != expected_args:
            raise AssertionError(just_values)
        break_glass_env = env.copy()
        break_glass_env['BOLT_ALLOW_LOCAL_RUST'] = '1'
        result = run_owner(['run', '--repo', str(repo), 'build', '--break-glass'], env=break_glass_env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        just_values = parse_log(just_log)
        expected_args = f"-f {repo / 'justfile'} --working-directory {repo} -- managed-build --break-glass"
        if not same_path(just_values['cwd'], repo) or just_values['target'] != str(target_dir) or just_values['args'] != expected_args:
            raise AssertionError(just_values)
        result = run_owner(['validate-policy', '--repo', str(repo)], env=env)
        if result.returncode != 0:
            raise AssertionError(result.stderr)
        payload = json.loads(result.stdout)
        expected_payload = {'build_profile': 'release', 'build_target': 'aarch64-unknown-linux-gnu', 'policy': str(repo / 'ci' / 'rust-verification.toml'), 'project_id': 'bolt-v2', 'status': 'ok'}
        if payload != expected_payload:
            raise AssertionError(payload)

def assert_rust_probe_guidance_distinguishes_feedback_from_proof() -> None:
    owner = load_owner_module()
    expected_guidance = 'fixed final-review workflow is the complete remote evidence path'
    if expected_guidance not in owner.RUST_PROBE_HELP_EPILOG:
        raise AssertionError(owner.RUST_PROBE_HELP_EPILOG)
    stdout = io.StringIO()
    run = {'databaseId': 1001, 'event': 'workflow_dispatch', 'headSha': TEST_HEAD, 'status': 'completed', 'conclusion': 'success', 'createdAt': '2026-06-13T00:00:00Z', 'displayTitle': 'Rust Probe abc123 check-lib', 'url': 'https://github.com/seungpyoson/bolt-v2/actions/runs/1001'}
    with contextlib.redirect_stdout(stdout):
        result = owner.evaluate_rust_probe_run(run, head=TEST_HEAD, probe_id='abc123')
    output = stdout.getvalue()
    if result != 0:
        raise AssertionError((result, output))
    if expected_guidance not in output:
        raise AssertionError(output)

def assert_fmt_avoids_managed_cache_lock() -> None:
    owner = load_owner_module()
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / 'repo'
        repo.mkdir()
        write_policy(repo)
        observed: dict[str, object] = {}

        def forbidden_cache_lock(_policy: dict[str, object], *, exclusive: bool) -> object:
            raise AssertionError('cargo fmt must not touch the managed cache lock')

        def fake_run_process(argv: list[str], *, repo: pathlib.Path, env: dict[str, str]) -> int:
            observed['argv'] = argv
            observed['env'] = env
            return 0
        original_cache_lock = owner.cache_lock
        original_run_process = owner.run_process
        try:
            owner.cache_lock = forbidden_cache_lock
            owner.run_process = fake_run_process
            args = type('Args', (), {'repo': str(repo), 'args': ['fmt', '--check']})()
            result = owner.cmd_cargo(args)
        finally:
            owner.cache_lock = original_cache_lock
            owner.run_process = original_run_process
    if result != 0:
        raise AssertionError(result)
    env = observed.get('env')
    if not isinstance(env, dict) or 'CARGO_TARGET_DIR' in env or 'BOLT_ALLOW_LOCAL_RUST' in env:
        raise AssertionError(observed)

def assert_system_python_contract() -> None:
    system_python = pathlib.Path('/usr/bin/python3')
    if not system_python.exists():
        return
    result = subprocess.run([str(system_python), '-S', str(SCRIPT), 'repo-status', '--repo', str(REPO_ROOT)], cwd=REPO_ROOT, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    if result.returncode != 0:
        raise AssertionError(result.stderr)
    if result.stdout.strip() != 'managed':
        raise AssertionError(result.stdout)

def assert_oversized_policy_fails_closed() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / 'repo'
        repo.mkdir()
        write_policy(repo)
        policy = repo / 'ci' / 'rust-verification.toml'
        policy.write_text('schema_version = 1\n' + '# padding\n' * 140000, encoding='utf-8')
        result = run_owner(['validate-policy', '--repo', str(repo)], env=os.environ.copy())
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if 'exceeds maximum size' not in result.stderr:
            raise AssertionError(result.stderr)

def assert_validate_policy_rejects_unknown_cheap_lane_just_recipe() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / 'repo'
        repo.mkdir()
        write_policy(repo)
        policy = repo / 'ci' / 'rust-verification.toml'
        policy.write_text(policy.read_text(encoding='utf-8').replace('poll_interval_seconds = 1\n', 'poll_interval_seconds = 1\ncheap_lane_just_recipes = ["missing-cheap-lane-recipe"]\n', 1), encoding='utf-8')
        result = run_owner(['validate-policy', '--repo', str(repo)], env=os.environ.copy())
    if result.returncode != 2:
        raise AssertionError((result.returncode, result.stdout, result.stderr))
    if 'missing from justfile' not in result.stderr:
        raise AssertionError(result.stderr)

@contextlib.contextmanager
def _patched_environ(values: dict[str, 'str | None']):
    saved = {key: os.environ.get(key) for key in values}
    try:
        for key, value in values.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        yield
    finally:
        for key, previous in saved.items():
            if previous is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = previous
REMOTE_COMPILE_CACHE_POLICY = {'enabled': True, 'enable_env': 'BOLT_RUST_VERIFICATION_SCCACHE', 'ci_env': 'GITHUB_ACTIONS', 'wrapper_env': 'SCCACHE_PATH', 'wrapper_program': 'sccache'}
REMOTE_FAST_LINKER_POLICY = {'enabled': True, 'ci_env': 'GITHUB_ACTIONS', 'linker_env': 'BOLT_RUST_FAST_LINKER', 'programs': ['mold']}

def assert_validate_remote_compile_cache_policy_contract() -> None:
    owner = load_owner_module()
    owner.validate_remote_compile_cache_policy({'remote_compile_cache': dict(REMOTE_COMPILE_CACHE_POLICY)})
    owner.validate_remote_compile_cache_policy({})
    rejects = [{**REMOTE_COMPILE_CACHE_POLICY, 'enabled': False}, {**REMOTE_COMPILE_CACHE_POLICY, 'enable_env': 'bad lower'}, {**REMOTE_COMPILE_CACHE_POLICY, 'ci_env': 'NOT_GITHUB_ACTIONS'}, {**REMOTE_COMPILE_CACHE_POLICY, 'wrapper_program': 'not-sccache'}, {**REMOTE_COMPILE_CACHE_POLICY, 'unexpected_key': 'x'}]
    for bad in rejects:
        try:
            owner.validate_remote_compile_cache_policy({'remote_compile_cache': bad})
        except owner.PolicyError:
            continue
        raise AssertionError(f'expected PolicyError for remote_compile_cache={bad!r}')

def assert_managed_remote_compile_cache_env_fails_open() -> None:
    owner = load_owner_module()
    policy = {'remote_compile_cache': dict(REMOTE_COMPILE_CACHE_POLICY)}
    with _patched_environ({'BOLT_RUST_VERIFICATION_SCCACHE': '1', 'GITHUB_ACTIONS': 'true', 'SCCACHE_PATH': '/opt/sccache/sccache'}):
        if owner.managed_remote_compile_cache_env(policy) != {'RUSTC_WRAPPER': '/opt/sccache/sccache'}:
            raise AssertionError('wrapper must be injected when every gate is satisfied')
    gate_off_cases = [{'BOLT_RUST_VERIFICATION_SCCACHE': '0', 'GITHUB_ACTIONS': 'true', 'SCCACHE_PATH': '/opt/sccache/sccache'}, {'BOLT_RUST_VERIFICATION_SCCACHE': '1', 'GITHUB_ACTIONS': None, 'SCCACHE_PATH': '/opt/sccache/sccache'}, {'BOLT_RUST_VERIFICATION_SCCACHE': '1', 'GITHUB_ACTIONS': 'false', 'SCCACHE_PATH': '/opt/sccache/sccache'}]
    for env in gate_off_cases:
        with _patched_environ(env):
            if owner.managed_remote_compile_cache_env(policy) != {}:
                raise AssertionError(f'wrapper must stay off when a gate is unmet: {env!r}')
    for path in (None, '', '/opt/sc cache/sccache', '/opt/sccache/other'):
        with _patched_environ({'BOLT_RUST_VERIFICATION_SCCACHE': '1', 'GITHUB_ACTIONS': 'true', 'SCCACHE_PATH': path}):
            if owner.managed_remote_compile_cache_env(policy) != {}:
                raise AssertionError(f'malformed wrapper must fail open to no wrapper: {path!r}')

def assert_validate_remote_fast_linker_policy_contract() -> None:
    owner = load_owner_module()
    owner.validate_remote_fast_linker_policy({'remote_fast_linker': dict(REMOTE_FAST_LINKER_POLICY)})
    owner.validate_remote_fast_linker_policy({})
    rejects = [{**REMOTE_FAST_LINKER_POLICY, 'enabled': False}, {**REMOTE_FAST_LINKER_POLICY, 'ci_env': 'NOT_GITHUB_ACTIONS'}, {**REMOTE_FAST_LINKER_POLICY, 'linker_env': 'bad lower'}, {**REMOTE_FAST_LINKER_POLICY, 'programs': []}, {**REMOTE_FAST_LINKER_POLICY, 'programs': ['mold', 'lld']}, {**REMOTE_FAST_LINKER_POLICY, 'unexpected_key': 'x'}]
    for bad in rejects:
        try:
            owner.validate_remote_fast_linker_policy({'remote_fast_linker': bad})
        except owner.PolicyError:
            continue
        raise AssertionError(f'expected PolicyError for remote_fast_linker={bad!r}')

def assert_managed_remote_fast_linker_env_selects_available_program() -> None:
    owner = load_owner_module()
    policy = {'target_namespace': 'rust-verification-fast-linker-test', 'remote_fast_linker': dict(REMOTE_FAST_LINKER_POLICY)}
    with tempfile.TemporaryDirectory() as tmp:
        bin_dir = pathlib.Path(tmp) / 'bin'
        bin_dir.mkdir()
        write_executable(bin_dir / 'cc', '#!/usr/bin/env bash\nexit 0\n')
        write_executable(bin_dir / 'mold', '#!/usr/bin/env bash\nexit 0\n')
        base_path = os.environ.get('PATH', '')
        with _patched_environ({'PATH': f'{bin_dir}{os.pathsep}{base_path}', 'RUST_VERIFICATION_ROOT_BASE': str(pathlib.Path(tmp) / 'rv-root'), 'GITHUB_ACTIONS': 'true', 'BOLT_RUST_FAST_LINKER': 'mold'}):
            env = owner.managed_remote_fast_linker_env(REPO_ROOT, policy)
        wrapper_dir = pathlib.Path(env['PATH'].split(os.pathsep)[0])
        if 'RUSTFLAGS' in env:
            raise AssertionError('fast linker path must not inject RUSTFLAGS because it invalidates sccache keys')
        if not (wrapper_dir / 'cc').is_file():
            raise AssertionError('fast linker path must prepend a generated cc wrapper')
        if bin_dir.as_posix() not in env['PATH']:
            raise AssertionError(env)
    with tempfile.TemporaryDirectory() as tmp:
        bin_dir = pathlib.Path(tmp) / 'bin'
        bin_dir.mkdir()
        write_executable(bin_dir / 'cc', '#!/usr/bin/env bash\nexit 0\n')
        write_executable(bin_dir / 'lld', '#!/usr/bin/env bash\nexit 0\n')
        base_path = os.environ.get('PATH', '')
        with _patched_environ({'PATH': f'{bin_dir}{os.pathsep}{base_path}', 'RUST_VERIFICATION_ROOT_BASE': str(pathlib.Path(tmp) / 'rv-root'), 'GITHUB_ACTIONS': 'true', 'BOLT_RUST_FAST_LINKER': 'lld'}):
            try:
                owner.managed_remote_fast_linker_env(REPO_ROOT, policy)
            except owner.PolicyError:
                pass
            else:
                raise AssertionError('unconfigured CI linker must fail closed')
    with tempfile.TemporaryDirectory() as tmp:
        bin_dir = pathlib.Path(tmp) / 'bin'
        bin_dir.mkdir()
        fake_real_cc = bin_dir / 'cc'
        write_executable(fake_real_cc, '#!/usr/bin/env bash\nexit 0\n')
        write_executable(bin_dir / 'mold', '#!/usr/bin/env bash\nexit 0\n')
        rv_root = pathlib.Path(tmp) / 'rv-root'
        with _patched_environ({'RUST_VERIFICATION_ROOT_BASE': str(rv_root)}):
            wrapper_dir = owner.target_dir(REPO_ROOT, policy) / 'fast-linker-bin'
        wrapper_dir.mkdir(parents=True)
        fake_recursive_cc = wrapper_dir / 'cc'
        write_executable(fake_recursive_cc, '#!/usr/bin/env bash\nexit 99\n')
        base_path = os.environ.get('PATH', '')
        with _patched_environ({'PATH': f'{wrapper_dir}{os.pathsep}{bin_dir}{os.pathsep}{base_path}', 'RUST_VERIFICATION_ROOT_BASE': str(rv_root), 'GITHUB_ACTIONS': 'true', 'BOLT_RUST_FAST_LINKER': 'mold'}):
            env = owner.managed_remote_fast_linker_env(REPO_ROOT, policy)
        wrapper = pathlib.Path(env['PATH'].split(os.pathsep)[0]) / 'cc'
        wrapper_text = wrapper.read_text(encoding='utf-8')
        if f'real_cc={shlex.quote(str(fake_real_cc))}' not in wrapper_text:
            raise AssertionError(f'fast linker wrapper must resolve real cc outside wrapper dir: {wrapper_text!r}')
        if str(fake_recursive_cc) in wrapper_text:
            raise AssertionError(f'fast linker wrapper must not resolve itself as real cc: {wrapper_text!r}')
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        bin_dir = tmp_path / 'bin'
        bin_dir.mkdir()
        fake_real_cc = bin_dir / 'cc'
        write_executable(fake_real_cc, '#!/usr/bin/env bash\nexit 0\n')
        write_executable(bin_dir / 'mold', '#!/usr/bin/env bash\nexit 0\n')
        rv_root = tmp_path / 'rv-root'
        with _patched_environ({'RUST_VERIFICATION_ROOT_BASE': str(rv_root)}):
            wrapper_dir = owner.target_dir(REPO_ROOT, policy) / 'fast-linker-bin'
        wrapper_dir.mkdir(parents=True)
        fake_recursive_cc = wrapper_dir / 'cc'
        write_executable(fake_recursive_cc, '#!/usr/bin/env bash\nexit 99\n')
        wrapper_dir_link = tmp_path / 'fast-linker-bin-link'
        wrapper_dir_link.symlink_to(wrapper_dir, target_is_directory=True)
        base_path = os.environ.get('PATH', '')
        with _patched_environ({'PATH': f'{wrapper_dir_link}{os.pathsep}{bin_dir}{os.pathsep}{base_path}', 'RUST_VERIFICATION_ROOT_BASE': str(rv_root), 'GITHUB_ACTIONS': 'true', 'BOLT_RUST_FAST_LINKER': 'mold'}):
            env = owner.managed_remote_fast_linker_env(REPO_ROOT, policy)
        wrapper = pathlib.Path(env['PATH'].split(os.pathsep)[0]) / 'cc'
        wrapper_text = wrapper.read_text(encoding='utf-8')
        if f'real_cc={shlex.quote(str(fake_real_cc))}' not in wrapper_text:
            raise AssertionError(f'fast linker wrapper must resolve real cc outside symlinked wrapper dir: {wrapper_text!r}')
        if str(wrapper_dir_link / 'cc') in wrapper_text or str(fake_recursive_cc) in wrapper_text:
            raise AssertionError(f'fast linker wrapper must not resolve symlinked wrapper as real cc: {wrapper_text!r}')
    non_ci_cases = [{'GITHUB_ACTIONS': None, 'BOLT_RUST_FAST_LINKER': 'mold'}, {'GITHUB_ACTIONS': 'false', 'BOLT_RUST_FAST_LINKER': 'mold'}]
    for env_values in non_ci_cases:
        with _patched_environ(env_values):
            if owner.managed_remote_fast_linker_env(REPO_ROOT, policy) != {}:
                raise AssertionError(f'fast linker must stay off outside CI: {env_values!r}')
    for env_values in (
        {'GITHUB_ACTIONS': 'true', 'BOLT_RUST_FAST_LINKER': None},
        {'GITHUB_ACTIONS': 'true', 'BOLT_RUST_FAST_LINKER': 'gold'},
    ):
        with _patched_environ(env_values):
            try:
                owner.managed_remote_fast_linker_env(REPO_ROOT, policy)
            except owner.PolicyError:
                pass
            else:
                raise AssertionError(f'CI linker selection must fail closed: {env_values!r}')
    with tempfile.TemporaryDirectory() as tmp:
        bin_dir = pathlib.Path(tmp)
        write_executable(bin_dir / 'cc', '#!/usr/bin/env bash\nexit 0\n')
        with _patched_environ({'PATH': str(bin_dir), 'GITHUB_ACTIONS': 'true', 'BOLT_RUST_FAST_LINKER': 'mold'}):
            try:
                owner.managed_remote_fast_linker_env(REPO_ROOT, policy)
            except owner.PolicyError:
                pass
            else:
                raise AssertionError('CI must fail before Cargo when mold is unavailable')
    with tempfile.TemporaryDirectory() as tmp:
        bin_dir = pathlib.Path(tmp)
        write_executable(bin_dir / 'mold', '#!/usr/bin/env bash\nexit 0\n')
        with _patched_environ({'PATH': str(bin_dir), 'GITHUB_ACTIONS': 'true', 'BOLT_RUST_FAST_LINKER': 'mold'}):
            try:
                owner.managed_remote_fast_linker_env(REPO_ROOT, policy)
            except owner.PolicyError:
                pass
            else:
                raise AssertionError('CI must fail before Cargo when cc is unavailable')

def assert_managed_env_scrubs_then_reinjects_wrapper() -> None:
    owner = load_owner_module()
    policy = {'target_namespace': 'rust-verification-sccache-test', 'remote_compile_cache': dict(REMOTE_COMPILE_CACHE_POLICY)}
    with _patched_environ({'RUSTC_WRAPPER': '/evil/wrapper', 'BOLT_RUST_VERIFICATION_SCCACHE': '1', 'GITHUB_ACTIONS': 'true', 'SCCACHE_PATH': '/opt/sccache/sccache'}):
        env = owner.managed_env(REPO_ROOT, policy)
    if env.get('RUSTC_WRAPPER') != '/opt/sccache/sccache':
        raise AssertionError('managed_env must scrub a pre-existing wrapper and re-inject the sccache path')
    with _patched_environ({'RUSTC_WRAPPER': '/evil/wrapper', 'BOLT_RUST_VERIFICATION_SCCACHE': '1', 'GITHUB_ACTIONS': None, 'SCCACHE_PATH': '/opt/sccache/sccache'}):
        env = owner.managed_env(REPO_ROOT, policy)
    if 'RUSTC_WRAPPER' in env:
        raise AssertionError('managed_env must not inject a wrapper outside CI (GITHUB_ACTIONS unset)')

def remote_compile_policy_text() -> str:
    return rust_verification_policy_text(target_namespace='rust-verification-remote-cache-test') + textwrap.dedent('\n            [remote_compile_cache]\n            enabled = true\n            enable_env = "BOLT_RUST_VERIFICATION_SCCACHE"\n            ci_env = "GITHUB_ACTIONS"\n            wrapper_env = "SCCACHE_PATH"\n            wrapper_program = "sccache"\n            ')

def install_owner_process_spies(owner: object, calls: list[tuple[list[str], str | None]], results: list[int], managed_env_calls: list[dict[str, str]]) -> tuple[object, object, object, object]:

    def fake_disk_preflight(_repo: pathlib.Path, _policy: dict[str, object]) -> None:
        calls.append((['__disk_preflight__'], None))
        return None

    @contextlib.contextmanager
    def fake_cache_lock(_policy: dict[str, object], *, exclusive: bool):
        calls.append((['__cache_lock__', str(exclusive)], None))
        yield

    def fake_run_process(argv: list[str], *, repo: pathlib.Path, env: dict[str, str]) -> int:
        calls.append((list(argv), env.get('RUSTC_WRAPPER')))
        if results:
            return results.pop(0)
        return 0
    original_preflight = owner.disk_preflight_refusal_payload
    original_cache_lock = owner.cache_lock
    original_run_process = owner.run_process
    original_managed_env = owner.managed_env

    def fake_managed_env(repo: pathlib.Path, policy: dict[str, object] | None=None) -> dict[str, str]:
        env = original_managed_env(repo, policy)
        managed_env_calls.append(dict(env))
        return env
    owner.disk_preflight_refusal_payload = fake_disk_preflight
    owner.cache_lock = fake_cache_lock
    owner.run_process = fake_run_process
    owner.managed_env = fake_managed_env
    return (original_preflight, original_cache_lock, original_run_process, original_managed_env)

def restore_owner_process_spies(owner: object, originals: tuple[object, object, object, object]) -> None:
    owner.disk_preflight_refusal_payload = originals[0]
    owner.cache_lock = originals[1]
    owner.run_process = originals[2]
    owner.managed_env = originals[3]

def assert_commands_test_schema_is_exact() -> None:
    owner = load_owner_module()
    policy = tomllib.loads(rust_verification_policy_text())
    owner.validate_policy_data(policy)
    policy['commands']['test']['unexpected_key'] = True
    try:
        owner.validate_policy_data(policy)
    except owner.PolicyError:
        return
    raise AssertionError('expected PolicyError for an unexpected commands.test key')

def assert_managed_test_runs_one_configured_command_for_root_and_backtester() -> None:
    owner = load_owner_module()
    backtester_policy = (REPO_ROOT / 'crates' / 'backtesting-vertical-slice' / 'ci' / 'rust-verification.toml').read_text(encoding='utf-8')
    policies = (('root', remote_compile_policy_text()), ('backtester', backtester_policy))
    expected = ['cargo', 'nextest', 'run', '--locked', '--config-file', 'nextest.toml', '--', '--skip', 'slow_case']
    for label, policy_text in policies:
        calls: list[tuple[list[str], str | None]] = []
        managed_env_calls: list[dict[str, str]] = []
        with tempfile.TemporaryDirectory() as tmp:
            repo = pathlib.Path(tmp) / label
            repo.mkdir()
            bin_dir = pathlib.Path(tmp) / 'bin'
            bin_dir.mkdir()
            write_executable(bin_dir / 'cc', '#!/usr/bin/env bash\nexit 0\n')
            write_executable(bin_dir / 'mold', '#!/usr/bin/env bash\nexit 0\n')
            write_policy(repo, policy_text=policy_text)
            originals = install_owner_process_spies(owner, calls, [], managed_env_calls)
            try:
                with _patched_environ({'PATH': f'{bin_dir}{os.pathsep}{os.environ.get("PATH", "")}', 'RUST_VERIFICATION_ROOT_BASE': str(pathlib.Path(tmp) / 'rv-root'), 'BOLT_RUST_VERIFICATION_SCCACHE': '1', 'GITHUB_ACTIONS': 'true', 'SCCACHE_PATH': '/opt/sccache/sccache', 'BOLT_RUST_FAST_LINKER': 'mold'}):
                    result = owner.cmd_run(argparse.Namespace(repo=str(repo), command='test', args=['--config-file', 'nextest.toml', '--', '--skip', 'slow_case'], args_separator=False))
            finally:
                restore_owner_process_spies(owner, originals)
        if result != 0:
            raise AssertionError((label, result))
        run_calls = [call for call in calls if call[0][0] == 'cargo']
        if run_calls != [(expected, '/opt/sccache/sccache')]:
            raise AssertionError((label, run_calls))
        if len(managed_env_calls) != 1 or managed_env_calls[0].get('RUSTC_WRAPPER') != '/opt/sccache/sccache':
            raise AssertionError((label, managed_env_calls))

def assert_direct_managed_nextest_runs_once_and_returns_first_status() -> None:
    owner = load_owner_module()
    cases = ((['nextest', 'run', '--locked', '--no-run', '--', '--skip', 'slow_case'], [42, 0], 42, [0]), (['nextest', 'run', '--locked', '--future-nextest-flag'], [], 0, []), (['nextest', 'run', '--locked', '--', '--skip', 'slow_case'], [], 0, []))
    for cargo_args, outcomes, expected_result, expected_remaining in cases:
        calls: list[tuple[list[str], str | None]] = []
        managed_env_calls: list[dict[str, str]] = []
        remaining = list(outcomes)
        stderr = io.StringIO()
        with tempfile.TemporaryDirectory() as tmp:
            repo = pathlib.Path(tmp) / 'repo'
            repo.mkdir()
            write_policy(repo, policy_text=remote_compile_policy_text())
            originals = install_owner_process_spies(owner, calls, remaining, managed_env_calls)
            try:
                with _patched_environ({'BOLT_RUST_VERIFICATION_SCCACHE': '1', 'GITHUB_ACTIONS': 'true', 'SCCACHE_PATH': '/opt/sccache/sccache'}):
                    with contextlib.redirect_stderr(stderr):
                        result = owner.cmd_cargo(argparse.Namespace(repo=str(repo), args=['--', *cargo_args]))
            finally:
                restore_owner_process_spies(owner, originals)
        if result != expected_result:
            raise AssertionError((cargo_args, result, calls))
        run_calls = [call for call in calls if call[0][0] == 'cargo']
        expected_call = (['cargo', *cargo_args], '/opt/sccache/sccache')
        if run_calls != [expected_call]:
            raise AssertionError((cargo_args, run_calls))
        if remaining != expected_remaining:
            raise AssertionError((cargo_args, remaining))
        if len(managed_env_calls) != 1 or managed_env_calls[0].get('RUSTC_WRAPPER') != '/opt/sccache/sccache':
            raise AssertionError((cargo_args, managed_env_calls))
        if stderr.getvalue():
            raise AssertionError((cargo_args, stderr.getvalue()))

def assert_managed_env_scrubs_then_injects_fast_linker_wrapper() -> None:
    owner = load_owner_module()
    policy = {'target_namespace': 'rust-verification-fast-linker-test', 'remote_fast_linker': dict(REMOTE_FAST_LINKER_POLICY)}
    with tempfile.TemporaryDirectory() as tmp:
        bin_dir = pathlib.Path(tmp) / 'bin'
        bin_dir.mkdir()
        cc_log = pathlib.Path(tmp) / 'cc.log'
        write_executable(bin_dir / 'cc', f"""#!/usr/bin/env bash\nprintf '%s\\n' "$@" >> {cc_log}\nexit 0\n""")
        write_executable(bin_dir / 'mold', '#!/usr/bin/env bash\nexit 0\n')
        base_path = os.environ.get('PATH', '')
        with _patched_environ({'PATH': f'{bin_dir}{os.pathsep}{base_path}', 'RUSTFLAGS': '-C link-arg=-fuse-ld=gold', 'RUST_VERIFICATION_ROOT_BASE': str(pathlib.Path(tmp) / 'rv-root'), 'GITHUB_ACTIONS': 'true', 'BOLT_RUST_FAST_LINKER': 'mold'}):
            env = owner.managed_env(REPO_ROOT, policy)
        wrapper_dir = pathlib.Path(env['PATH'].split(os.pathsep)[0])
        wrapper = wrapper_dir / 'cc'
        if not wrapper.is_file():
            raise AssertionError('managed_env must prepend a generated cc wrapper for the configured fast linker')
        run = subprocess.run(['cc', 'input.o', '-o', 'output'], executable=str(wrapper), check=False)
        if run.returncode != 0:
            raise AssertionError(f'fast linker wrapper failed with rc={run.returncode}')
        if 'RUSTFLAGS' in env:
            raise AssertionError('managed_env must keep RUSTFLAGS scrubbed so sccache keys remain stable')
        logged_args = cc_log.read_text(encoding='utf-8').splitlines()
        if logged_args[:1] != ['-fuse-ld=mold']:
            raise AssertionError(f'fast linker wrapper must add mold link arg before link command args: {logged_args!r}')
        cc_log.write_text('', encoding='utf-8')
        pass_through_cases = [(['-c', 'input.c'], 'compile-only'), (['-S', 'input.c'], 'assembly-only'), (['-E', 'input.c'], 'preprocess-only'), (['-M', 'input.c'], 'dependency-only'), (['-MM', 'input.c'], 'user-dependency-only'), (['-print-prog-name=ld'], 'compiler-query'), (['-dumpmachine'], 'compiler-query'), (['-dumpspecs'], 'compiler-query'), (['--help=warnings'], 'compiler-query')]
        for args, description in pass_through_cases:
            cc_log.write_text('', encoding='utf-8')
            run = subprocess.run(['cc', *args], executable=str(wrapper), check=False)
            if run.returncode != 0:
                raise AssertionError(f'fast linker wrapper {description} pass-through failed with rc={run.returncode}')
            pass_through_args = cc_log.read_text(encoding='utf-8').splitlines()
            if '-fuse-ld=mold' in pass_through_args:
                raise AssertionError(f'fast linker wrapper must not add link args to {description} commands: {pass_through_args!r}')
        cc_log.write_text('', encoding='utf-8')
        run = subprocess.run(['cc', '-fuse-ld=gold', 'input.o', '-o', 'output'], executable=str(wrapper), check=False)
        if run.returncode == 0:
            raise AssertionError('fast linker wrapper accepted an alternate explicit linker')
        if cc_log.read_text(encoding='utf-8'):
            raise AssertionError('alternate explicit linker reached the real compiler')
        cc_log.write_text('', encoding='utf-8')
        run = subprocess.run(['cc', '-fuse-ld=mold', 'input.o', '-o', 'output'], executable=str(wrapper), check=False)
        if run.returncode != 0:
            raise AssertionError(f'configured explicit linker failed with rc={run.returncode}')
        configured_args = cc_log.read_text(encoding='utf-8').splitlines()
        if configured_args[:1] != ['-fuse-ld=mold'] or configured_args.count('-fuse-ld=mold') != 1:
            raise AssertionError(f'configured explicit linker was not preserved exactly once: {configured_args!r}')
        cc_log.write_text('', encoding='utf-8')
        run = subprocess.run(['cc', '-Xlinker', '-E', 'input.o', '-o', 'output'], executable=str(wrapper), check=False)
        if run.returncode != 0:
            raise AssertionError(f'fast linker wrapper link command with forwarded -E failed with rc={run.returncode}')
        forwarded_link_args = cc_log.read_text(encoding='utf-8').splitlines()
        if forwarded_link_args[:1] != ['-fuse-ld=mold']:
            raise AssertionError(f'fast linker wrapper must still add mold when -E is forwarded as a linker argument: {forwarded_link_args!r}')
    with _patched_environ({'RUSTFLAGS': '-C link-arg=-fuse-ld=gold', 'GITHUB_ACTIONS': None, 'BOLT_RUST_FAST_LINKER': 'mold'}):
        env = owner.managed_env(REPO_ROOT, policy)
    if 'RUSTFLAGS' in env:
        raise AssertionError('managed_env must not inject fast linker flags outside CI')

def assert_fast_linker_programs_command_reads_policy() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp) / 'repo'
        repo.mkdir()
        policy_text = rust_verification_policy_text() + textwrap.dedent('\n                [remote_fast_linker]\n                enabled = true\n                ci_env = "GITHUB_ACTIONS"\n                linker_env = "BOLT_RUST_FAST_LINKER"\n                programs = ["mold"]\n                ')
        write_policy(repo, policy_text=policy_text)
        result = run_owner(['fast-linker-programs', '--repo', str(repo)], env=os.environ.copy())
    if result.returncode != 0:
        raise AssertionError((result.returncode, result.stdout, result.stderr))
    if result.stdout.splitlines() != ['mold']:
        raise AssertionError(result.stdout)

def run_global_cargo_config_assertion(repo: pathlib.Path, *, home: pathlib.Path, root_base: pathlib.Path, cargo_home: pathlib.Path | None=None) -> subprocess.CompletedProcess[str]:
    env = os.environ.copy()
    env['HOME'] = str(home)
    if cargo_home is not None:
        env['CARGO_HOME'] = str(cargo_home)
    env['RUST_VERIFICATION_ROOT_BASE'] = str(root_base)
    return run_owner(['assert-global-cargo-target-dir', '--repo', str(repo)], env=env)

def assert_global_cargo_target_dir_config_is_created_and_idempotent() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / 'repo'
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / 'home'
        root_base = tmp_path / 'rust-root'
        expected_target = (root_base / 'bolt-v2' / 'target').resolve()
        first = run_global_cargo_config_assertion(repo, home=home, root_base=root_base)
        if first.returncode != 0:
            raise AssertionError((first.returncode, first.stdout, first.stderr))
        config = home / '.cargo' / 'config.toml'
        first_content = config.read_text(encoding='utf-8')
        if '[build]' not in first_content or f'target-dir = "{expected_target}"' not in first_content:
            raise AssertionError(first_content)
        second = run_global_cargo_config_assertion(repo, home=home, root_base=root_base)
        if second.returncode != 0:
            raise AssertionError((second.returncode, second.stdout, second.stderr))
        if config.read_text(encoding='utf-8') != first_content:
            raise AssertionError('global Cargo config assertion is not idempotent')

def assert_global_cargo_target_dir_config_preserves_existing_content() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / 'repo'
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / 'home'
        config = home / '.cargo' / 'config.toml'
        config.parent.mkdir(parents=True)
        config.write_text(textwrap.dedent('                [net]\n                git-fetch-with-cli = true\n\n                [build]\n                rustflags = ["-Dwarnings"]\n                '), encoding='utf-8')
        root_base = tmp_path / 'rust-root'
        expected_target = (root_base / 'bolt-v2' / 'target').resolve()
        result = run_global_cargo_config_assertion(repo, home=home, root_base=root_base)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        content = config.read_text(encoding='utf-8')
        for preserved in ('[net]', 'git-fetch-with-cli = true', 'rustflags = ["-Dwarnings"]'):
            if preserved not in content:
                raise AssertionError(content)
        if f'target-dir = "{expected_target}"' not in content:
            raise AssertionError(content)
        second = run_global_cargo_config_assertion(repo, home=home, root_base=root_base)
        if second.returncode != 0:
            raise AssertionError((second.returncode, second.stdout, second.stderr))
        if config.read_text(encoding='utf-8') != content:
            raise AssertionError('assertion rewrote existing config on second run')

def assert_global_cargo_target_dir_config_refuses_conflict() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / 'repo'
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / 'home'
        config = home / '.cargo' / 'config.toml'
        config.parent.mkdir(parents=True)
        original = textwrap.dedent('            [build]\n            target-dir = "/tmp/raw-target"\n            ')
        config.write_text(original, encoding='utf-8')
        result = run_global_cargo_config_assertion(repo, home=home, root_base=tmp_path / 'rust-root')
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if 'build.target-dir' not in result.stderr or '/tmp/raw-target' not in result.stderr:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if config.read_text(encoding='utf-8') != original:
            raise AssertionError('conflicting global Cargo config was rewritten')

def assert_global_cargo_target_dir_config_accepts_resolved_equivalent_path() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / 'repo'
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / 'home'
        config = home / '.cargo' / 'config.toml'
        config.parent.mkdir(parents=True)
        actual_root = tmp_path / 'actual-rust-root'
        alias_root = tmp_path / 'alias-rust-root'
        actual_root.mkdir()
        alias_root.symlink_to(actual_root, target_is_directory=True)
        original = textwrap.dedent(f'''            [build]\n            target-dir = "{alias_root / 'bolt-v2' / 'target'}"\n            ''')
        config.write_text(original, encoding='utf-8')
        result = run_global_cargo_config_assertion(repo, home=home, root_base=actual_root)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if 'already-configured' not in result.stdout:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if config.read_text(encoding='utf-8') != original:
            raise AssertionError('resolved-equivalent global Cargo config was rewritten')

def assert_global_cargo_target_dir_config_uses_effective_cargo_home() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / 'repo'
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / 'home'
        cargo_home = tmp_path / 'cargo-home'
        root_base = tmp_path / 'rust-root'
        expected_target = (root_base / 'bolt-v2' / 'target').resolve()
        result = run_global_cargo_config_assertion(repo, home=home, root_base=root_base, cargo_home=cargo_home)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        effective_config = cargo_home / 'config.toml'
        home_config = home / '.cargo' / 'config.toml'
        if f'target-dir = "{expected_target}"' not in effective_config.read_text(encoding='utf-8'):
            raise AssertionError(effective_config.read_text(encoding='utf-8'))
        if home_config.exists():
            raise AssertionError('assertion wrote HOME Cargo config while CARGO_HOME was set')

def assert_global_cargo_target_dir_config_updates_legacy_config_when_present() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / 'repo'
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / 'home'
        legacy_config = home / '.cargo' / 'config'
        legacy_config.parent.mkdir(parents=True)
        legacy_config.write_text('[net]\ngit-fetch-with-cli = true\n', encoding='utf-8')
        root_base = tmp_path / 'rust-root'
        expected_target = (root_base / 'bolt-v2' / 'target').resolve()
        result = run_global_cargo_config_assertion(repo, home=home, root_base=root_base)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        content = legacy_config.read_text(encoding='utf-8')
        if '[net]' not in content or f'target-dir = "{expected_target}"' not in content:
            raise AssertionError(content)
        if (home / '.cargo' / 'config.toml').exists():
            raise AssertionError('assertion wrote config.toml even though Cargo will read legacy config')

def assert_global_cargo_target_dir_config_preserves_dotted_build_keys() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / 'repo'
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / 'home'
        config = home / '.cargo' / 'config.toml'
        config.parent.mkdir(parents=True)
        config.write_text(textwrap.dedent('                build.rustflags = ["-Dwarnings"]\n\n                [net]\n                git-fetch-with-cli = true\n                '), encoding='utf-8')
        root_base = tmp_path / 'rust-root'
        expected_target = (root_base / 'bolt-v2' / 'target').resolve()
        result = run_global_cargo_config_assertion(repo, home=home, root_base=root_base)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        content = config.read_text(encoding='utf-8')
        if 'build.rustflags = ["-Dwarnings"]' not in content:
            raise AssertionError(content)
        if f'build.target-dir = "{expected_target}"' not in content:
            raise AssertionError(content)

def assert_global_cargo_target_dir_config_handles_quoted_build_table() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / 'repo'
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / 'home'
        config = home / '.cargo' / 'config.toml'
        config.parent.mkdir(parents=True)
        config.write_text('[ "build" ]\nrustflags = ["-Dwarnings"]\n', encoding='utf-8')
        root_base = tmp_path / 'rust-root'
        expected_target = (root_base / 'bolt-v2' / 'target').resolve()
        result = run_global_cargo_config_assertion(repo, home=home, root_base=root_base)
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        content = config.read_text(encoding='utf-8')
        if f'target-dir = "{expected_target}"' not in content or 'rustflags = ["-Dwarnings"]' not in content:
            raise AssertionError(content)

def assert_global_cargo_target_dir_config_refuses_inline_build_table() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / 'repo'
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / 'home'
        config = home / '.cargo' / 'config.toml'
        config.parent.mkdir(parents=True)
        original = 'build = { rustflags = ["-Dwarnings"] }\n'
        config.write_text(original, encoding='utf-8')
        result = run_global_cargo_config_assertion(repo, home=home, root_base=tmp_path / 'rust-root')
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if 'cannot be safely edited' not in result.stderr:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if config.read_text(encoding='utf-8') != original:
            raise AssertionError('unsupported inline Cargo config was rewritten')

def assert_global_cargo_target_dir_config_reports_non_utf8_without_traceback() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / 'repo'
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / 'home'
        config = home / '.cargo' / 'config.toml'
        config.parent.mkdir(parents=True)
        config.write_bytes(b'\xff')
        result = run_global_cargo_config_assertion(repo, home=home, root_base=tmp_path / 'rust-root')
        if result.returncode != 2:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if 'Traceback' in result.stderr:
            raise AssertionError(result.stderr)

def assert_global_cargo_target_dir_config_preserves_symlink() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = pathlib.Path(tmp)
        repo = tmp_path / 'repo'
        repo.mkdir()
        write_policy(repo)
        home = tmp_path / 'home'
        config = home / '.cargo' / 'config.toml'
        target_config = tmp_path / 'dotfiles' / 'cargo-config.toml'
        config.parent.mkdir(parents=True)
        target_config.parent.mkdir()
        target_config.write_text('[net]\ngit-fetch-with-cli = true\n', encoding='utf-8')
        config.symlink_to(target_config)
        result = run_global_cargo_config_assertion(repo, home=home, root_base=tmp_path / 'rust-root')
        if result.returncode != 0:
            raise AssertionError((result.returncode, result.stdout, result.stderr))
        if not config.is_symlink():
            raise AssertionError('Cargo config symlink was replaced')
        if 'target-dir' not in target_config.read_text(encoding='utf-8'):
            raise AssertionError(target_config.read_text(encoding='utf-8'))

def assert_setup_recipe_asserts_global_cargo_target_dir() -> None:
    source = (REPO_ROOT / 'justfile').read_text(encoding='utf-8')
    if 'assert-global-cargo-target-dir' not in source:
        raise AssertionError('just setup must assert the machine-global Cargo target-dir')

def assert_verify_remote_command_is_absent() -> None:
    owner = load_owner_module()
    parser = owner.build_parser()
    try:
        with contextlib.redirect_stderr(io.StringIO()):
            parser.parse_args(['verify-remote', '--repo', str(REPO_ROOT)])
    except SystemExit as exc:
        if exc.code == 0:
            raise AssertionError('verify-remote unexpectedly parsed successfully')
    else:
        raise AssertionError('verify-remote remains a public command')

def main() -> int:
    assert_repo_local_owner_contract()
    assert_validate_remote_compile_cache_policy_contract()
    assert_managed_remote_compile_cache_env_fails_open()
    assert_managed_env_scrubs_then_reinjects_wrapper()
    assert_commands_test_schema_is_exact()
    assert_managed_test_runs_one_configured_command_for_root_and_backtester()
    assert_direct_managed_nextest_runs_once_and_returns_first_status()
    assert_validate_remote_fast_linker_policy_contract()
    assert_managed_remote_fast_linker_env_selects_available_program()
    assert_managed_env_scrubs_then_injects_fast_linker_wrapper()
    assert_fast_linker_programs_command_reads_policy()
    assert_rust_probe_guidance_distinguishes_feedback_from_proof()
    assert_fmt_avoids_managed_cache_lock()
    assert_minimal_toml_accepts_quoted_keys()
    assert_minimal_toml_accepts_multiline_string_arrays()
    assert_minimal_toml_matches_tomllib_for_rust_policy()
    assert_minimal_toml_rejects_non_ascii_bare_digits()
    assert_system_python_contract()
    assert_oversized_policy_fails_closed()
    assert_validate_policy_rejects_unknown_cheap_lane_just_recipe()
    assert_verify_remote_command_is_absent()
    assert_global_cargo_target_dir_config_is_created_and_idempotent()
    assert_global_cargo_target_dir_config_preserves_existing_content()
    assert_global_cargo_target_dir_config_refuses_conflict()
    assert_global_cargo_target_dir_config_accepts_resolved_equivalent_path()
    assert_global_cargo_target_dir_config_uses_effective_cargo_home()
    assert_global_cargo_target_dir_config_updates_legacy_config_when_present()
    assert_global_cargo_target_dir_config_preserves_dotted_build_keys()
    assert_global_cargo_target_dir_config_handles_quoted_build_table()
    assert_global_cargo_target_dir_config_refuses_inline_build_table()
    assert_global_cargo_target_dir_config_reports_non_utf8_without_traceback()
    assert_global_cargo_target_dir_config_preserves_symlink()
    assert_setup_recipe_asserts_global_cargo_target_dir()
    print('OK: Rust verification owner self-tests passed.')
    return 0
if __name__ == '__main__':
    import lane_governor
    lane_governor.acquire()
    sys.exit(main())
