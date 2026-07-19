"""Verify CI workflow hygiene invariants for the current workflow topology."""
from __future__ import annotations
from collections.abc import Iterable
import pathlib
import re
import sys
import tomllib

SCRIPT_DIR = pathlib.Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))
from workflow_expression_analysis import YAML_ANCHOR_PATTERN, YAML_KEY_PATTERN, strip_comment, unquote_yaml_scalar
from cargo_command_analysis import RECURSIVE_WRAPPER_EXECUTABLES, SHELL_COMMAND_BOUNDARIES, cargo_install_source_build_tools_in_text, command_has_raw_cargo, command_tokens, consume_assignment_words, consume_cargo_global_options, env_inner_tokens, expand_cargo_aliases, expand_known_shell_assignment_names, expand_known_shell_command_variables, managed_rust_verification_cargo_args, managed_rust_verification_command_tokens, path_executable_looks_like_cargo, path_executable_looks_like_rustc, path_name_looks_like_renamed_cargo, path_name_looks_like_renamed_rustc, persistent_shell_assignment_values, raw_cargo_storage_override_messages_from_tokens, raw_rust_tool_token, shell_alias_payloads, shell_array_assignment_values_from_tokens, shell_command_substitution_at, shell_command_substitution_payloads, shell_logical_lines, simple_cargo_aliases, strip_shell_redirections, text_has_path_style_cargo_config, tokens_have_raw_cargo_launch, tokens_have_target_routing_override, wrapper_inner_tokens
from shell_dataflow_analysis import dynamic_env_target_override_messages, github_env_assignment_lines, storage_transfer_policy_errors
from command_understanding import CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT, cargo_args_for_target_routing_scan, cargo_subcommand, cargo_subcommand_with_index, nextest_subcommand_with_index, python_call_command_argument, python_call_name, python_command_string, python_constant_string, python_inline_command_payloads
from ci_test_manifest import CiTestManifest, _mask_rust_non_code, build_test_manifest
from rust_verification import CARGO_ALIAS_SUBCOMMANDS, CARGO_DISK_PREFLIGHT_SUBCOMMANDS
from merge_queue_preflight import verify_mergify_config
import ci_storage_tripwire
from verifier_io import require_nonempty

COMMAND_UNDERSTANDING_PARITY_EXPORTS = (
    cargo_subcommand_with_index,
    nextest_subcommand_with_index,
    python_call_command_argument,
    python_call_name,
    python_command_string,
    python_constant_string,
)
REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_WORKFLOW_DIR = REPO_ROOT / '.github' / 'workflows'
DEFAULT_WORKFLOW = DEFAULT_WORKFLOW_DIR / 'final-review.yml'
DEFAULT_WORKFLOW_GLOBS = ('*.yml', '*.yaml')
DEFAULT_NO_MISTAKES_CONFIG = REPO_ROOT / '.no-mistakes.yaml'
DEFAULT_MERGIFY_CONFIG = REPO_ROOT / '.mergify.yml'
DEFAULT_RUNNERS_CONFIG = REPO_ROOT / 'ci' / 'github-actions-runners.toml'
DEFAULT_ACTIONLINT_CONFIG = REPO_ROOT / '.github' / 'actionlint.yaml'
DEFAULT_RUST_VERIFICATION_POLICY = REPO_ROOT / 'ci' / 'rust-verification.toml'
DEFAULT_BVS_RUST_VERIFICATION_POLICY = REPO_ROOT / 'crates' / 'backtesting-vertical-slice' / 'ci' / 'rust-verification.toml'
RUNNERS_CONFIG_LABEL = 'ci/github-actions-runners.toml'
JOB_RUNS_ON_VAR_RE = re.compile('^    runs-on:\\s*\\$\\{\\{\\s*vars\\.([A-Z0-9_]+)\\s*\\}\\}\\s*$')
WORKFLOW_RUNNER_CONFIG_KEYS = {'final-review.yml': 'final_review', '.github/workflows/final-review.yml': 'final_review', 'flaky-test-detection.yml': 'flaky_test_detection', '.github/workflows/flaky-test-detection.yml': 'flaky_test_detection', 'flaky-test-smoke.yml': 'flaky_test_smoke', '.github/workflows/flaky-test-smoke.yml': 'flaky_test_smoke', 'ci-storage-tripwire.yml': 'ci_storage_tripwire', '.github/workflows/ci-storage-tripwire.yml': 'ci_storage_tripwire', 'ci-storage-cleanup-alert.yml': 'ci_storage_cleanup_alert', '.github/workflows/ci-storage-cleanup-alert.yml': 'ci_storage_cleanup_alert', 'ci-runner-debug.yml': 'ci_runner_debug', '.github/workflows/ci-runner-debug.yml': 'ci_runner_debug', 'rust-probe.yml': 'rust_probe', '.github/workflows/rust-probe.yml': 'rust_probe', 'root-artifact.yml': 'root_artifact', '.github/workflows/root-artifact.yml': 'root_artifact', 'ai-review-glm.yml': 'ai_review_glm', '.github/workflows/ai-review-glm.yml': 'ai_review_glm', 'ai-review-kimi-cli.yml': 'ai_review_kimi_cli', '.github/workflows/ai-review-kimi-cli.yml': 'ai_review_kimi_cli', 'claude-code-review.yml': 'claude_code_review', '.github/workflows/claude-code-review.yml': 'claude_code_review', 'reference-boundary-capture.yml': 'reference_boundary_capture', '.github/workflows/reference-boundary-capture.yml': 'reference_boundary_capture', 'advisory.yml': 'advisory', '.github/workflows/advisory.yml': 'advisory', 'summary.yml': 'summary', '.github/workflows/summary.yml': 'summary', 'stale.yml': 'stale', '.github/workflows/stale.yml': 'stale', 'weekly-cleanup.yml': 'weekly_cleanup', '.github/workflows/weekly-cleanup.yml': 'weekly_cleanup', 'performance-improver.yml': 'performance_improver', '.github/workflows/performance-improver.yml': 'performance_improver', 'tech-debt-review.yml': 'tech_debt_review', '.github/workflows/tech-debt-review.yml': 'tech_debt_review'}
DORMANT_REVIEW_WORKFLOW_PATHS = frozenset({
    '.github/workflows/final-review.yml',
    '.github/workflows/ai-review-glm.yml',
    '.github/workflows/ai-review-kimi-cli.yml',
    '.github/workflows/claude-code-review.yml',
})
DORMANT_REVIEW_CONFIG_KEYS = frozenset({
    'final_review',
    'ai_review_glm',
    'ai_review_kimi_cli',
    'claude_code_review',
})
DEFAULT_REPO_AUTOMATION_FILES = (REPO_ROOT / 'justfile',)
DEFAULT_REPO_AUTOMATION_GLOBS = ((REPO_ROOT / 'scripts', '*.sh'), (REPO_ROOT / 'tests', '*.sh'), (REPO_ROOT / '.github' / 'scripts', '*.sh'), (REPO_ROOT / '.github' / 'actions', '**/action.yml'), (REPO_ROOT / '.github' / 'actions', '**/action.yaml'))
JULES_ADVISORY_WORKFLOW_PATHS = frozenset(('.github/workflows/weekly-cleanup.yml', '.github/workflows/performance-improver.yml', '.github/workflows/tech-debt-review.yml'))
JULES_ADVISORY_ENDPOINT_VARIABLE = 'JULES_SESSIONS_ENDPOINT'
JULES_ADVISORY_TIMEOUT_VARIABLE = 'JULES_SESSION_TIMEOUT_MINUTES'
JULES_ADVISORY_SECRET = 'JULES_API_KEY'
JULES_AWS_COMMAND_RE = re.compile('(^|[\\s;&|])aws([ \\t\\r\\n;&|]|$)')
GITHUB_SECRET_REF_RE = re.compile('secrets\\.([A-Z0-9_]+)')
LOCAL_COMPILE_REFUSED_MANAGED_COMMANDS = {'build', 'clippy', 'test'}
LOCAL_COMPILE_REFUSED_CARGO_SUBCOMMANDS = set(CARGO_DISK_PREFLIGHT_SUBCOMMANDS) | set(CARGO_ALIAS_SUBCOMMANDS)
YAML_STEP_ITEM_RE = re.compile(f'^-\\s+(?:{YAML_ANCHOR_PATTERN}(?:\\s+|$))?')
YAML_RUN_LINE_RE = re.compile(f'^(\\s*)(?:-\\s*(?:{YAML_ANCHOR_PATTERN}\\s+)?)?run:\\s*(.*?)\\s*$')
YAML_FOLDED_RUN_LINE_RE = re.compile(f'^(\\s*)(?:-\\s*(?:{YAML_ANCHOR_PATTERN}\\s+)?)?run:\\s*>[+-]?\\s*(?:#.*)?$')

class PolicyError(RuntimeError):
    pass
DECLARED_TOP_LEVEL_TEST_HELPERS = {'bolt_v3_iv_support'}
RUST_TEST_ATTR_RE = re.compile('#\\s*\\[\\s*(?:tokio::)?test(?:\\s*\\([^]]*\\))?\\s*\\]')
RUST_INNER_ATTR_RE = re.compile('#!\\s*\\[\\s*([A-Za-z_][A-Za-z0-9_]*)')
BANNED_RUST_INNER_ATTRS = {'feature', 'no_std', 'no_main', 'crate_name', 'crate_type', 'crate_id'}
INLINE_CARGO_BUILD_JOBS_RE = re.compile('\\bCARGO_BUILD_JOBS\\b')
CARGO_BUILD_JOBS_COMPILE_COMMAND_RE = re.compile('(?:^|[\\s;&|()])(?:cargo\\s+(?:build|check|clippy|test|nextest|zigbuild)\\b|cargo\\s+--repo\\b|just\\s+(?:build|check-aarch64|clippy|source-fence|source-fence-static|cargo-shim-tests|test-archive-filtered-run|test-archive|test-archive-run|test|bte-clippy|bte-test-archive|bte-test-archive-run|bte-test)\\b)')

def parse_jobs(workflow_text: str) -> dict[str, list[str]]:
    """Parse this repo's strict GitHub Actions job subset.

    Top-level job ids must be indented by exactly two spaces under `jobs:`.
    The verifier reports required job ids that drift to another indentation.
    """
    lines = workflow_text.splitlines()
    jobs: dict[str, list[str]] = {}
    in_jobs = False
    current: str | None = None
    for line in lines:
        clean = strip_comment(line)
        if clean == 'jobs:':
            in_jobs = True
            current = None
            continue
        if not in_jobs:
            continue
        if clean and (not clean.startswith((' ', '\t'))):
            break
        match = re.match('^  ([^ \\t:#][^:#]*):(?:\\s+&[A-Za-z0-9_.-]+)?\\s*$', clean)
        if match:
            current = match.group(1).strip().strip('\'"')
            jobs[current] = []
            continue
        if current is not None:
            jobs[current].append(clean)
    return jobs

