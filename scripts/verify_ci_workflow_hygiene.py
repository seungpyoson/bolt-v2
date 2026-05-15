#!/usr/bin/env python3
"""Verify CI workflow hygiene invariants for the current workflow topology."""

from __future__ import annotations

import pathlib
import re
import sys


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"
DEFAULT_WORKFLOWS = (
    DEFAULT_WORKFLOW,
    REPO_ROOT / ".github" / "workflows" / "advisory.yml",
)
DEFAULT_SETUP_ACTION = REPO_ROOT / ".github" / "actions" / "setup-environment" / "action.yml"

REQUIRED_JOBS = (
    "detector",
    "fmt-check",
    "deny",
    "clippy",
    "check-aarch64",
    "source-fence",
    "test",
    "build",
    "same-sha-main-evidence",
    "gate",
    "deploy",
)
GATE_REQUIRED = ("detector", "fmt-check", "deny", "clippy", "check-aarch64", "source-fence", "test", "build")
DEPLOY_REQUIRED_NEEDS = (
    "gate",
    "same-sha-main-evidence",
    "build",
    "detector",
    "fmt-check",
    "deny",
    "clippy",
    "check-aarch64",
    "source-fence",
    "test",
)
TAG_SKIPPED_JOBS = ("fmt-check", "deny", "clippy", "check-aarch64", "source-fence", "test", "build")
TARGET_DIR_JOBS = ("clippy", "check-aarch64", "source-fence", "test", "build")
CACHE_KEY_JOBS = ("deny", "clippy", "check-aarch64", "source-fence", "test", "build")
TAG_SKIP_IF_RE = re.compile(r"^    if:\s*(?:\$\{\{\s*)?!startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)\s*(?:\}\})?\s*$")
SAME_SHA_IF_RE = re.compile(r"^    if:\s*(?:\$\{\{\s*)?startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)\s*(?:\}\})?\s*$")
BUILD_IF_RE = re.compile(
    r"^    if:\s*\$\{\{\s*!startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)\s*&&\s*"
    r"needs\.detector\.outputs\.build_required\s*==\s*['\"]true['\"]\s*\}\}\s*$"
)
GATE_IF_RE = re.compile(r"^    if:\s*(?:\$\{\{\s*)?always\(\)\s*(?:\}\})?\s*$")
DEPLOY_IF_RE = re.compile(
    r"^    if:\s*\$\{\{\s*always\(\)\s*&&\s*startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)\s*&&\s*"
    r"needs\.gate\.result\s*==\s*['\"]success['\"]\s*&&\s*"
    r"needs\.same-sha-main-evidence\.result\s*==\s*['\"]success['\"]\s*\}\}\s*$"
)
EXIT_RE = re.compile(r"^\s*exit(?:\s+([0-9]+))?\s*$", re.MULTILINE)
IF_OR_ELIF_RE = re.compile(r"^\s*(if|elif)\s+\[\[\s*(?P<condition>.*?)\s*\]\];\s*then\s*$")
ELSE_RE = re.compile(r"^\s*else\s*$")
FI_RE = re.compile(r"^\s*fi\s*$")
TARGET_DIR_OPT_IN_RE = re.compile(r"^\s+include-managed-target-dir:\s*(['\"])true\1\s*$")
SETUP_TARGET_DIR_EXPORT_RE = re.compile(r"^\s+value:\s*\$\{\{\s*steps\.target_dir\.outputs\.managed_target_dir\s*\}\}\s*$")
SETUP_TARGET_DIR_RELATIVE_EXPORT_RE = re.compile(
    r"^\s+value:\s*\$\{\{\s*steps\.target_dir\.outputs\.managed_target_dir_relative\s*\}\}\s*$"
)
SETUP_TARGET_DIR_IF_RE = re.compile(
    r"^\s+if:\s*\$\{\{\s*inputs\.include-managed-target-dir\s*==\s*['\"]true['\"]\s*\}\}\s*$"
)
SETUP_ACTION_REQUIRED_LITERALS = (
    "inputs.just-version",
    "inputs.include-deny-version",
    "inputs.include-nextest-version",
    "inputs.include-build-values",
    "inputs.lint-workflow-contract",
    "inputs.include-managed-target-dir",
    "CLAUDE_CONFIG_READ_TOKEN:",
    "inputs.claude-config-read-token",
    "just ci-lint-workflow",
    "managed_target_dir_relative",
    "os.path.relpath",
    "awk -F'\\\"' '/^channel = / {print $2}' rust-toolchain.toml",
    "just --evaluate deny_version",
    "just --evaluate nextest_version",
    "just --evaluate target",
    "just --evaluate zig_version",
    "just --evaluate zigbuild_version",
    "just --evaluate rust_verification_owner",
    "just --evaluate rust_verification_source_repo",
    "just --evaluate rust_verification_source_sha",
    "just --evaluate rust_verification_ci_install_script",
    'target-dir --repo "$GITHUB_WORKSPACE"',
)
SETUP_ACTION_OUTPUT_MAPPINGS = {
    "rust_toolchain": "steps.shared.outputs.rust_toolchain",
    "deny_version": "steps.shared.outputs.deny_version",
    "nextest_version": "steps.shared.outputs.nextest_version",
    "target": "steps.shared.outputs.target",
    "zig_version": "steps.shared.outputs.zig_version",
    "zigbuild_version": "steps.shared.outputs.zigbuild_version",
    "rust_verification_owner": "steps.shared.outputs.rust_verification_owner",
    "rust_verification_source_repo": "steps.shared.outputs.rust_verification_source_repo",
    "rust_verification_source_sha": "steps.shared.outputs.rust_verification_source_sha",
    "rust_verification_ci_install_script": "steps.shared.outputs.rust_verification_ci_install_script",
    "managed_target_dir": "steps.target_dir.outputs.managed_target_dir",
    "managed_target_dir_relative": "steps.target_dir.outputs.managed_target_dir_relative",
}
SETUP_ACTION_ORDERED_STEPS = (
    "Lint workflow contract",
    "Read shared values",
    "Install managed Rust owner",
    "Resolve managed target dir",
    "Setup Rust toolchain",
)
TEST_FAIL_FAST_FALSE_RE = re.compile(r"^\s+fail-fast:\s*false\s*$")
TEST_MATRIX_SHARD_RE = re.compile(r"^\s+shard:\s*\[\s*1\s*,\s*2\s*,\s*3\s*,\s*4\s*\]\s*$")
TEST_PARTITION_COMMAND = "just test -- --partition count:${{ matrix.shard }}/4"
TEST_REPRODUCTION_COMMAND = TEST_PARTITION_COMMAND
TEST_REPRODUCTION_ECHO = f'echo "reproduce locally: {TEST_REPRODUCTION_COMMAND}"'
TEST_SHARD_CACHE_RE = re.compile(r"^\s+key:\s*.*matrix\.shard.*of-4\s*$")
CACHE_KEY_RE = re.compile(r"^\s+key:\s*\S+.*$")
JUST_LANE_RE = re.compile(
    r"(^|[^A-Za-z0-9_./-])just\s+"
    r"(fmt-check|deny|deny-advisories|clippy|test|build|check-aarch64|source-fence)"
    r"([^A-Za-z0-9_]|$)"
)
REPO_LOCAL_ARTIFACT_RE = re.compile(r"(^|[^A-Za-z0-9_./-])target/(?:.*/)?release/bolt-v2(?:\.sha256)?([^A-Za-z0-9_./-]|$)")
BINARY_PATH_COMMAND = 'python3 "${{ steps.setup.outputs.rust_verification_owner }}" binary-path --repo "$GITHUB_WORKSPACE" --bin bolt-v2'
TEST_WORKSPACES_RE = re.compile(
    r"^\s+workspaces:\s*\.\s*->\s*\$\{\{\s*steps\.setup\.outputs\.managed_target_dir_relative\s*\}\}\s*$"
)
TEST_CACHE_TARGETS_TRUE_RE = re.compile(r"^\s+cache-targets:\s*(['\"]?)true\1\s*$")
TEST_CACHE_WORKSPACE_CRATES_RE = re.compile(r"^\s+cache-workspace-crates:\s*(['\"]?)true\1\s*$")
TEST_RUST_ENV_HASH_RE = re.compile(r"^\s+add-rust-environment-hash-key:\s*(['\"]?)true\1\s*$")


def strip_comment(line: str) -> str:
    quote: str | None = None
    escaped = False
    for index, char in enumerate(line):
        if quote is not None:
            if escaped:
                escaped = False
            elif char == "\\" and quote == '"':
                escaped = True
            elif char == quote:
                quote = None
            continue
        if char in {"'", '"'}:
            quote = char
            continue
        if char == "#":
            return line[:index].rstrip()
    return line.rstrip()


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
        if clean == "jobs:":
            in_jobs = True
            current = None
            continue
        if not in_jobs:
            continue
        if clean and not clean.startswith((" ", "\t")):
            break
        match = re.match(r"^  ([^ \t:#][^:#]*):\s*$", clean)
        if match:
            current = match.group(1).strip().strip("'\"")
            jobs[current] = []
            continue
        if current is not None:
            jobs[current].append(clean)

    return jobs


def job_header_indent_errors(workflow_text: str) -> list[str]:
    errors: list[str] = []
    required_job_re = re.compile(rf"^(?P<indent>\s+)({'|'.join(re.escape(job) for job in REQUIRED_JOBS)}):\s*$")
    in_jobs = False

    for line in workflow_text.splitlines():
        clean = strip_comment(line)
        if clean == "jobs:":
            in_jobs = True
            continue
        if not in_jobs:
            continue
        if clean and not clean.startswith((" ", "\t")):
            break
        match = required_job_re.match(clean)
        if match and match.group("indent") != "  ":
            job = clean.strip()[:-1]
            errors.append(f"job {job} must use two-space top-level indentation")

    return errors