def top_level_block(workflow_text: str, key: str) -> list[str]:
    lines = workflow_text.splitlines()
    start_line = f'{key}:'
    for index, line in enumerate(lines):
        clean = strip_comment(line)
        if clean != start_line:
            continue
        block: list[str] = []
        for child_line in lines[index + 1:]:
            child_clean = strip_comment(child_line)
            if child_clean and (not child_clean.startswith((' ', '\t'))):
                break
            block.append(child_clean)
        return block
    return []

def yaml_scalar(value: str) -> str:
    stripped = value.strip()
    if len(stripped) >= 2 and stripped[0] == stripped[-1] and (stripped[0] in {"'", '"'}):
        return stripped[1:-1]
    return stripped

def scalar_mapping(block_lines: list[str]) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in block_lines:
        clean = strip_comment(line).strip()
        match = re.fullmatch('([A-Za-z-]+):\\s*(.+)', clean)
        if match:
            values[match.group(1)] = yaml_scalar(match.group(2))
    return values

def workflow_trigger_block(workflow_text: str, trigger: str) -> list[str]:
    on_block = top_level_block(workflow_text, 'on')
    trigger_line = f'  {trigger}:'
    for index, line in enumerate(on_block):
        if line.strip() != trigger_line.strip():
            continue
        block: list[str] = []
        for child in on_block[index + 1:]:
            if re.match('^  [^ \\t:#][^:#]*:', child):
                break
            block.append(child)
        return block
    return []
CI_POLICY_SHELL_COMMAND_BOUNDARIES = {';', '&', '&&', '||', '|', '(', '{', ')', '}'}

def command_segments(tokens: list[str]) -> list[list[str]]:
    segments: list[list[str]] = []
    current: list[str] = []
    for token in tokens:
        if token in CI_POLICY_SHELL_COMMAND_BOUNDARIES:
            if current:
                segments.append(current)
                current = []
            continue
        current.append(token)
    if current:
        segments.append(current)
    return segments

def step_blocks(job_lines: list[str]) -> list[list[str]]:
    blocks: list[list[str]] = []
    current: list[str] | None = None
    in_steps = False
    steps_indent: int | None = None
    step_indent: int | None = None
    for line in job_lines:
        clean = strip_comment(line)
        stripped = clean.lstrip()
        if not in_steps:
            if re.match('^\\s*steps:\\s*$', clean):
                in_steps = True
                steps_indent = len(clean) - len(stripped)
            continue
        if not stripped:
            if current is not None:
                current.append(line)
            continue
        indent = len(clean) - len(stripped)
        is_step_item = YAML_STEP_ITEM_RE.match(stripped) is not None
        if steps_indent is not None and indent <= steps_indent and (not (indent == steps_indent and is_step_item)):
            break
        if step_indent is None and is_step_item:
            step_indent = indent
        if step_indent is not None and indent == step_indent and is_step_item:
            if current is not None:
                blocks.append(current)
            current = [line]
            continue
        if current is not None:
            current.append(line)
    if current is not None:
        blocks.append(current)
    return blocks

def setup_action_blocks(job_lines: list[str]) -> list[list[str]]:
    return [block for block in step_blocks(job_lines) if any(('./.github/actions/setup-environment' in line for line in block))]

def block_input_items(block: list[str]) -> list[tuple[str, str]]:
    items: list[tuple[str, str]] = []
    with_indent: int | None = None
    input_indent: int | None = None
    for line in block:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        if with_indent is None:
            match = re.match(f'^(\\s*)({YAML_KEY_PATTERN})\\s*:\\s*$', clean)
            if match is not None and unquote_yaml_scalar(match.group(2)) == 'with':
                with_indent = len(match.group(1))
                input_indent = with_indent + 2
            continue
        indent = len(clean) - len(clean.lstrip(' '))
        if indent <= with_indent:
            break
        if indent != input_indent:
            continue
        match = re.match(f'^\\s{{{input_indent}}}({YAML_KEY_PATTERN})\\s*:\\s*(.*)$', clean)
        if match is not None:
            items.append((unquote_yaml_scalar(match.group(1)), match.group(2).strip()))
    return items

def block_has_input(block: list[str], name: str, value: str | None=None) -> bool:
    expected = None if value is None else unquote_yaml_scalar(value)
    for item_name, item_value in block_input_items(block):
        if item_name != name:
            continue
        if expected is None or unquote_yaml_scalar(item_value) == expected:
            return True
    return False

def job_has_setup_input(job_lines: list[str], name: str, value: str | None=None) -> bool:
    return any((block_has_input(block, name, value) for block in setup_action_blocks(job_lines)))

def step_if_condition(block: list[str]) -> str | None:
    for line in block:
        clean = strip_comment(line).strip()
        if clean.startswith('if:'):
            return clean[3:].strip()
    return None

def block_has_cargo_build_jobs_compile_command(block: list[str]) -> bool:
    return CARGO_BUILD_JOBS_COMPILE_COMMAND_RE.search(uncommented_text(block)) is not None

def cargo_build_jobs_setup_order_errors(job_lines: list[str], expected_key: str) -> list[str]:
    setup_conditions: set[str | None] = set()
    for block in step_blocks(job_lines):
        if any(('./.github/actions/setup-environment' in line for line in block)) and block_has_input(block, 'build-jobs-key', expected_key):
            setup_conditions.add(step_if_condition(block))
            continue
        if not block_has_cargo_build_jobs_compile_command(block):
            continue
        compile_condition = step_if_condition(block)
        if None in setup_conditions or compile_condition in setup_conditions:
            continue
        if setup_conditions:
            return ['build-jobs-key setup-environment step must be unconditional or match the cargo/just compile step condition']
        return ['build-jobs-key setup-environment step must run before cargo/just compile commands']
    return []

def uncommented_text(lines: list[str]) -> str:
    return '\n'.join((strip_comment(line) for line in lines))

def block_step_property_indent(block: list[str]) -> int | None:
    for line in block:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        match = re.match(f'^(\\s*)-\\s*(?:{YAML_ANCHOR_PATTERN}\\s+)?{YAML_KEY_PATTERN}\\s*:\\s*.*$', clean)
        if match is None:
            return None
        return len(match.group(1)) + 2
    return None

def block_top_level_items(block: list[str]) -> dict[str, str] | None:
    property_indent = block_step_property_indent(block)
    if property_indent is None:
        return None
    step_item_indent = property_indent - 2
    items: dict[str, str] = {}
    for line in block:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        step_match = re.match(f'^(\\s*)-\\s*(?:{YAML_ANCHOR_PATTERN}\\s+)?({YAML_KEY_PATTERN})\\s*:\\s*(.*?)\\s*$', clean)
        if step_match is not None:
            if len(step_match.group(1)) != step_item_indent:
                continue
            key = unquote_yaml_scalar(step_match.group(2))
            value = step_match.group(3)
        else:
            indent = len(clean) - len(clean.lstrip(' '))
            if indent != property_indent:
                continue
            item_match = re.match(f'^\\s*({YAML_KEY_PATTERN})\\s*:\\s*(.*?)\\s*$', clean)
            if item_match is None:
                return None
            key = unquote_yaml_scalar(item_match.group(1))
            value = item_match.group(2)
        if key in items:
            return None
        items[key] = unquote_yaml_scalar(value)
    return items

def block_nested_mapping_items(block: list[str], parent_key: str) -> dict[str, str] | None:
    property_indent = block_step_property_indent(block)
    if property_indent is None:
        return None
    parent_indent: int | None = None
    item_indent: int | None = None
    items: dict[str, str] = {}
    for line in block:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        indent = len(clean) - len(clean.lstrip(' '))
        if parent_indent is None:
            parent_match = re.match(f'^\\s*({YAML_KEY_PATTERN})\\s*:\\s*(.*?)\\s*$', clean)
            if parent_match is not None and indent == property_indent and (unquote_yaml_scalar(parent_match.group(1)) == parent_key) and (unquote_yaml_scalar(parent_match.group(2)) == ''):
                parent_indent = indent
            continue
        if indent <= parent_indent:
            break
        if item_indent is None:
            item_indent = indent
        if indent != item_indent:
            continue
        item_match = re.match(f'^\\s*({YAML_KEY_PATTERN})\\s*:\\s*(.*?)\\s*$', clean)
        if item_match is None:
            return None
        key = unquote_yaml_scalar(item_match.group(1))
        if key in items:
            return None
        items[key] = unquote_yaml_scalar(item_match.group(2))
    return items

def job_if_value(job_lines: list[str]) -> str:
    for index, line in enumerate(job_lines):
        clean = strip_comment(line).rstrip()
        match = re.match('^    if:\\s*(?P<value>.*?)\\s*$', clean)
        if match is not None:
            value = match.group('value')
            if value in {'>-', '>+', '>', '|-', '|+', '|'}:
                value = ''
            child_values: list[str] = []
            for child in job_lines[index + 1:]:
                child_clean = strip_comment(child).rstrip()
                if not child_clean.strip():
                    continue
                indent = len(child_clean) - len(child_clean.lstrip(' '))
                if indent <= 4:
                    break
                child_values.append(child_clean.strip())
            if child_values:
                return '\n'.join([value, *child_values])
            return value
    return ''

def tokens_are_rust_version_probe(tokens: list[str]) -> bool:
    if not tokens:
        return False
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index:
        return tokens_are_rust_version_probe(tokens[assignment_index:])
    executable = pathlib.Path(tokens[0]).name
    if executable == 'cargo':
        command_index = consume_cargo_global_options(tokens, 1)
        probe_commands = {'--version', '-V', 'version', '--help', '-h', 'help'}
        return command_index < len(tokens) and tokens[command_index] in probe_commands
    if raw_rust_tool_token(executable):
        return any((token in {'--version', '-V', '--help', '-h'} for token in tokens[1:]))
    return False

def tokens_have_repo_automation_raw_cargo(tokens: list[str], *, variables: dict[str, str] | None=None) -> bool:
    if not tokens:
        return False
    variables = variables or {}
    for payload in shell_command_substitution_payloads(tokens):
        if tokens_have_raw_cargo_launch(payload, variables=variables):
            return True
    array_assignments, array_assignment_index = shell_array_assignment_values_from_tokens(tokens)
    if array_assignments and array_assignment_index == len(tokens):
        return array_assignment_values_have_cargo_executable(array_assignments)
    if any((token in SHELL_COMMAND_BOUNDARIES for token in tokens)):
        segment: list[str] = []
        segment_variables = dict(variables)
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                assignments, is_persistent_assignment = persistent_shell_assignment_values(segment)
                if is_persistent_assignment:
                    array_assignments, array_assignment_index = shell_array_assignment_values_from_tokens(segment)
                    if array_assignments and array_assignment_index == len(segment) and array_assignment_values_have_cargo_executable(array_assignments):
                        return True
                    segment_variables.update(assignments)
                    segment = []
                    continue
                if tokens_have_repo_automation_raw_cargo(segment, variables=segment_variables):
                    return True
                segment = []
                continue
            segment.append(token)
        return tokens_have_repo_automation_raw_cargo(segment, variables=segment_variables)
    if tokens_are_rust_version_probe(tokens):
        return False
    return tokens_have_raw_cargo_launch(tokens, variables=variables)