def parse_inline_needs(value: str) -> set[str]:
    value = value.strip()
    if not value:
        return set()
    if value.startswith("[") and value.endswith("]"):
        return {part.strip().strip("'\"") for part in value[1:-1].split(",") if part.strip()}
    return {value.strip().strip("'\"")}


def extract_needs(job_lines: list[str]) -> set[str]:
    needs: set[str] = set()
    index = 0
    while index < len(job_lines):
        clean = strip_comment(job_lines[index])
        match = re.match(r"^    needs:\s*(.*)$", clean)
        if not match:
            index += 1
            continue
        rest = match.group(1).strip()
        if rest:
            needs.update(parse_inline_needs(rest))
            index += 1
            continue
        index += 1
        while index < len(job_lines):
            nested = strip_comment(job_lines[index])
            if re.match(r"^    [A-Za-z0-9_.-]+:", nested):
                break
            item = re.match(r"^\s*-\s*([A-Za-z0-9_.-]+)\s*$", nested)
            if item:
                needs.add(item.group(1))
            index += 1
    return needs


def step_blocks(job_lines: list[str]) -> list[list[str]]:
    blocks: list[list[str]] = []
    current: list[str] | None = None
    for line in job_lines:
        if re.match(r"^      - ", line):
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
    return [block for block in step_blocks(job_lines) if any("./.github/actions/setup-environment" in line for line in block)]


def block_runs_command(block: list[str], command: str) -> bool:
    for index, line in enumerate(block):
        clean = strip_comment(line)
        inline = re.match(r"^\s*(?:-\s*)?run:\s*(.*?)\s*$", clean)
        if inline is None:
            continue
        value = inline.group(1).strip().strip("'\"")
        if value == command:
            return True
        if value not in {"|", ">"}:
            continue
        for nested in block[index + 1 :]:
            nested_clean = strip_comment(nested).strip()
            if nested_clean == command:
                return True
        return False
    return False


def job_runs_command(job_lines: list[str], command: str) -> bool:
    return any(block_runs_command(block, command) for block in step_blocks(job_lines))


def block_has_target_dir_opt_in(block: list[str]) -> bool:
    return any(TARGET_DIR_OPT_IN_RE.match(strip_comment(line)) for line in block)


def block_has_input(block: list[str], name: str, value: str | None = None) -> bool:
    if value is None:
        pattern = re.compile(rf"^\s+{re.escape(name)}:\s*\S+.*$")
    else:
        pattern = re.compile(rf"^\s+{re.escape(name)}:\s*{re.escape(value)}\s*$")
    return any(pattern.match(strip_comment(line)) for line in block)


def job_has_setup_input(job_lines: list[str], name: str, value: str | None = None) -> bool:
    return any(block_has_input(block, name, value) for block in setup_action_blocks(job_lines))


def job_uses_managed_target_dir(job_lines: list[str]) -> bool:
    return any("steps.setup.outputs.managed_target_dir" in strip_comment(line) for line in job_lines)


def job_opts_into_managed_target_dir(job_lines: list[str]) -> bool:
    return any(block_has_target_dir_opt_in(block) for block in setup_action_blocks(job_lines))


def uncommented_text(lines: list[str]) -> str:
    return "\n".join(strip_comment(line) for line in lines)


def has_line_matching(lines: list[str], pattern: re.Pattern[str]) -> bool:
    return any(pattern.match(strip_comment(line)) for line in lines)


def has_run_command(lines: list[str], command: str) -> bool:
    expected = {f"run: {command}", f"- run: {command}"}
    return any(strip_comment(line).strip() in expected for line in lines)


def job_has_explicit_cache_key(job_lines: list[str]) -> bool:
    return any(CACHE_KEY_RE.match(strip_comment(line)) for line in job_lines)


def job_just_lanes(job_lines: list[str]) -> set[str]:
    return {match.group(2) for match in JUST_LANE_RE.finditer(uncommented_text(job_lines))}


def rust_cache_blocks(job_lines: list[str]) -> list[list[str]]:
    return [block for block in step_blocks(job_lines) if any("Swatinem/rust-cache" in line for line in block)]


def test_cache_block(job_lines: list[str]) -> list[str]:
    blocks = rust_cache_blocks(job_lines)
    if not blocks:
        return []
    return blocks[0]


def block_has_line_matching(block: list[str], pattern: re.Pattern[str]) -> bool:
    return any(pattern.match(strip_comment(line)) for line in block)