def array_assignment_values_have_cargo_executable(assignments: dict[str, str]) -> bool:
    return any((tokens_have_cargo_executable_launch(command_tokens(value)) for value in assignments.values()))

def tokens_have_cargo_executable_launch(tokens: list[str], *, depth: int=0) -> bool:
    if depth > 6:
        return True
    tokens = strip_shell_redirections(tokens)
    if not tokens:
        return False
    if any((token in SHELL_COMMAND_BOUNDARIES for token in tokens)):
        segment: list[str] = []
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                if tokens_have_cargo_executable_launch(segment, depth=depth + 1):
                    return True
                segment = []
                continue
            segment.append(token)
        return tokens_have_cargo_executable_launch(segment, depth=depth + 1)
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index:
        return assignment_index < len(tokens) and tokens_have_cargo_executable_launch(tokens[assignment_index:], depth=depth + 1)
    executable = pathlib.Path(tokens[0]).name
    if executable in RECURSIVE_WRAPPER_EXECUTABLES:
        inner = wrapper_inner_tokens(tokens)
        if inner is not None:
            return tokens_have_cargo_executable_launch(inner, depth=depth + 1)
    if executable == 'env':
        inner = env_inner_tokens(tokens)
        if inner is not None:
            return tokens_have_cargo_executable_launch(inner, depth=depth + 1)
    return executable == 'cargo'

def is_managed_just_recipe_guard(recipe: str, stripped_line: str) -> bool:
    expected = f'if [ "${{BOLT_MANAGED_JUST:-}}" != "1" ]; then echo "ERROR: {recipe} must run through scripts/rust_verification.py run"; exit 2; fi'
    return stripped_line == expected

def is_allowed_managed_just_recipe_command(recipe: str, stripped_line: str) -> bool:
    allowed_commands = {'managed-build': 'cargo zigbuild --release --target {{target}} --locked', 'managed-clippy': 'cargo clippy --locked -- -D warnings'}
    return stripped_line == allowed_commands.get(recipe)

def repo_automation_raw_cargo_errors(file_name: str, text: str) -> list[str]:
    errors: list[str] = []
    managed_just_recipe = False
    current_just_recipe = ''
    shell_variables: dict[str, str] = {}
    is_justfile = file_name == 'justfile' or file_name.startswith('justfile.')
    for line in shell_logical_lines(text):
        stripped = strip_comment(line).strip()
        if not stripped:
            continue
        if is_justfile and (not line[:1].isspace()):
            if stripped.startswith('['):
                continue
            if ':' in stripped and ':=' not in stripped:
                recipe = stripped.split(':', 1)[0].strip()
                current_just_recipe = recipe.split()[0] if recipe else ''
                managed_just_recipe = False
                continue
        if is_justfile and current_just_recipe in {'managed-build', 'managed-clippy'} and is_managed_just_recipe_guard(current_just_recipe, stripped):
            managed_just_recipe = True
            continue
        if is_justfile and managed_just_recipe:
            if is_allowed_managed_just_recipe_command(current_just_recipe, stripped):
                continue
        tokens = command_tokens(stripped)
        if tokens_have_repo_automation_raw_cargo(tokens, variables=shell_variables):
            errors.append('repo automation raw Cargo must use managed rust_verification wrapper')
            break
        assignments, is_persistent_assignment = persistent_shell_assignment_values(tokens)
        if is_persistent_assignment:
            shell_variables.update(assignments)
            continue
        tokens = expand_known_shell_assignment_names(tokens, shell_variables)
        tokens = expand_known_shell_command_variables(tokens, shell_variables)
        if tokens_have_repo_automation_raw_cargo(tokens, variables=shell_variables):
            errors.append('repo automation raw Cargo must use managed rust_verification wrapper')
            break
    return errors

def text_raw_cargo_storage_override_messages(text: str) -> set[str]:
    messages: set[str] = set()
    aliases: set[str] = set()
    variables: dict[str, str] = {}
    for line in shell_logical_lines(text):
        stripped = strip_comment(line).strip()
        if not stripped:
            continue
        tokens = command_tokens(stripped)
        messages.update(raw_cargo_storage_override_messages_from_tokens(tokens, aliases=aliases, variables=variables))
        shell_assignments, is_persistent_assignment = persistent_shell_assignment_values(tokens)
        if is_persistent_assignment:
            variables.update(shell_assignments)
        segment: list[str] = []
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                if segment and segment[0] == 'alias':
                    aliases.update(simple_cargo_aliases(segment, aliases))
                segment = []
                continue
            segment.append(token)
        if segment and segment[0] == 'alias':
            aliases.update(simple_cargo_aliases(segment, aliases))
    return messages

def strip_yaml_anchor(value: str) -> tuple[str | None, str]:
    match = re.match('&([A-Za-z0-9_.-]+)(?:\\s+|$)(.*)', value)
    if match is None:
        return (None, value)
    return (match.group(1), match.group(2).strip())

def resolve_no_mistakes_scalar(value: str, anchors: dict[str, str]) -> tuple[str, str | None]:
    value = value.strip()
    alias = re.fullmatch('\\*([A-Za-z0-9_.-]+)', value)
    if alias is not None:
        return (anchors.get(alias.group(1), value), None)
    anchor, value = strip_yaml_anchor(value)
    if len(value) >= 2 and value[0] == value[-1] and (value[0] in ("'", '"')):
        value = value[1:-1]
    return (value, anchor)

def record_no_mistakes_anchor_from_scalar(value: str, anchors: dict[str, str]) -> None:
    value = value.strip()
    if value.startswith('-'):
        value = value[1:].strip()
    value, anchor = resolve_no_mistakes_scalar(value, anchors)
    if anchor is not None:
        anchors[anchor] = value

def no_mistakes_anchor_candidate(value: str) -> tuple[str | None, str]:
    value = value.strip()
    if value.startswith('-'):
        value = value[1:].strip()
    return strip_yaml_anchor(value)

def no_mistakes_commands(config_text: str) -> dict[str, str]:
    commands: dict[str, str] = {}
    anchors: dict[str, str] = {}
    in_commands = False
    lines = config_text.splitlines()
    index = 0
    while index < len(lines):
        raw_line = lines[index]
        line = strip_comment(raw_line).rstrip()
        stripped = line.strip()
        if not stripped or stripped.startswith('#'):
            index += 1
            continue
        indent = len(line) - len(line.lstrip(' '))
        if indent == 0:
            name, separator, value = stripped.partition(':')
            in_commands = bool(separator) and name.strip() == 'commands' and (not value.strip() or value.strip().startswith('#'))
            if separator:
                record_no_mistakes_anchor_from_scalar(value, anchors)
            index += 1
            continue
        if not in_commands:
            _, separator, value = stripped.partition(':')
            candidate_value = value if separator else stripped
            anchor, stripped_value = no_mistakes_anchor_candidate(candidate_value)
            if anchor is not None and (stripped_value in ('|', '>') or stripped_value.startswith(('|', '>'))):
                block_lines: list[str] = []
                index += 1
                while index < len(lines):
                    candidate = lines[index].rstrip()
                    candidate_stripped = candidate.strip()
                    if not candidate_stripped or candidate_stripped.startswith('#'):
                        index += 1
                        continue
                    candidate_indent = len(candidate) - len(candidate.lstrip(' '))
                    if candidate_indent <= indent:
                        break
                    block_lines.append(candidate_stripped)
                    index += 1
                anchors[anchor] = '\n'.join(block_lines).strip()
                continue
            record_no_mistakes_anchor_from_scalar(candidate_value, anchors)
            index += 1
            continue
        if indent <= 2 and ':' in stripped:
            name, _, value = stripped.partition(':')
            value = value.strip()
            anchor, stripped_value = strip_yaml_anchor(value)
            if anchor is not None:
                value = stripped_value
            if value in ('|', '>') or value.startswith(('|', '>')):
                block_lines: list[str] = []
                index += 1
                while index < len(lines):
                    candidate = lines[index].rstrip()
                    candidate_stripped = candidate.strip()
                    if not candidate_stripped or candidate_stripped.startswith('#'):
                        index += 1
                        continue
                    candidate_indent = len(candidate) - len(candidate.lstrip(' '))
                    if candidate_indent <= indent:
                        break
                    block_lines.append(candidate_stripped)
                    index += 1
                command = '\n'.join(block_lines).strip()
                commands[name.strip()] = command
                if anchor is not None:
                    anchors[anchor] = command
                continue
            scalar_parts = [value]
            index += 1
            while index < len(lines):
                candidate = lines[index].rstrip()
                candidate_stripped = candidate.strip()
                if not candidate_stripped or candidate_stripped.startswith('#'):
                    index += 1
                    continue
                candidate_indent = len(candidate) - len(candidate.lstrip(' '))
                if candidate_indent <= indent:
                    break
                scalar_parts.append(candidate_stripped)
                index += 1
            value = ' '.join((part for part in scalar_parts if part)).strip()
            value, scalar_anchor = resolve_no_mistakes_scalar(value if anchor is None else f'&{anchor} {value}', anchors)
            if scalar_anchor is not None:
                anchors[scalar_anchor] = value
            commands[name.strip()] = value
            continue
        index += 1
    return commands

def no_mistakes_command_section_errors(config_text: str, config_name: str) -> list[str]:
    errors: list[str] = []
    for raw_line in config_text.splitlines():
        line = raw_line.rstrip()
        stripped = line.strip()
        if not stripped or stripped.startswith('#'):
            continue
        indent = len(line) - len(line.lstrip(' '))
        if indent != 0:
            continue
        name, separator, value = stripped.partition(':')
        if not separator or name.strip() != 'commands':
            continue
        value = value.strip()
        if value and (not value.startswith('#')):
            errors.append(f'{config_name} commands section must use block mapping')
    return errors

def command_has_managed_compile_heavy_invocation(command: str) -> bool:
    for raw_line in command.splitlines() or [command]:
        tokens = command_tokens(raw_line)
        normalized_tokens = managed_rust_verification_command_tokens(tokens)
        if normalized_tokens is None:
            continue
        managed_args = managed_rust_verification_cargo_args(tokens)
        if not managed_args:
            continue
        subcommand = cargo_subcommand(managed_args)
        if normalized_tokens[2] == 'run' and subcommand in LOCAL_COMPILE_REFUSED_MANAGED_COMMANDS:
            return True
        if normalized_tokens[2] == 'cargo' and subcommand in LOCAL_COMPILE_REFUSED_CARGO_SUBCOMMANDS:
            return True
    return False

def verify_no_mistakes_config(config_text: str, config_name: str='.no-mistakes.yaml') -> list[str]:
    errors: list[str] = no_mistakes_command_section_errors(config_text, config_name)
    for command_name, command in no_mistakes_commands(config_text).items():
        command_segments = [command, *command.splitlines()]
        storage_errors = raw_rust_storage_errors(command)
        if any((command_has_raw_cargo(segment) for segment in command_segments if segment.strip())) or any(('BOLT_MANAGED_JUST private just recipe bypass' in error for error in storage_errors)):
            errors.append(f'{config_name} commands.{command_name} raw Cargo drift must be classified')
        if command_has_managed_compile_heavy_invocation(command):
            errors.append(f'{config_name} commands.{command_name} wrapper-routed local compile-heavy Rust must be remote-first')
        for storage_error in storage_errors:
            if storage_error == 'BOLT_MANAGED_JUST private just recipe bypass must be classified':
                continue
            errors.append(f'{config_name} commands.{command_name} {storage_error}')
    return errors

def string_set(table: dict[str, object], key: str) -> set[str] | None:
    value = table.get(key)
    if not isinstance(value, list) or not all((isinstance(item, str) for item in value)):
        return None
    return set(value)

def local_compile_policy_errors(data: dict[str, object], display_name: str) -> list[str]:
    policy = data.get('local_compile_policy')
    if not isinstance(policy, dict):
        return [f'{display_name} must define [local_compile_policy]']
    errors: list[str] = []
    if policy.get('enabled') is not True:
        errors.append(f'{display_name} local_compile_policy.enabled must be true')
    if policy.get('allowed_ci_env') != 'GITHUB_ACTIONS':
        errors.append(f'{display_name} local_compile_policy.allowed_ci_env must be GITHUB_ACTIONS')
    if policy.get('break_glass_env') != 'BOLT_ALLOW_LOCAL_RUST':
        errors.append(f'{display_name} local_compile_policy.break_glass_env must be BOLT_ALLOW_LOCAL_RUST')
    if string_set(policy, 'refused_managed_commands') != LOCAL_COMPILE_REFUSED_MANAGED_COMMANDS:
        errors.append(f'{display_name} local_compile_policy.refused_managed_commands must be build/clippy/test')
    if string_set(policy, 'refused_cargo_subcommands') != LOCAL_COMPILE_REFUSED_CARGO_SUBCOMMANDS:
        errors.append(f'{display_name} local_compile_policy.refused_cargo_subcommands must match disk preflight and aliases')
    return errors

def load_rust_verification_policy_toml(path: pathlib.Path, display_name: str) -> dict[str, object]:
    try:
        return tomllib.loads(path.read_text(encoding='utf-8'))
    except FileNotFoundError:
        raise
    except tomllib.TOMLDecodeError as exc:
        raise PolicyError(f'{display_name} is invalid TOML: {exc}') from exc
    except OSError as exc:
        raise PolicyError(f'{display_name} could not be read: {exc}') from exc

def verify_rust_verification_policy(path: pathlib.Path) -> list[str]:
    display_name = path.relative_to(REPO_ROOT).as_posix()
    try:
        data = load_rust_verification_policy_toml(path, display_name)
    except FileNotFoundError:
        return [f'{display_name} is required']
    except PolicyError as exc:
        return [str(exc)]
    errors: list[str] = []
    if data.get('schema_version') != 2:
        errors.append(f'{display_name} schema_version must be 2')
    errors.extend(local_compile_policy_errors(data, display_name))
    return errors

def verify_rust_verification_policies() -> list[str]:
    errors: list[str] = []
    errors.extend(verify_rust_verification_policy(DEFAULT_RUST_VERIFICATION_POLICY))
    errors.extend(verify_rust_verification_policy(DEFAULT_BVS_RUST_VERIFICATION_POLICY))
    return errors

def alias_payload_storage_messages(text: str, *, depth: int=0) -> set[str]:
    if depth > 4:
        return set()
    messages: set[str] = set()
    segment: list[str] = []
    for token in command_tokens(text) + [';']:
        if token in SHELL_COMMAND_BOUNDARIES:
            if segment and pathlib.Path(segment[0]).name == 'alias':
                for payload in shell_alias_payloads(segment).values():
                    messages.update(raw_rust_storage_errors(payload, alias_depth=depth + 1))
            segment = []
            continue
        segment.append(token)
    return messages

def text_has_alias_cargo_target_routing_override(text: str) -> bool:
    cargo_aliases: set[str] = set()
    for line in text.splitlines():
        if not line.strip():
            continue
        tokens = command_tokens(line)
        segment: list[str] = []
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                if segment and segment[0] == 'alias':
                    cargo_aliases.update(simple_cargo_aliases(segment, cargo_aliases))
                elif any((token in cargo_aliases for token in segment)):
                    expanded = expand_cargo_aliases(segment, cargo_aliases)
                    if tokens_have_target_routing_override(expanded) and tokens_have_raw_cargo_launch(expanded):
                        return True
                segment = []
                continue
            segment.append(token)
        if segment and segment[0] == 'alias':
            cargo_aliases.update(simple_cargo_aliases(segment, cargo_aliases))
            continue
        if not any((token in cargo_aliases for token in segment)):
            continue
        expanded = expand_cargo_aliases(segment, cargo_aliases)
        if tokens_have_target_routing_override(expanded) and tokens_have_raw_cargo_launch(expanded):
            return True
    return False

def folded_yaml_run_commands(text: str) -> list[str]:
    lines = text.splitlines()
    commands: list[str] = []
    index = 0
    while index < len(lines):
        line = lines[index]
        match = YAML_FOLDED_RUN_LINE_RE.match(line)
        if match is None:
            index += 1
            continue
        base_indent = len(match.group(1))
        block: list[str] = []
        index += 1
        while index < len(lines):
            candidate = lines[index]
            if not candidate.strip():
                index += 1
                continue
            indent = len(candidate) - len(candidate.lstrip(' '))
            if indent <= base_indent:
                break
            block.append(candidate.strip())
            index += 1
        if block:
            commands.append(' '.join(block))
    return commands

def step_run_command(block: list[str]) -> str | None:
    for index, line in enumerate(block):
        clean = strip_comment(line).rstrip()
        match = YAML_RUN_LINE_RE.match(clean)
        if match is None:
            continue
        value = match.group(2).strip()
        if not value:
            return ''
        if value[0] not in {'|', '>'}:
            return unquote_yaml_scalar(value)
        folded = value[0] == '>'
        base_indent = len(match.group(1))
        raw_command_lines: list[str] = []
        for nested in block[index + 1:]:
            nested_clean = strip_comment(nested).rstrip()
            if not nested_clean.strip():
                continue
            indent = len(nested_clean) - len(nested_clean.lstrip(' '))
            if indent <= base_indent:
                break
            raw_command_lines.append(nested_clean)
        command_indent = min((len(command) - len(command.lstrip(' ')) for command in raw_command_lines), default=base_indent + 1)
        command_lines = [command[command_indent:] if command.startswith(' ' * command_indent) else command.lstrip(' ') for command in raw_command_lines]
        if folded:
            return ' '.join((command.strip() for command in command_lines))
        return '\n'.join(command_lines)
    return None

def yaml_run_shell_texts(yaml_text: str) -> list[str]:
    lines = yaml_text.splitlines()
    texts: list[str] = []
    index = 0
    while index < len(lines):
        clean = strip_comment(lines[index]).rstrip()
        match = YAML_RUN_LINE_RE.match(clean)
        if match is None:
            index += 1
            continue
        value = match.group(2).strip()
        if not value:
            texts.append('')
            index += 1
            continue
        if value[0] not in {'|', '>'}:
            texts.append(unquote_yaml_scalar(value))
            index += 1
            continue
        folded = value[0] == '>'
        base_indent = len(match.group(1))
        command_lines: list[str] = []
        index += 1
        while index < len(lines):
            nested_clean = strip_comment(lines[index]).rstrip()
            if not nested_clean.strip():
                index += 1
                continue
            indent = len(nested_clean) - len(nested_clean.lstrip(' '))
            if indent <= base_indent:
                break
            command_lines.append(nested_clean.strip())
            index += 1
        texts.append(' '.join(command_lines) if folded else '\n'.join(command_lines))
    return texts

def workflow_run_shell_texts(workflow_text: str) -> list[str]:
    texts: list[str] = []
    step_scopes = list(parse_jobs(workflow_text).values())
    runs_block = top_level_block(workflow_text, 'runs')
    if any(((match := re.match('^\\s*using:\\s*(.*?)\\s*$', strip_comment(line))) and unquote_yaml_scalar(match.group(1).strip()) == 'composite' for line in runs_block)):
        step_scopes.append(runs_block)
    for job_lines in step_scopes:
        persisted_env: dict[str, str] = {}
        for block in step_blocks(job_lines):
            command = step_run_command(block)
            if command is None:
                continue
            parts = [f'{name}={value}' for name, value in persisted_env.items()]
            if command.strip():
                parts.append(command)
            texts.append('\n'.join(parts))
            for assignment in github_env_assignment_lines(command):
                name, separator, value = assignment.partition('=')
                if separator and re.fullmatch('[A-Za-z_][A-Za-z0-9_]*', name):
                    persisted_env[name] = value
    return texts

def add_unique_errors(errors: list[str], messages: Iterable[str]) -> None:
    for message in messages:
        if message not in errors:
            errors.append(message)