def block_has_managed_target_cache_directories(block: list[str]) -> bool:
    return any(
        "cache-directories:" in strip_comment(line) and "steps.setup.outputs.managed_target_dir" in strip_comment(line)
        for line in block
    )


def block_cache_key_contains_github_sha(block: list[str]) -> bool:
    return any("key:" in strip_comment(line) and "github.sha" in strip_comment(line) for line in block)


def test_has_shard_reproduction_command(job_lines: list[str]) -> bool:
    for block in step_blocks(job_lines):
        for index, line in enumerate(block):
            clean = strip_comment(line).strip()
            if clean == "run: |":
                for nested in block[index + 1 :]:
                    if strip_comment(nested).strip() == TEST_REPRODUCTION_ECHO:
                        return True
                break
    return False


def test_has_inline_shard_reproduction_command(job_lines: list[str]) -> bool:
    for block in step_blocks(job_lines):
        for line in block:
            clean = strip_comment(line).strip()
            if clean.startswith(("run:", "- run:")) and "reproduce" in clean.lower() and TEST_REPRODUCTION_COMMAND in clean:
                return True
    return False


def job_skips_tag_reuse(job_lines: list[str]) -> bool:
    return has_line_matching(job_lines, TAG_SKIP_IF_RE)


def same_sha_job_has_outputs(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    required = (
        "source_run_id: ${{ steps.evidence.outputs.source_run_id }}",
        "check_suite_id: ${{ steps.evidence.outputs.check_suite_id }}",
        "artifact_id: ${{ steps.evidence.outputs.artifact_id }}",
        "source_sha: ${{ steps.evidence.outputs.source_sha }}",
    )
    return all(item in text for item in required)


def same_sha_job_runs_resolver(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    return "id: evidence" in text and "python3 scripts/find_same_sha_main_evidence.py" in text


def clippy_installs_aarch64_toolchain(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    return "gcc-aarch64-linux-gnu" in text or "libc6-dev-arm64-cross" in text


def check_aarch64_installs_cross_compiler_packages(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    return "gcc-aarch64-linux-gnu" in text and "libc6-dev-arm64-cross" in text


def gate_checks_lane_success(gate_text: str, job: str) -> bool:
    condition = f'"${{{{ needs.{job}.result }}}}" != "success"'
    return branch_exits(gate_text, "if", condition)


def gate_checks_build_result(gate_text: str) -> bool:
    # These literals intentionally lock the current gate shell contract.
    # Any gate refactor must update this verifier and its self-tests together.
    required_condition = '"$build_required" == "true"'
    true_result_condition = '"$build_result" != "success"'
    optional_result_condition = '"$build_result" != "success" && "$build_result" != "skipped"'
    chain = if_chain_bodies(gate_text, required_condition)
    if chain is None:
        return False
    return (
        'build_required="${{ needs.detector.outputs.build_required }}"' in gate_text
        and 'build_result="${{ needs.build.result }}"' in gate_text
        and branch_exits(chain.get(("if", required_condition), ""), "if", true_result_condition)
        and body_exits(chain.get(("elif", optional_result_condition), ""))
    )


def if_chain_bodies(gate_text: str, condition: str) -> dict[tuple[str, str], str] | None:
    lines = gate_text.splitlines()
    for start, line in enumerate(lines):
        match = IF_OR_ELIF_RE.match(line)
        if match and match.group(1) == "if" and match.group("condition") == condition:
            return collect_if_chain_bodies(lines, start, condition)
    return None


def collect_if_chain_bodies(lines: list[str], start: int, condition: str) -> dict[tuple[str, str], str] | None:
    bodies: dict[tuple[str, str], list[str]] = {("if", condition): []}
    current = ("if", condition)
    depth = 0
    for line in lines[start + 1 :]:
        branch_match = IF_OR_ELIF_RE.match(line)
        if branch_match:
            keyword = branch_match.group(1)
            branch_condition = branch_match.group("condition")
            if depth == 0 and keyword == "elif":
                current = ("elif", branch_condition)
                bodies[current] = []
                continue
            bodies[current].append(line)
            if keyword == "if":
                depth += 1
            continue
        if ELSE_RE.match(line):
            if depth == 0:
                current = ("else", "")
                bodies[current] = []
            else:
                bodies[current].append(line)
            continue
        if FI_RE.match(line):
            if depth == 0:
                return {key: "\n".join(value) for key, value in bodies.items()}
            bodies[current].append(line)
            depth -= 1
            continue
        bodies[current].append(line)
    return None


def gate_checks_same_sha_reuse(gate_text: str) -> list[str]:
    errors: list[str] = []
    if 'tag_ref="${{ startsWith(github.ref, \'refs/tags/v\') }}"' not in gate_text and (
        'tag_ref="${{ startsWith(github.ref, "refs/tags/v") }}"' not in gate_text
    ):
        errors.append("gate must compute tag_ref")
    if not branch_exits(gate_text, "if", '"${{ needs.same-sha-main-evidence.result }}" != "success"'):
        errors.append("gate must check same-sha-main-evidence success")
    if not branch_exits(gate_text, "if", '"${{ needs.same-sha-main-evidence.result }}" != "skipped"'):
        errors.append("gate must check same-sha-main-evidence skip on non-tag")
    for job in TAG_SKIPPED_JOBS:
        if not branch_exits(gate_text, "if", f'"${{{{ needs.{job}.result }}}}" != "skipped"'):
            errors.append(f"gate must require {job} skipped on tag reuse")
    return errors


def deploy_downloads_same_sha_artifact(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    required = (
        "actions/download-artifact",
        "artifact-ids: ${{ needs.same-sha-main-evidence.outputs.artifact_id }}",
        "github-token: ${{ github.token }}",
        "repository: ${{ github.repository }}",
        "run-id: ${{ needs.same-sha-main-evidence.outputs.source_run_id }}",
    )
    return all(item in text for item in required)


def deploy_logs_reused_evidence(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    required = (
        "needs.same-sha-main-evidence.outputs.source_run_id",
        "needs.same-sha-main-evidence.outputs.check_suite_id",
        "needs.same-sha-main-evidence.outputs.artifact_id",
        "needs.same-sha-main-evidence.outputs.source_sha",
    )
    return all(item in text for item in required)


def job_permission_has(job_lines: list[str], permission: str, value: str) -> bool:
    return any(re.match(rf"^\s+{re.escape(permission)}:\s*{re.escape(value)}\s*$", strip_comment(line)) for line in job_lines)


def workflow_permissions_have_actions_read(workflow_text: str) -> bool:
    return re.search(r"(?m)^permissions:\n(?:^\s+[A-Za-z0-9_-]+:\s+\w+\n)*^\s+actions:\s+read\s*$", workflow_text) is not None


def branch_body(gate_text: str, keyword: str, condition: str) -> str | None:
    pattern = re.compile(
        rf"^\s*{keyword}\s+\[\[\s*{re.escape(condition)}\s*\]\];\s*then\s*$\n(?P<body>.*?)(?=^\s*(?:elif|else|fi)\b)",
        re.MULTILINE | re.DOTALL,
    )
    match = pattern.search(gate_text)
    if match is None:
        return None
    return match.group("body")


def branch_exists(gate_text: str, keyword: str, condition: str) -> bool:
    return branch_body(gate_text, keyword, condition) is not None


def branch_exits(gate_text: str, keyword: str, condition: str) -> bool:
    body = branch_body(gate_text, keyword, condition)
    if body is None:
        return False
    return body_exits(body)


def body_exits(body: str) -> bool:
    exit_codes: list[str | None] = []
    depth = 0
    for line in body.splitlines():
        if FI_RE.match(line):
            depth = max(0, depth - 1)
            continue
        branch_match = IF_OR_ELIF_RE.match(line)
        if branch_match:
            if branch_match.group(1) == "if":
                depth += 1
            continue
        if ELSE_RE.match(line):
            continue
        match = EXIT_RE.match(line)
        if depth == 0 and match:
            exit_codes.append(match.group(1))
    return exit_codes == ["1"]


def extract_action_input_block(action_text: str, input_name: str) -> list[str]:
    lines = action_text.splitlines()
    input_re = re.compile(rf"^  {re.escape(input_name)}:\s*$")
    next_input_re = re.compile(r"^  [A-Za-z0-9_.-]+:\s*$")
    for start, line in enumerate(lines):
        if not input_re.match(strip_comment(line)):
            continue
        end = len(lines)
        for index in range(start + 1, len(lines)):
            clean = strip_comment(lines[index])
            if clean and not clean.startswith((" ", "\t")):
                end = index
                break
            if next_input_re.match(clean):
                end = index
                break
        return lines[start:end]
    return []


def input_block_has_default_false(input_block: list[str]) -> bool:
    return any(re.match(r"^\s+default:\s*(['\"]?)false\1\s*$", strip_comment(line)) for line in input_block)


def action_step_line(action_text: str, step_name: str) -> int | None:
    pattern = re.compile(rf"^\s+-\s+name:\s*{re.escape(step_name)}\s*$")
    for line_number, line in enumerate(action_text.splitlines(), start=1):
        if pattern.match(strip_comment(line)):
            return line_number
    return None


def extract_action_output_block(action_text: str, output_name: str) -> list[str]:
    lines = action_text.splitlines()
    output_re = re.compile(rf"^  {re.escape(output_name)}:\s*$")
    next_output_re = re.compile(r"^  [A-Za-z0-9_.-]+:\s*$")
    for start, line in enumerate(lines):
        if not output_re.match(strip_comment(line)):
            continue
        end = len(lines)
        for index in range(start + 1, len(lines)):
            clean = strip_comment(lines[index])
            if clean and not clean.startswith((" ", "\t")):
                end = index
                break
            if next_output_re.match(clean):
                end = index
                break
        return lines[start:end]
    return []


def verify_workflow(workflow_text: str) -> list[str]:
    errors: list[str] = job_header_indent_errors(workflow_text)
    jobs = parse_jobs(workflow_text)

    if not workflow_permissions_have_actions_read(workflow_text):
        errors.append("workflow permissions must include actions: read")

    for job in REQUIRED_JOBS:
        if job not in jobs:
            errors.append(f"missing required job {job}")

    if "fmt-check" in jobs and "detector" in extract_needs(jobs["fmt-check"]):
        errors.append("fmt-check must not need detector")

    for job in ("fmt-check", "deny", "clippy", "check-aarch64", "source-fence", "test"):
        if job in jobs and not job_skips_tag_reuse(jobs[job]):
            errors.append(f"{job} must skip on tag reuse")

    if "source-fence" in jobs and "detector" not in extract_needs(jobs["source-fence"]):
        # FR-005: #342 owns the early-fail source-fence lane, so it remains detector-gated.
        errors.append("source-fence needs detector")
    if "source-fence" in jobs and not job_runs_command(jobs["source-fence"], "just source-fence"):
        errors.append("source-fence must run just source-fence")

    if "test" in jobs and "source-fence" not in extract_needs(jobs["test"]):
        errors.append("test needs source-fence")

    if "clippy" in jobs:
        clippy_text = uncommented_text(jobs["clippy"])
        if "just check-aarch64" in clippy_text:
            errors.append("clippy must not run check-aarch64")
        if clippy_installs_aarch64_toolchain(jobs["clippy"]):
            errors.append("clippy must not install aarch64 cross compiler")

    if "check-aarch64" in jobs:
        if "detector" not in extract_needs(jobs["check-aarch64"]):
            errors.append("check-aarch64 needs detector")
        if "just check-aarch64" not in uncommented_text(jobs["check-aarch64"]):
            errors.append("check-aarch64 must run just check-aarch64")
        if not check_aarch64_installs_cross_compiler_packages(jobs["check-aarch64"]):
            errors.append("check-aarch64 must install aarch64 cross compiler packages")

    if "test" in jobs:
        test_lines = jobs["test"]
        cache_block = test_cache_block(test_lines)
        if not has_line_matching(test_lines, TEST_FAIL_FAST_FALSE_RE):
            errors.append("test matrix must set fail-fast false")
        if not has_line_matching(test_lines, TEST_MATRIX_SHARD_RE):
            errors.append("test matrix shard must be [1, 2, 3, 4]")
        if not has_run_command(test_lines, TEST_PARTITION_COMMAND):
            errors.append("test must run partitioned nextest through just test")
        if test_has_inline_shard_reproduction_command(test_lines):
            errors.append("test shard reproduction command must use YAML block scalar")
        elif not test_has_shard_reproduction_command(test_lines):
            errors.append("test must log shard reproduction command")
        if not has_line_matching(test_lines, TEST_SHARD_CACHE_RE):
            errors.append("test cache key must include matrix.shard")
        if not block_has_line_matching(cache_block, TEST_WORKSPACES_RE):
            errors.append("test cache must use rust-cache workspaces managed target mapping")
        if block_has_managed_target_cache_directories(cache_block):
            errors.append("test cache must not use opaque cache-directories for managed target dir")
        if not block_has_line_matching(cache_block, TEST_CACHE_TARGETS_TRUE_RE):
            errors.append("test cache must enable target caching")
        if not block_has_line_matching(cache_block, TEST_CACHE_WORKSPACE_CRATES_RE):
            errors.append("test cache must preserve workspace crates")
        if not block_has_line_matching(cache_block, TEST_RUST_ENV_HASH_RE):
            errors.append("test cache must enable rust environment hash key")
        if block_cache_key_contains_github_sha(cache_block):
            errors.append("test cache key must not include github.sha")

    if "build" in jobs:
        if "detector" not in extract_needs(jobs["build"]):
            errors.append("build needs detector")
        if not has_line_matching(jobs["build"], BUILD_IF_RE):
            errors.append("build must gate on needs.detector.outputs.build_required and skip tag reuse")

    if "same-sha-main-evidence" in jobs:
        if "detector" not in extract_needs(jobs["same-sha-main-evidence"]):
            errors.append("same-sha-main-evidence needs detector")
        if not has_line_matching(jobs["same-sha-main-evidence"], SAME_SHA_IF_RE):
            errors.append("same-sha-main-evidence must be tag-gated")
        if not same_sha_job_has_outputs(jobs["same-sha-main-evidence"]):
            errors.append("same-sha-main-evidence must expose source run, check suite, artifact, and SHA outputs")
        if not same_sha_job_runs_resolver(jobs["same-sha-main-evidence"]):
            errors.append("same-sha-main-evidence must run resolver script")

    if "gate" in jobs:
        gate_needs = extract_needs(jobs["gate"])
        gate_text = uncommented_text(jobs["gate"])
        for job in GATE_REQUIRED:
            if job not in gate_needs:
                errors.append(f"gate needs {job}")
            if job == "build":
                checks_result = gate_checks_build_result(gate_text)
            else:
                checks_result = gate_checks_lane_success(gate_text, job)
            if not checks_result:
                errors.append(f"gate must check needs.{job}.result")
        if "same-sha-main-evidence" not in gate_needs:
            errors.append("gate needs same-sha-main-evidence")
        errors.extend(gate_checks_same_sha_reuse(gate_text))
        if not has_line_matching(jobs["gate"], GATE_IF_RE):
            errors.append("gate must use always()")

    if "deploy" in jobs:
        deploy_needs = extract_needs(jobs["deploy"])
        for job in DEPLOY_REQUIRED_NEEDS:
            if job not in deploy_needs:
                errors.append(f"deploy needs {job}")
        if not has_line_matching(jobs["deploy"], DEPLOY_IF_RE):
            errors.append("deploy must be tag-gated")
        if not job_permission_has(jobs["deploy"], "actions", "read"):
            errors.append("deploy permissions must include actions: read")
        if not deploy_downloads_same_sha_artifact(jobs["deploy"]):
            errors.append("deploy must download same-SHA main artifact by artifact ID")
        if not deploy_logs_reused_evidence(jobs["deploy"]):
            errors.append("deploy must log reused source run, check suite, artifact, and SHA")

    for job, lines in jobs.items():
        uses_target_dir = job_uses_managed_target_dir(lines)
        opts_in = job_opts_into_managed_target_dir(lines)
        if uses_target_dir and not opts_in:
            errors.append(f"{job} uses managed target dir but setup does not opt in")
        if opts_in and not uses_target_dir:
            errors.append(f"{job} opts into managed target dir but does not use it")

    for job in TARGET_DIR_JOBS:
        if job in jobs and not job_uses_managed_target_dir(jobs[job]):
            errors.append(f"{job} must use setup.outputs.managed_target_dir")

    for job in CACHE_KEY_JOBS:
        if job in jobs and not job_has_explicit_cache_key(jobs[job]):
            errors.append(f"{job} must declare explicit rust-cache key")

    return errors


def verify_managed_workflow(workflow_text: str, workflow_name: str) -> list[str]:
    errors: list[str] = []
    jobs = parse_jobs(workflow_text)

    for job, lines in jobs.items():
        lanes = job_just_lanes(lines)
        if not lanes:
            continue
        if not setup_action_blocks(lines):
            errors.append(f"{workflow_name} {job} must use setup-environment")
            continue
        if not job_has_setup_input(lines, "claude-config-read-token", "${{ secrets.CLAUDE_CONFIG_READ_TOKEN }}"):
            errors.append(f"{workflow_name} {job} setup token must come from secrets.CLAUDE_CONFIG_READ_TOKEN")
        if not job_has_setup_input(lines, "just-version", "${{ env.JUST_VERSION }}"):
            errors.append(f"{workflow_name} {job} setup just-version must come from env.JUST_VERSION")
        if "fmt-check" in lanes:
            if not job_has_setup_input(lines, "lint-workflow-contract", '"true"'):
                errors.append(f"{workflow_name} {job} must enable workflow contract lint")
            if not job_has_setup_input(lines, "toolchain-components", "rustfmt"):
                errors.append(f"{workflow_name} {job} must install rustfmt component")
        if "clippy" in lanes and not job_has_setup_input(lines, "toolchain-components", "clippy"):
            errors.append(f"{workflow_name} {job} must install clippy component")
        if lanes.intersection({"deny", "deny-advisories"}):
            if not job_has_setup_input(lines, "include-deny-version", '"true"'):
                errors.append(f"{workflow_name} {job} must include deny version")
            if "steps.setup.outputs.deny_version" not in uncommented_text(lines):
                errors.append(f"{workflow_name} {job} must use setup.outputs.deny_version")
        if "test" in lanes:
            if not job_has_setup_input(lines, "include-nextest-version", '"true"'):
                errors.append(f"{workflow_name} {job} must include nextest version")
            if "steps.setup.outputs.nextest_version" not in uncommented_text(lines):
                errors.append(f"{workflow_name} {job} must use setup.outputs.nextest_version")
        if "check-aarch64" in lanes:
            if not job_has_setup_input(lines, "include-build-values", '"true"'):
                errors.append(f"{workflow_name} {job} must include build values")
            if not job_has_setup_input(lines, "use-default-target", '"true"'):
                errors.append(f"{workflow_name} {job} must use default target")
        if "build" in lanes:
            if not job_has_setup_input(lines, "include-build-values", '"true"'):
                errors.append(f"{workflow_name} {job} must include build values")
            if not job_has_setup_input(lines, "use-default-target", '"true"'):
                errors.append(f"{workflow_name} {job} must use default target")
            text = uncommented_text(lines)
            if "steps.setup.outputs.zig_version" not in text:
                errors.append(f"{workflow_name} {job} must use setup.outputs.zig_version")
            if "steps.setup.outputs.zigbuild_version" not in text:
                errors.append(f"{workflow_name} {job} must use setup.outputs.zigbuild_version")

    return errors


def verify_build_artifacts(workflow_text: str, workflow_name: str) -> list[str]:
    errors: list[str] = []
    if REPO_LOCAL_ARTIFACT_RE.search(uncommented_text(workflow_text.splitlines())):
        errors.append(f"{workflow_name} must not reference repo-local target release artifacts")

    jobs = parse_jobs(workflow_text)
    build = jobs.get("build")
    if build is None:
        return errors
    build_text = uncommented_text(build)
    if BINARY_PATH_COMMAND not in build_text:
        errors.append(f"{workflow_name} build must resolve artifact through rust_verification_owner binary-path")
    if 'cp "$binary_path" "$stage_dir/bolt-v2"' not in build_text:
        errors.append(f"{workflow_name} build must copy the managed binary into a staged artifact directory")
    if "steps.managed_artifact.outputs.stage_dir" not in build_text:
        errors.append(f"{workflow_name} build upload must use the staged artifact directory")
    return errors


def verify_setup_action(action_text: str) -> list[str]:
    errors: list[str] = []
    uncommented_lines = [strip_comment(line) for line in action_text.splitlines()]
    uncommented = "\n".join(uncommented_lines)
    step_lines = [action_step_line(action_text, step) for step in SETUP_ACTION_ORDERED_STEPS]
    if any(line is None for line in step_lines):
        errors.append("setup action missing required ordered steps")
    elif any(left >= right for left, right in zip(step_lines, step_lines[1:]) if left is not None and right is not None):
        errors.append("setup action step order drifted")
    for literal in SETUP_ACTION_REQUIRED_LITERALS:
        if literal not in uncommented:
            errors.append(f"setup action missing expected literal {literal!r}")
    for output_name, output_mapping in SETUP_ACTION_OUTPUT_MAPPINGS.items():
        output_block = extract_action_output_block(action_text, output_name)
        if not output_block:
            errors.append(f"setup action missing exported output {output_name!r}")
        elif output_mapping not in uncommented_text(output_block):
            errors.append(f"setup action missing output mapping for {output_name!r}")
    target_dir_input = extract_action_input_block(action_text, "include-managed-target-dir")
    if not target_dir_input:
        errors.append("setup action missing include-managed-target-dir input")
    elif not input_block_has_default_false(target_dir_input):
        errors.append("setup action include-managed-target-dir default must be false")
    if not any(SETUP_TARGET_DIR_EXPORT_RE.match(line) for line in uncommented_lines):
        errors.append("setup action must export managed_target_dir from target_dir step")
    if not any(SETUP_TARGET_DIR_RELATIVE_EXPORT_RE.match(line) for line in uncommented_lines):
        errors.append("setup action must export managed_target_dir_relative from target_dir step")
    if not any("managed_target_dir_relative=\"$(" in line and "os.path.relpath" in line for line in uncommented_lines):
        errors.append("setup action must compute managed_target_dir_relative")
    if not any(SETUP_TARGET_DIR_IF_RE.match(line) for line in uncommented_lines):
        errors.append("setup action target dir step must be conditional")
    return errors


def verify_text(workflow_text: str, action_text: str) -> list[str]:
    return verify_workflows({"ci.yml": workflow_text}, action_text)


def verify_workflows(workflows: dict[str, str], action_text: str) -> list[str]:
    errors: list[str] = []
    for workflow_name, workflow_text in workflows.items():
        if workflow_name == "ci.yml" or workflow_name.endswith("/ci.yml"):
            errors.extend(verify_workflow(workflow_text))
        errors.extend(verify_managed_workflow(workflow_text, workflow_name))
        errors.extend(verify_build_artifacts(workflow_text, workflow_name))
    errors.extend(verify_setup_action(action_text))
    return errors


def main() -> int:
    workflow_texts = {workflow.relative_to(REPO_ROOT).as_posix(): workflow.read_text() for workflow in DEFAULT_WORKFLOWS if workflow.exists()}
    action_text = DEFAULT_SETUP_ACTION.read_text()
    errors = verify_workflows(workflow_texts, action_text)
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: CI workflow hygiene verifier passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