def raw_rust_storage_errors(workflow_text: str, *, alias_depth: int=0) -> list[str]:
    uncommented = uncommented_text(workflow_text.splitlines())
    folded_command_texts = folded_yaml_run_commands(uncommented)
    yaml_command_texts = yaml_run_shell_texts(uncommented)
    folded_commands = '\n'.join(folded_command_texts)
    text = re.sub('\\\\\\s*\\n\\s*', ' ', '\n'.join((part for part in (uncommented, folded_commands) if part)))
    shell_texts = workflow_run_shell_texts(uncommented)
    if not shell_texts:
        shell_texts = [uncommented]
    shell_texts.extend(folded_command_texts)
    shell_texts.extend(yaml_command_texts)
    shell_texts = [re.sub('\\\\\\s*\\n\\s*', ' ', shell_text) for shell_text in shell_texts]
    checks: tuple[tuple[str, str], ...] = (('(^|[^A-Za-z0-9_])[\\"\']?CARGO_TARGET_DIR[\\"\']?\\s*(?:=|:)', 'CARGO_TARGET_DIR raw target override must be classified'), ('(^|[^A-Za-z0-9_])[\\"\']?CARGO_BUILD_TARGET_DIR[\\"\']?\\s*(?:=|:)', 'CARGO_BUILD_TARGET_DIR raw target override must be classified'), ('(?:target-dir|build\\.target-dir)[^\\n]*>\\s*\\.cargo/config\\.toml|\\.cargo/config\\.toml[^\\n]*(?:target-dir|build\\.target-dir)', '.cargo/config.toml build.target-dir raw target override must be classified'), ('(^|[^A-Za-z0-9_])[\\"\']?CARGO_TARGET_TMPDIR[\\"\']?\\s*(?:=|:)', 'CARGO_TARGET_TMPDIR raw target override must be classified'), ('(^|[^A-Za-z0-9_])[\\"\']?CARGO_INCREMENTAL[\\"\']?\\s*(?:=|:)', 'CARGO_INCREMENTAL raw cache override must be classified'), ('(^|[^A-Za-z0-9_])[\\"\']?CARGO_BUILD_RUSTFLAGS[\\"\']?\\s*(?:=|:).*(?:--out-dir|--artifact-dir)', 'CARGO_BUILD_RUSTFLAGS raw output override must be classified'), ('(^|[^A-Za-z0-9_])[\\"\']?CARGO_ENCODED_RUSTFLAGS[\\"\']?\\s*(?:=|:).*(?:--out-dir|--artifact-dir)', 'CARGO_ENCODED_RUSTFLAGS raw output override must be classified'), ('(^|[^A-Za-z0-9_])[\\"\']?CARGO_INSTALL_ROOT[\\"\']?\\s*(?:=|:)', 'CARGO_INSTALL_ROOT install output override must be classified'), ('(^|[^A-Za-z0-9_])[\\"\']?CARGO_HOME[\\"\']?\\s*(?:=|:)', 'CARGO_HOME raw cache override must be classified'), ('(^|[^A-Za-z0-9_])[\\"\']?RUSTUP_HOME[\\"\']?\\s*(?:=|:)', 'RUSTUP_HOME raw toolchain override must be classified'), ('(^|[^A-Za-z0-9_])[\\"\']?RUSTFLAGS[\\"\']?\\s*(?:=|:).*(?:--out-dir|--artifact-dir)', 'RUSTFLAGS raw output override must be classified'), ('(^|[^A-Za-z0-9_])[\\"\']?RUSTC_WRAPPER[\\"\']?\\s*(?:=|:)', 'RUSTC_WRAPPER raw compiler wrapper must be classified'), ('(^|[^A-Za-z0-9_])[\\"\']?RUSTC_WORKSPACE_WRAPPER[\\"\']?\\s*(?:=|:)', 'RUSTC_WORKSPACE_WRAPPER raw compiler wrapper must be classified'), ('(^|[^A-Za-z0-9_$\\{])[\\"\']?BOLT_ALLOW_LOCAL_RUST[\\"\']?\\s*(?:=|:|<<)', 'BOLT_ALLOW_LOCAL_RUST local Rust break-glass must not be checked in'), ('(^|[^A-Za-z0-9_$\\{])[\\"\']?BOLT_MANAGED_JUST[\\"\']?\\s*(?:=|:|<<)', 'BOLT_MANAGED_JUST private just recipe bypass must be classified'), ('(^|[^A-Za-z0-9_$\\{])[\\"\']?GITHUB_ACTIONS[\\"\']?\\s*(?:=|:|<<)', 'GITHUB_ACTIONS local CI spoof must not be checked in'), ('\\bno-mistakes\\b[^\\n]*\\bcargo\\b', 'no-mistakes raw Cargo drift must be classified'), ('\\bno-mistakes\\b[^\\n]*--worktree[^\\n]*(?:--target-dir\\s+target|\\btarget\\b)', 'no-mistakes worktree-local target path evidence must be reported'), ('\\bcargo\\b[^\\n|]*\\$@[^|]*\\|\\s*bash\\b[^\\n;&|]*\\s-s\\b[^\\n;&|]*\\s--target-dir\\b', 'cargo --target-dir raw target override must be classified'))
    errors: list[str] = []
    for pattern, message in checks:
        if re.search(pattern, text):
            errors.append(message)
    for shell_text in shell_texts:
        add_unique_errors(errors, sorted(text_raw_cargo_storage_override_messages(shell_text)))
        add_unique_errors(errors, sorted(dynamic_env_target_override_messages(shell_text)))
        add_unique_errors(errors, sorted(alias_payload_storage_messages(shell_text, depth=alias_depth)))
    config_file_message = 'cargo --config file raw target override must be classified'
    if text_has_path_style_cargo_config(text) or any((text_has_path_style_cargo_config(shell_text) for shell_text in shell_texts)):
        add_unique_errors(errors, [config_file_message])
    target_override_message = 'cargo --target-dir raw target override must be classified'
    if any((text_has_alias_cargo_target_routing_override(shell_text) for shell_text in shell_texts)):
        add_unique_errors(errors, [target_override_message])
    for shell_text in shell_texts:
        add_unique_errors(errors, storage_transfer_policy_errors(shell_text))
    return errors

def rust_text_has_test_attr(masked_text: str) -> bool:
    return RUST_TEST_ATTR_RE.search(masked_text) is not None

def rust_inner_attr_is_banned(attr_name: str) -> bool:
    return attr_name in BANNED_RUST_INNER_ATTRS or attr_name.startswith('crate_')

def format_banned_inner_attr(attr_name: str) -> str:
    return f'#![{attr_name}(...)]'

def test_manifest_referenced_by(manifest: CiTestManifest) -> dict[str, list[str]]:
    referenced_by: dict[str, list[str]] = {}
    for harness, members in manifest.harness_to_members.items():
        for member in members:
            if member == harness:
                continue
            referenced_by.setdefault(member, []).append(harness)
    return referenced_by

def verify_test_harness_manifest(*, cargo_manifest_path: pathlib.Path | str | None=None, tests_root: pathlib.Path | str | None=None, workflow_path: pathlib.Path | str | None=None, justfile_path: pathlib.Path | str | None=None) -> list[str]:
    cargo_manifest = pathlib.Path(cargo_manifest_path) if cargo_manifest_path is not None else REPO_ROOT / 'Cargo.toml'
    root = pathlib.Path(tests_root) if tests_root is not None else REPO_ROOT / 'tests'
    workflow = pathlib.Path(workflow_path) if workflow_path is not None else DEFAULT_WORKFLOW
    justfile = pathlib.Path(justfile_path) if justfile_path is not None else REPO_ROOT / 'justfile'
    errors: list[str] = []
    try:
        with cargo_manifest.open('rb') as handle:
            cargo_config = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        return [f'{cargo_manifest.name} could not be parsed for explicit test harness governance: {exc}']
    package = cargo_config.get('package')
    if not isinstance(package, dict) or package.get('autotests') is not False:
        errors.append(f'{cargo_manifest.name} [package].autotests must be false for explicit test harnesses')
    try:
        manifest = build_test_manifest(cargo_manifest, root)
    except Exception as exc:
        errors.append(f'{cargo_manifest.name} explicit test harness manifest could not be built: {exc}')
        return errors
    harness_roots = set(manifest.harness_to_members)
    referenced_by = test_manifest_referenced_by(manifest)
    for harness, members in manifest.harness_to_members.items():
        for member in members:
            if member in harness_roots and member != harness:
                errors.append(f'tests/{member}.rs is a harness root and must not be mod-ed by harness {harness}')
    for stem, harnesses in sorted(referenced_by.items()):
        if len(harnesses) <= 1:
            continue
        unique_harnesses = sorted(set(harnesses))
        test_path = root / f'{stem}.rs'
        if len(unique_harnesses) == 1:
            errors.append(f'{test_path.relative_to(root.parent).as_posix()} is registered multiple times by harness {unique_harnesses[0]}')
        else:
            errors.append(f"{test_path.relative_to(root.parent).as_posix()} is registered by multiple harnesses: {', '.join(unique_harnesses)}")
    for test_path in sorted(root.glob('*.rs')):
        stem = test_path.stem
        try:
            masked_text = _mask_rust_non_code(test_path.read_text(encoding='utf-8'))
        except OSError as exc:
            errors.append(f'{test_path.relative_to(root.parent).as_posix()} could not be read: {exc}')
            continue
        rel_path = test_path.relative_to(root.parent).as_posix()
        has_test_attr = rust_text_has_test_attr(masked_text)
        if stem not in harness_roots:
            for attr_name in RUST_INNER_ATTR_RE.findall(masked_text):
                if rust_inner_attr_is_banned(attr_name):
                    errors.append(f'{rel_path} uses banned module-level inner attribute {format_banned_inner_attr(attr_name)}')
        if stem in harness_roots:
            continue
        if stem in DECLARED_TOP_LEVEL_TEST_HELPERS:
            if has_test_attr:
                errors.append(f'{rel_path} is declared as a test helper but contains #[test]')
            continue
        harnesses = referenced_by.get(stem, [])
        if has_test_attr:
            if not harnesses:
                errors.append(f'{rel_path} has #[test] but is not registered in any explicit test harness')
            elif len(harnesses) == 1 and manifest.member_to_harness.get(stem) == harnesses[0]:
                continue
            else:
                errors.append(f'{rel_path} has #[test] but is not registered by exactly one explicit test harness')
            continue
        errors.append(f'{rel_path} is neither a harness root, a #[test]-bearing registered member, nor a declared test helper')
    for file_name, path in (('justfile', justfile),):
        if not path.exists():
            continue
        errors.extend(verify_test_harness_test_args(file_name, path.read_text(encoding='utf-8'), manifest))
    return errors

def verify_test_harness_test_args(file_name: str, text: str, manifest: CiTestManifest) -> list[str]:
    errors: list[str] = []
    harness_roots = set(manifest.harness_to_members)
    for match in re.finditer('[\'\\"]?--test[\'\\"]?(?:=|\\s+)(?P<quote>[\\"\']?)(?P<name>[A-Za-z0-9_-]+)(?P=quote)', text):
        test_name = match.group('name')
        if test_name in harness_roots:
            continue
        harness = manifest.member_to_harness.get(test_name)
        if harness is not None and test_name not in harness_roots:
            errors.append(f'{file_name} references retired integration-test member {test_name!r} with --test; use harness {harness!r}')
        else:
            expected = ', '.join(sorted(harness_roots))
            errors.append(f'{file_name} references unknown integration-test binary {test_name!r} with --test; expected one of: {expected}')
    for line in text.splitlines():
        head = re.search('[\'\\"]?--test[\'\\"]?(?:=|\\s+)[\\"\']?(?P<harness>[A-Za-z0-9_-]+)[\\"\']?', line)
        if head is None or ' -- ' not in line:
            continue
        harness = head.group('harness')
        for pm in re.finditer('\\b(?P<member>[A-Za-z0-9_]+)::', line.split(' -- ', 1)[1]):
            member = pm.group('member')
            owner = manifest.member_to_harness.get(member)
            if owner != harness:
                filt = member + '::'
                errors.append(f'{file_name} source-fence test filter {filt!r} does not belong to --test harness {harness!r} (member maps to {owner!r}); typo or stale member')
    return errors

def repo_automation_source_build_errors(text: str) -> list[str]:
    return [f'repo automation must not compile {tool} from source' for tool in sorted(cargo_install_source_build_tools_in_text(text))]

def normalized_repo_file_name(file_name: str) -> str:
    normalized = file_name.replace('\\', '/')
    while normalized.startswith('./'):
        normalized = normalized[2:]
    repo_root = REPO_ROOT.as_posix()
    if normalized.startswith(repo_root):
        normalized = normalized[len(repo_root):].lstrip('/')
    return normalized

def jules_advisory_workflow_contract_errors(file_name: str, text: str) -> list[str]:
    normalized = normalized_repo_file_name(file_name)
    workflow_path = normalized if normalized.startswith('.github/workflows/') else f'.github/workflows/{normalized}'
    is_allowed_jules_workflow = workflow_path in JULES_ADVISORY_WORKFLOW_PATHS
    errors: list[str] = []
    if JULES_ADVISORY_SECRET in text and (not is_allowed_jules_workflow):
        return ['JULES_API_KEY may only be used by Jules advisory workflows']
    if not is_allowed_jules_workflow:
        return []
    required = (('permissions: {}', 'Jules advisory workflows must use empty permissions'), (f'{JULES_ADVISORY_SECRET}: ${{{{ secrets.{JULES_ADVISORY_SECRET} }}}}', 'Jules advisory workflows must use only the JULES_API_KEY secret'), (f'JULES_SESSIONS_ENDPOINT: ${{{{ vars.{JULES_ADVISORY_ENDPOINT_VARIABLE} }}}}', 'Jules advisory workflows must use configured sessions endpoint variable'), (f'timeout-minutes: ${{{{ fromJSON(vars.{JULES_ADVISORY_TIMEOUT_VARIABLE}) }}}}', 'Jules advisory workflows must use configured session timeout variable'), ('"$JULES_SESSIONS_ENDPOINT"', 'Jules advisory workflows must use configured sessions endpoint variable'), ('automationMode: "AUTO_CREATE_PR"', 'Jules advisory workflows must use Jules PR automation mode'), ('requirePlanApproval: true', 'Jules advisory workflows must require plan approval'), ('continue-on-error: true', 'Jules advisory workflows must remain non-blocking'), ('Create a draft pull request only', 'Jules advisory workflows must constrain Jules to draft PRs'), ('Label any pull request with agent:jules', 'Jules advisory workflows must label Jules PRs'))
    for needle, message in required:
        if needle not in text:
            errors.append(message)
    if 'https://jules.googleapis.com' in text:
        errors.append('Jules advisory workflows must use configured sessions endpoint variable')
    if 'timeout-minutes: 10' in text:
        errors.append('Jules advisory workflows must use configured session timeout variable')
    if 'requirePlanApproval: false' in text:
        errors.append('Jules advisory workflows must require plan approval')
    if 'Verified Jules session evidence' in text:
        errors.append('Jules advisory workflows must not claim verified session evidence on unavailable results')
    secret_refs = set(GITHUB_SECRET_REF_RE.findall(text))
    extra_secrets = secret_refs - {JULES_ADVISORY_SECRET}
    if extra_secrets:
        errors.append('Jules advisory workflows must not reference non-Jules secrets: ' + ', '.join(sorted(extra_secrets)))
    for forbidden in ('github.token', 'GITHUB_TOKEN', 'role-to-assume:', 'aws-actions/'):
        if forbidden in text:
            errors.append('Jules advisory workflows must not use GitHub token or AWS credentials')
            break
    shell_text = '\n'.join(yaml_run_shell_texts(uncommented_text(text.splitlines())))
    if JULES_AWS_COMMAND_RE.search(shell_text) is not None or 'AWS_' in shell_text:
        errors.append('Jules advisory workflows must not use AWS commands')
    success_if = "if: ${{ steps.invoke-jules.outcome == 'success' }}"
    success_notice = '::notice::Jules advisory session started and returned a session id'
    if success_if not in text or success_notice not in text:
        errors.append('Jules advisory workflows must emit verified session notice only on invoke success')
    unavailable_if = "if: ${{ steps.invoke-jules.outcome != 'success' }}"
    unavailable_warning = '::warning::Jules advisory session did not start'
    if unavailable_if not in text or unavailable_warning not in text:
        errors.append('Jules advisory workflows must warn when invocation is unavailable')
    return errors

def verify_repo_automation_texts(texts: dict[str, str]) -> list[str]:
    errors: list[str] = []
    for file_name, text in texts.items():
        errors.extend((f'{file_name}: {error}' for error in raw_rust_storage_errors(text)))
        add_unique_errors(errors, (f'{file_name}: {error}' for error in jules_advisory_workflow_contract_errors(file_name, text)))
        automation_texts = [text, *yaml_run_shell_texts(uncommented_text(text.splitlines()))]
        for automation_text in automation_texts:
            add_unique_errors(errors, (f'{file_name}: {error}' for error in repo_automation_raw_cargo_errors(file_name, automation_text)))
            add_unique_errors(errors, (f'{file_name}: {error}' for error in repo_automation_source_build_errors(automation_text)))
    return errors

def require_config_string(parent: dict[str, object], key: str, prefix: str) -> str:
    value = parent.get(key)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f'{prefix}.{key} must be a non-empty string')
    return value

def require_config_positive_int(parent: dict[str, object], key: str, prefix: str) -> int:
    value = parent.get(key)
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise ValueError(f'{prefix}.{key} must be a positive integer')
    return value

def require_config_string_list(parent: dict[str, object], key: str, prefix: str) -> list[str]:
    value = parent.get(key)
    if not isinstance(value, list) or not all((isinstance(item, str) and item.strip() for item in value)):
        raise ValueError(f'{prefix}.{key} must be a non-empty string list')
    return value

def validate_jules_advisory_config(data: dict[str, object]) -> dict[str, object]:
    section = data.get('jules_advisory')
    if not isinstance(section, dict):
        raise ValueError('ci/github-actions-runners.toml must define [jules_advisory]')
    workflow_paths = require_config_string_list(section, 'workflow_paths', 'jules_advisory')
    if set(workflow_paths) != JULES_ADVISORY_WORKFLOW_PATHS:
        raise ValueError('jules_advisory.workflow_paths must match Jules advisory workflows')
    secret = require_config_string(section, 'secret', 'jules_advisory')
    if secret != JULES_ADVISORY_SECRET:
        raise ValueError('jules_advisory.secret must be JULES_API_KEY')
    sessions_endpoint_variable = require_config_string(section, 'sessions_endpoint_variable', 'jules_advisory')
    if sessions_endpoint_variable != JULES_ADVISORY_ENDPOINT_VARIABLE:
        raise ValueError('jules_advisory.sessions_endpoint_variable must be JULES_SESSIONS_ENDPOINT')
    timeout_variable = require_config_string(section, 'session_timeout_minutes_variable', 'jules_advisory')
    if timeout_variable != JULES_ADVISORY_TIMEOUT_VARIABLE:
        raise ValueError('jules_advisory.session_timeout_minutes_variable must be JULES_SESSION_TIMEOUT_MINUTES')
    sessions_endpoint = require_config_string(section, 'sessions_endpoint', 'jules_advisory')
    timeout_minutes = require_config_positive_int(section, 'session_timeout_minutes', 'jules_advisory')
    if section.get('require_plan_approval') is not True:
        raise ValueError('jules_advisory.require_plan_approval must be true')
    return {'workflow_paths': sorted(workflow_paths), 'secret': secret, 'sessions_endpoint_variable': sessions_endpoint_variable, 'session_timeout_minutes_variable': timeout_variable, 'repository_variables': {sessions_endpoint_variable: sessions_endpoint, timeout_variable: str(timeout_minutes)}, 'require_plan_approval': True}

def github_actions_runners_config_floor_errors() -> list[str]:
    findings: list[str] = []
    if not DEFAULT_RUNNERS_CONFIG.exists():
        require_nonempty((), RUNNERS_CONFIG_LABEL, findings)
        return findings
    try:
        text = DEFAULT_RUNNERS_CONFIG.read_text(encoding='utf-8')
    except OSError as exc:
        return [f'github-actions runner config invalid: {exc}']
    require_nonempty(text.strip(), RUNNERS_CONFIG_LABEL, findings)
    return findings

def load_required_github_actions_runners_config() -> tuple[dict[str, object] | None, list[str]]:
    floor_errors = github_actions_runners_config_floor_errors()
    if floor_errors:
        return (None, floor_errors)
    try:
        return (load_github_actions_runners_config(), [])
    except (OSError, ValueError, tomllib.TOMLDecodeError) as exc:
        return (None, [f'github-actions runner config invalid: {exc}'])

def load_github_actions_runners_config(path: pathlib.Path | None=None) -> dict[str, object]:
    if path is None:
        path = DEFAULT_RUNNERS_CONFIG
    if not path.exists():
        raise FileNotFoundError(f'managed runner config missing: {path}')
    data = tomllib.loads(path.read_text(encoding='utf-8'))
    runners = data.get('runners')
    workflows = data.get('workflows')
    meter = data.get('meter')
    if not isinstance(runners, dict) or not isinstance(workflows, dict):
        raise ValueError('ci/github-actions-runners.toml must define [runners] and [workflows]')
    if not isinstance(meter, dict):
        raise ValueError('ci/github-actions-runners.toml must define [meter]')
    workflows = {
        key: value for key, value in workflows.items()
        if key not in DORMANT_REVIEW_CONFIG_KEYS
    }
    active_data = dict(data)
    cargo_build_jobs_section = data.get('cargo_build_jobs')
    if isinstance(cargo_build_jobs_section, dict):
        active_data['cargo_build_jobs'] = {
            key: value for key, value in cargo_build_jobs_section.items()
            if key not in DORMANT_REVIEW_CONFIG_KEYS
        }
    jules_advisory = validate_jules_advisory_config(active_data)
    cargo_build_jobs = validate_cargo_build_jobs_config(active_data)
    meter_workflows = meter.get('included_workflows')
    if not isinstance(meter_workflows, list) or not all((isinstance(workflow, str) and workflow for workflow in meter_workflows)):
        raise ValueError('meter.included_workflows must be a non-empty string list')
    meter_workflows = [
        workflow for workflow in meter_workflows
        if workflow not in DORMANT_REVIEW_CONFIG_KEYS
    ]
    meter_api_limits = meter.get('api_limits')
    if not isinstance(meter_api_limits, dict):
        raise ValueError('meter.api_limits must be a table')
    for key in ('workflow_runs_per_page', 'run_jobs_per_page', 'branch_pull_requests_per_page', 'draft_timeline_items'):
        value = meter_api_limits.get(key)
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            raise ValueError(f'meter.api_limits.{key} must be a positive integer')
    tier_to_var: dict[str, str] = {}
    managed_labels: list[str] = []
    for tier, entry in runners.items():
        if not isinstance(entry, dict):
            raise ValueError(f'runners.{tier} must be a table')
        variable = entry.get('variable')
        label = entry.get('label')
        if not isinstance(variable, str) or not variable:
            raise ValueError(f'runners.{tier}.variable must be a non-empty string')
        if not isinstance(label, str) or not label:
            raise ValueError(f'runners.{tier}.label must be a non-empty string')
        tier_to_var[tier] = variable
        if tier != 'github_hosted':
            managed_labels.append(label)
    for workflow_key, job_table in workflows.items():
        if not isinstance(job_table, dict):
            raise ValueError(f'workflows.{workflow_key} must be a table')
        for job, tier in job_table.items():
            if not isinstance(tier, str) or not tier:
                raise ValueError(f'workflows.{workflow_key}.{job} must name a runner tier')
    return {'tier_to_var': tier_to_var, 'managed_labels': sorted(set(managed_labels)), 'meter_included_workflows': sorted(set(meter_workflows)), 'variables': sorted(set(tier_to_var.values()) | set(jules_advisory['repository_variables'])), 'workflows': workflows, 'jules_advisory': jules_advisory, 'cargo_build_jobs': cargo_build_jobs}

def extract_job_runs_on_var(job_lines: list[str]) -> str | None:
    for line in job_lines:
        match = JOB_RUNS_ON_VAR_RE.match(line)
        if match is not None:
            return match.group(1)
    return None

def workflow_trigger_keys(workflow_text: str) -> set[str]:
    lines = [strip_comment(line).rstrip() for line in workflow_text.splitlines()]
    for index, line in enumerate(lines):
        if line == 'on:':
            triggers: set[str] = set()
            for child in lines[index + 1:]:
                if child and (not child.startswith((' ', '\t'))):
                    break
                match = re.match('^  ([^ \\t:#][^:#]*):', child)
                if match:
                    triggers.add(match.group(1).strip().strip('\'"'))
            return triggers
        if line.startswith('on:'):
            inline = line[len('on:'):].strip()
            if inline.startswith('[') and inline.endswith(']'):
                return {item.strip().strip('\'"') for item in inline[1:-1].split(',') if item.strip()}
            if inline:
                return {inline.strip().strip('\'"')}
    return set()

def validate_cargo_build_jobs_config(data: dict[str, object]) -> dict[str, dict[str, int]]:
    section = data.get('cargo_build_jobs')
    if not isinstance(section, dict):
        raise ValueError('ci/github-actions-runners.toml must define [cargo_build_jobs]')
    config: dict[str, dict[str, int]] = {}
    for workflow_key, job_table in section.items():
        if not isinstance(workflow_key, str) or not workflow_key:
            raise ValueError('cargo_build_jobs workflow keys must be non-empty strings')
        if not isinstance(job_table, dict):
            raise ValueError(f'cargo_build_jobs.{workflow_key} must be a table')
        config[workflow_key] = {}
        for job, value in job_table.items():
            if not isinstance(job, str) or not job:
                raise ValueError(f'cargo_build_jobs.{workflow_key} job keys must be non-empty strings')
            if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
                raise ValueError(f'cargo_build_jobs.{workflow_key}.{job} must be a positive integer')
            config[workflow_key][job] = value
    return config

def workflow_schedule_crons(workflow_text: str) -> tuple[list[str], list[str]]:
    crons: list[str] = []
    extras: list[str] = []
    for line in workflow_trigger_block(workflow_text, 'schedule'):
        clean = strip_comment(line).strip()
        if not clean:
            continue
        match = re.fullmatch('-\\s*cron:\\s*(.+)', clean)
        if match is None:
            extras.append(clean)
            continue
        crons.append(yaml_scalar(match.group(1)))
    return (crons, extras)
INVALID_STORAGE_TRIPWIRE_KEY = '<invalid-storage-tripwire-key>'

def storage_tripwire_key_at_indent(line: str, indent: int) -> str | None:
    clean = strip_comment(line).rstrip()
    if not clean:
        return None
    actual_indent = len(clean) - len(clean.lstrip(' '))
    if actual_indent != indent:
        return None
    match = re.fullmatch(f'\\s{{{indent}}}({YAML_KEY_PATTERN})\\s*:\\s*.*', clean)
    if match is None:
        return INVALID_STORAGE_TRIPWIRE_KEY
    return unquote_yaml_scalar(match.group(1))

def storage_tripwire_key_at_any_indent(line: str) -> str | None:
    clean = strip_comment(line).rstrip()
    if not clean:
        return None
    match = re.fullmatch(f'\\s*({YAML_KEY_PATTERN})\\s*:\\s*.*', clean)
    if match is None:
        return None
    return unquote_yaml_scalar(match.group(1))

def workflow_top_level_keys(workflow_text: str) -> list[str]:
    keys: list[str] = []
    for line in workflow_text.splitlines():
        key = storage_tripwire_key_at_indent(line, 0)
        if key is not None:
            keys.append(key)
    return keys

def storage_tripwire_job_top_level_keys(job_lines: list[str]) -> list[str]:
    keys: list[str] = []
    for line in job_lines:
        key = storage_tripwire_key_at_indent(line, 4)
        if key is not None:
            keys.append(key)
    return keys

def storage_tripwire_expected_checkout_action(required_fragments: tuple[str, ...]) -> str | None:
    actions = [fragment.removeprefix('uses: ').strip() for fragment in required_fragments if fragment.startswith('uses: ')]
    return actions[0] if len(actions) == 1 else None

def storage_tripwire_expected_persist_credentials(required_fragments: tuple[str, ...]) -> str | None:
    values = [fragment.split(':', 1)[1].strip() for fragment in required_fragments if fragment.startswith('persist-credentials:')]
    return values[0] if len(values) == 1 else None

def storage_tripwire_expected_env(required_fragments: tuple[str, ...]) -> dict[str, str]:
    env: dict[str, str] = {}
    for fragment in required_fragments:
        match = re.fullmatch('([A-Z][A-Z0-9_]*):\\s*(.+)', fragment)
        if match is not None:
            env[match.group(1)] = match.group(2)
    return env

def verify_storage_tripwire_workflow(workflows: dict[str, str], policy_text: str) -> list[str]:
    try:
        policy = ci_storage_tripwire.load_policy_text(policy_text, source='storage tripwire policy')
    except ci_storage_tripwire.TripwireError as exc:
        return [f'storage tripwire policy invalid: {exc}']
    workflow_contract = policy.workflow
    workflow_name = workflow_contract.workflow_path
    workflow_text = workflows.get(workflow_name)
    if workflow_text is None:
        return [f'{workflow_name} must exist']
    errors: list[str] = []
    workflow_keys = workflow_top_level_keys(workflow_text)
    allowed_workflow_keys = set(workflow_contract.top_level_keys)
    if set(workflow_keys) != allowed_workflow_keys or len(workflow_keys) != len(set(workflow_keys)):
        errors.append(f'{workflow_name} top-level keys must match the storage tripwire workflow contract')
    if workflow_trigger_keys(workflow_text) != set(workflow_contract.triggers):
        errors.append(f'{workflow_name} triggers must match storage_tripwire.workflow.triggers')
    schedule_crons, schedule_extras = workflow_schedule_crons(workflow_text)
    if schedule_crons != [workflow_contract.schedule_cron] or schedule_extras:
        errors.append(f'{workflow_name} schedule cron must match storage_tripwire.workflow.schedule_cron')
    actual_permissions = scalar_mapping(top_level_block(workflow_text, 'permissions'))
    if actual_permissions != dict(workflow_contract.permissions):
        errors.append(f'{workflow_name} permissions must match storage_tripwire.workflow.permissions')
    expected_concurrency = {'group': workflow_contract.concurrency_group, 'cancel-in-progress': str(workflow_contract.cancel_in_progress).lower()}
    actual_concurrency = scalar_mapping(top_level_block(workflow_text, 'concurrency'))
    if actual_concurrency != expected_concurrency:
        errors.append(f'{workflow_name} concurrency must match storage_tripwire.workflow concurrency settings')
    for forbidden in workflow_contract.forbidden_fragments:
        if forbidden in workflow_text:
            errors.append(f'{workflow_name} must not contain forbidden workflow fragment from storage_tripwire.workflow.forbidden_fragments')
    jobs = parse_jobs(workflow_text)
    if set(jobs) != {workflow_contract.job_id}:
        errors.append(f'{workflow_name} must define only the configured storage tripwire job')
    job = jobs.get(workflow_contract.job_id)
    if job is None:
        errors.append(f'{workflow_name} must define configured storage tripwire job')
        return errors
    job_text = '\n'.join(job)
    job_keys = storage_tripwire_job_top_level_keys(job)
    allowed_job_keys = set(workflow_contract.job_keys)
    if set(job_keys) != allowed_job_keys or len(job_keys) != len(set(job_keys)):
        errors.append(f'{workflow_name} storage tripwire job keys must match the workflow contract')
    if job_if_value(job) != workflow_contract.job_if:
        errors.append(f'{workflow_name} storage tripwire job if must match storage_tripwire.workflow.job_if')
    actual_var = extract_job_runs_on_var(job)
    if actual_var != workflow_contract.runner_var:
        errors.append(f'{workflow_name} storage tripwire runs-on must match storage_tripwire.workflow.runner_var')
    if any((storage_tripwire_key_at_indent(line, 4) == 'permissions' for line in job)):
        errors.append(f'{workflow_name} storage tripwire job must not define job-level permissions')
    if any((storage_tripwire_key_at_any_indent(line) == 'continue-on-error' for line in job)):
        errors.append(f'{workflow_name} storage tripwire job must not use continue-on-error')
    steps = step_blocks(job)
    if len(steps) != 2:
        errors.append(f'{workflow_name} storage tripwire job must contain exactly checkout and run steps')
    else:
        checkout_action = storage_tripwire_expected_checkout_action(workflow_contract.required_fragments)
        persist_credentials = storage_tripwire_expected_persist_credentials(workflow_contract.required_fragments)
        expected_env = storage_tripwire_expected_env(workflow_contract.required_fragments)
        checkout_items = block_top_level_items(steps[0])
        if checkout_action is None or persist_credentials is None or checkout_items is None or (set(checkout_items) != {'uses', 'with'}) or (checkout_items.get('uses') != checkout_action) or (block_nested_mapping_items(steps[0], 'with') != {'persist-credentials': persist_credentials}):
            errors.append(f'{workflow_name} checkout step must match storage_tripwire.workflow.required_fragments')
        run_items = block_top_level_items(steps[1])
        if not expected_env or run_items is None or set(run_items) != {'name', 'env', 'run'} or (not run_items.get('name')) or (block_nested_mapping_items(steps[1], 'env') != expected_env) or (step_run_command(steps[1]) != workflow_contract.run_command):
            errors.append(f'{workflow_name} run step must match storage_tripwire.workflow contract')
    for required in workflow_contract.required_fragments:
        if required not in job_text:
            errors.append(f'{workflow_name} job must contain storage_tripwire.workflow.required_fragments')
    return errors

def verify_github_actions_runner_contract(workflows: dict[str, str]) -> list[str]:
    config, config_errors = load_required_github_actions_runners_config()
    if config_errors:
        return config_errors
    assert config is not None
    tier_to_var = config['tier_to_var']
    meter_included_workflows = set(config['meter_included_workflows'])
    workflow_tables = config['workflows']
    cargo_build_jobs = config['cargo_build_jobs']
    errors: list[str] = []
    known_workflow_keys = set(WORKFLOW_RUNNER_CONFIG_KEYS.values()) - DORMANT_REVIEW_CONFIG_KEYS
    for workflow_key in sorted(workflow_tables):
        if workflow_key not in known_workflow_keys:
            errors.append(f'workflows.{workflow_key} in ci/github-actions-runners.toml has no workflow contract')
    managed_workflows = {workflow_key for workflow_key, job_table in workflow_tables.items() if isinstance(job_table, dict) and any((isinstance(tier, str) and tier != 'github_hosted' for tier in job_table.values()))}
    if meter_included_workflows != managed_workflows:
        errors.append(f'meter.included_workflows must match workflows with managed runner tiers: expected {sorted(managed_workflows)!r}, got {sorted(meter_included_workflows)!r}')
    for workflow_key, job_table in sorted(workflow_tables.items()):
        if not isinstance(job_table, dict):
            continue
    if isinstance(cargo_build_jobs, dict):
        for workflow_key, job_table in sorted(cargo_build_jobs.items()):
            configured_workflow = workflow_tables.get(workflow_key)
            if not isinstance(configured_workflow, dict):
                errors.append(f'cargo_build_jobs.{workflow_key} in ci/github-actions-runners.toml has no workflow contract')
                continue
            if not isinstance(job_table, dict):
                continue
            for job in sorted(job_table):
                if job not in configured_workflow:
                    errors.append(f'cargo_build_jobs.{workflow_key}.{job} must reference a configured workflow job')
    for workflow_name, workflow_text in sorted(workflows.items()):
        jobs = parse_jobs(workflow_text)
        if not jobs:
            continue
        workflow_key = WORKFLOW_RUNNER_CONFIG_KEYS.get(workflow_name)
        if workflow_key is None:
            errors.append(f'{workflow_name} must be mapped in ci/github-actions-runners.toml')
            continue
        job_table = workflow_tables.get(workflow_key)
        if not isinstance(job_table, dict):
            errors.append(f'workflows.{workflow_key} missing in ci/github-actions-runners.toml')
            continue
        cargo_job_table = cargo_build_jobs.get(workflow_key) if isinstance(cargo_build_jobs, dict) else None
        if not isinstance(cargo_job_table, dict):
            cargo_job_table = {}
        workflow_env_text = uncommented_text(top_level_block(workflow_text, 'env'))
        if INLINE_CARGO_BUILD_JOBS_RE.search(workflow_env_text):
            errors.append(f'{workflow_name} workflow-level CARGO_BUILD_JOBS must come from ci/github-actions-runners.toml via setup-environment')
        configured_jobs = set(job_table)
        actual_jobs = {job for job, lines in jobs.items() if not any((re.match('^    uses:\\s*', strip_comment(line)) for line in lines))}
        for job in sorted(configured_jobs - actual_jobs):
            errors.append(f'{workflow_name} configured runner job {job} missing from workflow')
        for job in sorted(actual_jobs - configured_jobs):
            errors.append(f'{workflow_name} job {job} missing from ci/github-actions-runners.toml')
        for job in sorted(configured_jobs & actual_jobs):
            tier = job_table[job]
            expected_var = tier_to_var.get(tier)
            if expected_var is None:
                errors.append(f'unknown runner tier {tier!r} for {workflow_name} {job}')
                continue
            actual_var = extract_job_runs_on_var(jobs[job])
            if actual_var is None:
                errors.append(f'{workflow_name} {job} runs-on must reference vars.{expected_var} (no hardcoded runner labels)')
                continue
            if actual_var != expected_var:
                errors.append(f'{workflow_name} {job} runs-on must use vars.{expected_var}, got vars.{actual_var}')
            job_text = uncommented_text(jobs[job])
            if INLINE_CARGO_BUILD_JOBS_RE.search(job_text):
                errors.append(f'{workflow_name} {job} CARGO_BUILD_JOBS must come from ci/github-actions-runners.toml via setup-environment')
            if job in cargo_job_table:
                expected_key = f'{workflow_key}.{job}'
                if not job_has_setup_input(jobs[job], 'build-jobs-key', expected_key):
                    errors.append(f'{workflow_name} {job} must resolve CARGO_BUILD_JOBS from cargo_build_jobs.{expected_key}')
                else:
                    for setup_error in cargo_build_jobs_setup_order_errors(jobs[job], expected_key):
                        errors.append(f'{workflow_name} {job} {setup_error}')
            elif 'build-jobs-key:' in job_text:
                errors.append(f'{workflow_name} {job} has build-jobs-key but is missing from cargo_build_jobs.{workflow_key} in ci/github-actions-runners.toml')
    return errors

def actionlint_config_variables(actionlint_text: str) -> set[str]:
    variables: set[str] = set()
    in_section = False
    for line in actionlint_text.splitlines():
        clean = strip_comment(line).strip()
        if clean == 'config-variables:':
            in_section = True
            continue
        if in_section:
            if clean and (not clean.startswith('- ')):
                break
            if clean.startswith('- '):
                variables.add(clean[2:].strip())
    return variables

def workflow_repository_variables(workflows: dict[str, str]) -> set[str]:
    variables: set[str] = set()
    for workflow_text in workflows.values():
        for match in re.finditer('vars\\.([A-Z0-9_]+)', workflow_text):
            variables.add(match.group(1))
    return variables

def verify_actionlint_runner_contract(workflows: dict[str, str], actionlint_path: pathlib.Path=DEFAULT_ACTIONLINT_CONFIG) -> list[str]:
    config, config_errors = load_required_github_actions_runners_config()
    if config_errors:
        return config_errors
    assert config is not None
    if not actionlint_path.exists():
        return [f'actionlint config missing: {actionlint_path}']
    text = actionlint_path.read_text(encoding='utf-8')
    allowed_variables = actionlint_config_variables(text)
    errors: list[str] = []
    for label in config['managed_labels']:
        if f'- {label}' not in text:
            errors.append(f'.github/actionlint.yaml must list managed runner label {label!r}')
    for variable in config['variables']:
        if variable not in allowed_variables:
            errors.append(f'.github/actionlint.yaml must allow config variable {variable!r}')
    for variable in sorted(workflow_repository_variables(workflows)):
        if variable not in allowed_variables:
            errors.append(f'.github/actionlint.yaml must allow repository variable {variable!r} referenced by workflow vars.* expressions')
    expected_variables = set(config['variables']) | workflow_repository_variables(workflows)
    for variable in sorted(allowed_variables - expected_variables):
        errors.append(f'.github/actionlint.yaml allows stale config variable {variable!r} not referenced by workflows or ci/github-actions-runners.toml')
    return errors

def repo_workflow_paths() -> tuple[str, ...]:
    if not DEFAULT_WORKFLOW_DIR.exists():
        return ()
    paths: set[pathlib.Path] = set()
    for pattern in DEFAULT_WORKFLOW_GLOBS:
        paths.update(DEFAULT_WORKFLOW_DIR.glob(pattern))
    return tuple(
        path.relative_to(REPO_ROOT).as_posix()
        for path in sorted(paths)
        if path.relative_to(REPO_ROOT).as_posix() not in DORMANT_REVIEW_WORKFLOW_PATHS
    )

def repo_workflow_texts() -> dict[str, str]:
    return {
        path: (REPO_ROOT / path).read_text()
        for path in repo_workflow_paths()
    }

def verify_fixed_final_review_topology(workflows: dict[str, str]) -> list[str]:
    errors: list[str] = []
    final_path = '.github/workflows/final-review.yml'
    worker_paths = ('.github/workflows/claude-code-review.yml', '.github/workflows/ai-review-kimi-cli.yml', '.github/workflows/ai-review-glm.yml')
    forbidden_paths = ('.github/workflows/ci.yml', '.github/workflows/backtester-ci.yml', '.github/workflows/actionlint.yml', '.github/workflows/coverage-enforcer.yml', '.github/workflows/merge-readiness-finalizer.yml')
    fixed_jobs = ('capture-head', 'evidence', 'claude-review', 'kimi-review', 'glm-review')
    for path in forbidden_paths:
        if path in workflows:
            errors.append(f'superseded verification workflow remains: {path}')
    final_text = workflows.get(final_path)
    if final_text is None:
        errors.append(f'fixed final-review workflow missing: {final_path}')
        return errors
    on_block = final_text.split('concurrency:', 1)[0]
    if 'workflow_dispatch:' not in on_block:
        errors.append('final-review.yml must be workflow_dispatch-only')
    for trigger in ('pull_request:', 'pull_request_review:', 'workflow_call:'):
        if trigger in on_block:
            errors.append(f'final-review.yml exposes alternate trigger {trigger}')
    for job in fixed_jobs:
        if re.search(f'^  {re.escape(job)}:\\s*$', final_text, flags=re.MULTILINE) is None:
            errors.append(f'final-review.yml missing fixed job {job}')
    for forbidden in ('paths-ignore', 'changed-path', 'full_ci_required', 'cache-hit =='):
        if forbidden in final_text:
            errors.append(f'final-review.yml contains conditional verification selector {forbidden}')
    if re.search(r'^\s*if:\s*', final_text, flags=re.MULTILINE):
        errors.append('final-review.yml must not contain conditional job or step paths')
    if 'scripts/final_review_runner.py' not in final_text:
        errors.append('final-review.yml must execute the fixed evidence runner')
    for path in worker_paths:
        text = workflows.get(path)
        if text is None:
            errors.append(f'review worker missing: {path}')
            continue
        if f'uses: ./{path}' not in final_text:
            errors.append(f'final-review.yml does not invoke {path}')
        worker_on = text.split('concurrency:', 1)[0]
        if 'workflow_call:' not in worker_on:
            errors.append(f'{path} must expose workflow_call')
        for trigger in ('pull_request:', 'pull_request_review:', 'workflow_dispatch:'):
            if trigger in worker_on:
                errors.append(f'{path} exposes alternate trigger {trigger}')
    return errors

def main() -> int:
    workflow_texts = repo_workflow_texts()
    repo_automation_texts = {path.relative_to(REPO_ROOT).as_posix(): path.read_text() for path in DEFAULT_REPO_AUTOMATION_FILES if path.exists()}
    for directory, pattern in DEFAULT_REPO_AUTOMATION_GLOBS:
        if not directory.exists():
            continue
        for path in sorted(directory.glob(pattern)):
            repo_automation_texts[path.relative_to(REPO_ROOT).as_posix()] = path.read_text()
    errors = verify_github_actions_runner_contract(workflow_texts)
    errors.extend(verify_actionlint_runner_contract(workflow_texts))
    errors.extend(verify_repo_automation_texts(repo_automation_texts))
    errors.extend(verify_rust_verification_policies())
    errors.extend(verify_test_harness_manifest())
    if DEFAULT_NO_MISTAKES_CONFIG.exists():
        errors.extend(verify_no_mistakes_config(DEFAULT_NO_MISTAKES_CONFIG.read_text()))
    if DEFAULT_MERGIFY_CONFIG.exists():
        errors.extend(verify_mergify_config(DEFAULT_MERGIFY_CONFIG.read_text()))
    else:
        errors.append('.mergify.yml is required for Mergify queue governance')
    errors = list(dict.fromkeys(errors))
    if errors:
        for error in errors:
            print(f'ERROR: {error}', file=sys.stderr)
        return 1
    print('OK: CI workflow hygiene verifier passed.')
    return 0

def cli(argv: list[str]) -> int:
    if argv:
        print(f'ERROR: unknown verify_ci_workflow_hygiene mode: {argv[0]}', file=sys.stderr)
        return 2
    return main()
if __name__ == '__main__':
    import lane_governor
    lane_governor.acquire()
    sys.exit(cli(sys.argv[1:]))
