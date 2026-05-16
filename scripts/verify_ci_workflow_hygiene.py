#!/usr/bin/env python3
"""Verify CI workflow hygiene invariants for the current workflow topology."""

from __future__ import annotations

import pathlib
import re
import sys
import tomllib


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci.yml"
DEFAULT_WORKFLOWS = (
    DEFAULT_WORKFLOW,
    REPO_ROOT / ".github" / "workflows" / "advisory.yml",
)
DEFAULT_SETUP_ACTION = REPO_ROOT / ".github" / "actions" / "setup-environment" / "action.yml"
DEFAULT_NEXTEST_CONFIG = REPO_ROOT / ".config" / "nextest.toml"

REQUIRED_JOBS = (
    "detector",
    "fmt-check",
    "deny",
    "clippy",
    "check-aarch64",
    "source-fence",
    "test-archive",
    "test-shards",
    "test",
    "build",
    "gate",
    "deploy",
)
GATE_REQUIRED = ("detector", "fmt-check", "deny", "clippy", "check-aarch64", "source-fence", "test", "build")
DEPLOY_REQUIRED_NEEDS = (
    "gate",
    "build",
    "detector",
    "fmt-check",
    "deny",
    "clippy",
    "check-aarch64",
    "source-fence",
    "test",
)
TARGET_DIR_JOBS = ("clippy", "check-aarch64", "source-fence", "build")
CACHE_KEY_JOBS = ("deny", "clippy", "check-aarch64", "source-fence", "test-archive", "build")
LIVE_NODE_TEST_GROUP = "live-node"
LIVE_NODE_UNIT_TEST_FILTERS = (
    "binary(=bolt_v2)",
    "test(~bolt_v3_client_registration::tests::)",
    "test(~bolt_v3_live_node::tests::)",
    "test(~platform::runtime::tests::)",
)
LIVE_NODE_NEXTEST_BINARIES = (
    "bolt_v3_adapter_mapping",
    "bolt_v3_client_registration",
    "bolt_v3_controlled_connect",
    "bolt_v3_credential_log_suppression",
    "bolt_v3_live_canary_gate",
    "bolt_v3_readiness",
    "bolt_v3_strategy_registration",
    "bolt_v3_submit_admission",
    "bolt_v3_tiny_canary_operator",
    "config_parsing",
    "eth_chainlink_taker_runtime",
    "lake_batch",
    "live_node_run",
    "nt_runtime_capture",
    "platform_runtime",
    "polymarket_bootstrap",
    "venue_contract",
)
LIVE_NODE_NEXTEST_FILTER = " | ".join(f"binary(={binary})" for binary in LIVE_NODE_NEXTEST_BINARIES)
BUILD_IF_RE = re.compile(r"^    if:\s*(?:\$\{\{\s*)?needs\.detector\.outputs\.build_required\s*==\s*['\"]true['\"]\s*(?:\}\})?\s*$")
GATE_IF_RE = re.compile(r"^    if:\s*(?:\$\{\{\s*)?always\(\)\s*(?:\}\})?\s*$")
DEPLOY_IF_RE = re.compile(r"^    if:\s*(?:\$\{\{\s*)?startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)\s*(?:\}\})?\s*$")
EXIT_RE = re.compile(r"^\s*exit(?:\s+([0-9]+))?\s*$", re.MULTILINE)
IF_OR_ELIF_RE = re.compile(r"^\s*(if|elif)\s+\[\[\s*(?P<condition>.*?)\s*\]\];\s*then\s*$")
ELSE_RE = re.compile(r"^\s*else\s*$")
FI_RE = re.compile(r"^\s*fi\s*$")
TARGET_DIR_OPT_IN_RE = re.compile(r"^\s+include-managed-target-dir:\s*(['\"])true\1\s*$")
SETUP_TARGET_DIR_EXPORT_RE = re.compile(r"^\s+value:\s*\$\{\{\s*steps\.target_dir\.outputs\.managed_target_dir\s*\}\}\s*$")
SETUP_TARGET_DIR_RELATIVE_EXPORT_RE = re.compile(
    r"^\s+value:\s*\$\{\{\s*steps\.target_dir\.outputs\.managed_target_dir_relative\s*\}\}\s*$"
)
SETUP_TARGET_DIR_RELATIVE_OUTPUT_RE = re.compile(
    r'^\s*echo\s+"managed_target_dir_relative=\$managed_target_dir_relative"\s*>>\s*"\$GITHUB_OUTPUT"\s*$'
)
SETUP_TARGET_DIR_RELATIVE_COMPUTE = (
    "managed_target_dir_relative=\"$(python3 -c 'import os, sys; "
    "print(os.path.relpath(sys.argv[2], sys.argv[1]))' \"$GITHUB_WORKSPACE\" \"$managed_target_dir\")\""
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
    "CLAUDE_CONFIG_READ_TOKEN:",
    "inputs.claude-config-read-token",
    "just ci-lint-workflow",
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
    "os.path.relpath",
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
TEST_SHARD_NAME_RE = re.compile(r"^\s+name:\s*nextest shard \$\{\{\s*matrix\.shard\s*\}\} of 4\s*$")
TEST_PARTITION_COMMAND = (
    'just test-archive-run "$RUNNER_TEMP/nextest-archive/nextest-archive.tar.zst" '
    '"${{ steps.archive-root.outputs.archive_extract_root }}" '
    "--partition count:${{ matrix.shard }}/4"
)
TEST_REPRODUCTION_COMMAND = (
    "just test-archive-run .nextest-archive/nextest-archive.tar.zst "
    "<managed-target-parent> "
    "--partition count:${{ matrix.shard }}/4"
)
TEST_REPRODUCTION_ECHO = f'echo "reproduce locally: {TEST_REPRODUCTION_COMMAND}"'
TEST_ARCHIVE_EXTRACT_ROOT_COMMAND = 'archive_extract_root="$(dirname "${{ steps.setup.outputs.managed_target_dir }}")"'
TEST_ARCHIVE_EXTRACT_ROOT_OUTPUT = 'echo "archive_extract_root=$archive_extract_root" >> "$GITHUB_OUTPUT"'
TEST_ARCHIVE_KEY_INPUTS = (
    "key: nextest-archive-v1-${{ runner.os }}-${{ runner.arch }}-test-profile-shards-4-${{ hashFiles(",
    "'Cargo.lock'",
    "'Cargo.toml'",
    "'rust-toolchain.toml'",
    "'.cargo/config.toml'",
    "'.config/nextest.toml'",
    "'.claude/rust-verification.toml'",
    "'justfile'",
    "'src/**/*.rs'",
    "'tests/**/*.rs'",
    "'benches/**/*.rs'",
    "'examples/**/*.rs'",
)
TEST_ARCHIVE_PATH = "NEXTEST_ARCHIVE_PATH: .nextest-archive/nextest-archive.tar.zst"
TEST_ARCHIVE_CACHE_PATH = "path: ${{ env.NEXTEST_ARCHIVE_PATH }}"
TEST_ARCHIVE_CACHE_HIT_GUARD = "if: steps.nextest-archive-cache.outputs.cache-hit != 'true'"
TEST_ARCHIVE_RESTORE_ACTION = "uses: actions/cache/restore@0057852bfaa89a56745cba8c7296529d2fc39830"
TEST_ARCHIVE_SAVE_ACTION = "uses: actions/cache/save@0057852bfaa89a56745cba8c7296529d2fc39830"
TEST_ARCHIVE_UPLOAD_ACTION = "uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a"
TEST_ARCHIVE_DOWNLOAD_ACTION = "uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c"
CACHE_KEY_RE = re.compile(r"^\s+(?:key|shared-key):\s*\S+.*$")
JUST_LANE_RE = re.compile(
    r"(^|[^A-Za-z0-9_./-])just\s+"
    r"(fmt-check|deny|deny-advisories|clippy|test-archive-run|test-archive|test|build|check-aarch64|source-fence)"
    r"([^A-Za-z0-9_]|$)"
)
REPO_LOCAL_ARTIFACT_RE = re.compile(r"(^|[^A-Za-z0-9_./-])target/(?:.*/)?release/bolt-v2(?:\.sha256)?([^A-Za-z0-9_./-]|$)")
BINARY_PATH_COMMAND = 'python3 "${{ steps.setup.outputs.rust_verification_owner }}" binary-path --repo "$GITHUB_WORKSPACE" --bin bolt-v2'


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
    return any(
        "steps.setup.outputs.managed_target_dir" in strip_comment(line)
        or "steps.setup.outputs.managed_target_dir_relative" in strip_comment(line)
        for line in job_lines
    )


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


def test_has_shard_reproduction_command(job_lines: list[str]) -> bool:
    return job_runs_command(job_lines, TEST_REPRODUCTION_ECHO)


def test_has_inline_shard_reproduction_command(job_lines: list[str]) -> bool:
    for block in step_blocks(job_lines):
        for line in block:
            clean = strip_comment(line).strip()
            if clean.startswith(("run:", "- run:")) and "reproduce" in clean.lower() and TEST_REPRODUCTION_COMMAND in clean:
                return True
    return False


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

    for job in REQUIRED_JOBS:
        if job not in jobs:
            errors.append(f"missing required job {job}")

    if "fmt-check" in jobs and "detector" in extract_needs(jobs["fmt-check"]):
        errors.append("fmt-check must not need detector")

    if "source-fence" in jobs and "detector" not in extract_needs(jobs["source-fence"]):
        # FR-005: #342 owns the early-fail source-fence lane, so it remains detector-gated.
        errors.append("source-fence needs detector")
    if "source-fence" in jobs and not job_runs_command(jobs["source-fence"], "just source-fence"):
        errors.append("source-fence must run just source-fence")

    if "test-archive" in jobs and "source-fence" not in extract_needs(jobs["test-archive"]):
        errors.append("test-archive needs source-fence")
    if "test-shards" in jobs and "test-archive" not in extract_needs(jobs["test-shards"]):
        errors.append("test-shards needs test-archive")

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

    if "test-archive" in jobs:
        archive_lines = jobs["test-archive"]
        archive_text = uncommented_text(archive_lines)
        if TEST_ARCHIVE_PATH not in archive_text:
            errors.append("test-archive must declare nextest archive path")
        if not all(input_fragment in archive_text for input_fragment in TEST_ARCHIVE_KEY_INPUTS):
            errors.append("test-archive cache key must include Rust and test graph inputs")
        if "Swatinem/rust-cache@" in archive_text:
            errors.append("test-archive must not use managed target rust-cache")
        if "include-managed-target-dir:" in archive_text:
            errors.append("test-archive must not opt into managed target dir")
        if "nextest-archive-build-v1" in archive_text:
            errors.append("test-archive must not save a second archive-build cache")
        if TEST_ARCHIVE_RESTORE_ACTION not in archive_text:
            errors.append("test-archive must restore nextest archive cache")
        if TEST_ARCHIVE_SAVE_ACTION not in archive_text:
            errors.append("test-archive must save nextest archive cache")
        if TEST_ARCHIVE_UPLOAD_ACTION not in archive_text:
            errors.append("test-archive must upload nextest archive artifact")
        if "restore-keys:" in archive_text:
            errors.append("test-archive cache must not use restore-keys")
        if archive_text.count(TEST_ARCHIVE_CACHE_PATH) < 2:
            errors.append("test-archive cache must use archive path env")
        if archive_text.count(TEST_ARCHIVE_CACHE_HIT_GUARD) < 3:
            errors.append("test-archive build must be skipped on archive cache hit")
        if not job_runs_command(archive_lines, 'just test-archive "$NEXTEST_ARCHIVE_PATH"'):
            errors.append("test-archive must build through just test-archive")

    if "test-shards" in jobs:
        test_lines = jobs["test-shards"]
        test_text = uncommented_text(test_lines)
        if not has_line_matching(test_lines, TEST_FAIL_FAST_FALSE_RE):
            errors.append("test-shards matrix must set fail-fast false")
        if not has_line_matching(test_lines, TEST_MATRIX_SHARD_RE):
            errors.append("test-shards matrix shard must be [1, 2, 3, 4]")
        if not has_line_matching(test_lines, TEST_SHARD_NAME_RE):
            errors.append("test-shards name must describe nextest shard")
        if not job_has_setup_input(test_lines, "include-managed-target-dir", '"true"'):
            errors.append("test-shards must resolve managed target dir")
        if (
            TEST_ARCHIVE_EXTRACT_ROOT_COMMAND not in test_text
            or TEST_ARCHIVE_EXTRACT_ROOT_OUTPUT not in test_text
        ):
            errors.append("test-shards must extract archive to managed target parent")
        if not has_run_command(test_lines, TEST_PARTITION_COMMAND):
            errors.append("test-shards must run partitioned nextest from archive")
        if test_has_inline_shard_reproduction_command(test_lines):
            errors.append("test-shards reproduction command must use YAML block scalar")
        elif not test_has_shard_reproduction_command(test_lines):
            errors.append("test-shards must log shard reproduction command")
        if TEST_ARCHIVE_DOWNLOAD_ACTION not in test_text:
            errors.append("test-shards must download nextest archive artifact")
        if "Swatinem/rust-cache" in test_text:
            errors.append("test-shards must not restore a per-shard Rust target cache")

    if "test" in jobs:
        test_needs = extract_needs(jobs["test"])
        test_text = uncommented_text(jobs["test"])
        if "test-shards" not in test_needs:
            errors.append("test needs test-shards")
        if not gate_checks_lane_success(test_text, "test-shards"):
            errors.append("test must check needs.test-shards.result")
        if not has_line_matching(jobs["test"], GATE_IF_RE):
            errors.append("test must use always()")

    if "build" in jobs:
        if "detector" not in extract_needs(jobs["build"]):
            errors.append("build needs detector")
        if not has_line_matching(jobs["build"], BUILD_IF_RE):
            errors.append("build must gate on needs.detector.outputs.build_required")

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
        if not has_line_matching(jobs["gate"], GATE_IF_RE):
            errors.append("gate must use always()")

    if "deploy" in jobs:
        deploy_needs = extract_needs(jobs["deploy"])
        for job in DEPLOY_REQUIRED_NEEDS:
            if job not in deploy_needs:
                errors.append(f"deploy needs {job}")
        if not has_line_matching(jobs["deploy"], DEPLOY_IF_RE):
            errors.append("deploy must be tag-gated")

    for job, lines in jobs.items():
        uses_target_dir = job_uses_managed_target_dir(lines)
        opts_in = job_opts_into_managed_target_dir(lines)
        if uses_target_dir and not opts_in:
            errors.append(f"{job} uses managed target dir but setup does not opt in")
        if opts_in and not uses_target_dir:
            errors.append(f"{job} opts into managed target dir but does not use it")

    for job in TARGET_DIR_JOBS:
        if job in jobs and not job_uses_managed_target_dir(jobs[job]):
            errors.append(f"{job} must use setup.outputs.managed_target_dir or managed_target_dir_relative")

    for job in CACHE_KEY_JOBS:
        if job in jobs and not job_has_explicit_cache_key(jobs[job]):
            errors.append(f"{job} must declare explicit rust-cache key or shared-key")

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
        if lanes.intersection({"test", "test-archive", "test-archive-run"}):
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
    if not any(line.strip() == SETUP_TARGET_DIR_RELATIVE_COMPUTE for line in uncommented_lines):
        errors.append("setup action target_dir step must compute managed_target_dir_relative from workspace to target dir")
    if not any(SETUP_TARGET_DIR_RELATIVE_OUTPUT_RE.match(line) for line in uncommented_lines):
        errors.append("setup action target_dir step must write managed_target_dir_relative")
    if not any(SETUP_TARGET_DIR_IF_RE.match(line) for line in uncommented_lines):
        errors.append("setup action target dir step must be conditional")
    return errors


def verify_nextest_config(config_text: str) -> list[str]:
    errors: list[str] = []
    try:
        config = tomllib.loads(config_text)
    except tomllib.TOMLDecodeError as exc:
        return [f"nextest config invalid TOML: {exc}"]

    groups = config.get("test-groups", {})
    if not isinstance(groups, dict):
        groups = {}
    live_node_group = groups.get(LIVE_NODE_TEST_GROUP)
    if not isinstance(live_node_group, dict):
        errors.append("nextest config missing live-node test group")
    elif live_node_group.get("max-threads") != 1:
        errors.append("nextest live-node test group max-threads must be 1")

    profile = config.get("profile", {})
    default_profile = profile.get("default", {}) if isinstance(profile, dict) else {}
    overrides = default_profile.get("overrides", []) if isinstance(default_profile, dict) else []
    if not isinstance(overrides, list):
        overrides = []
    live_node_filters = [
        override.get("filter")
        for override in overrides
        if isinstance(override, dict) and override.get("test-group") == LIVE_NODE_TEST_GROUP
    ]
    missing_binaries = [
        binary
        for binary in LIVE_NODE_NEXTEST_BINARIES
        if not any(isinstance(filter_expr, str) and f"binary(={binary})" in filter_expr for filter_expr in live_node_filters)
    ]
    missing_unit_filters = [
        fragment
        for fragment in LIVE_NODE_UNIT_TEST_FILTERS
        if not any(isinstance(filter_expr, str) and fragment in filter_expr for filter_expr in live_node_filters)
    ]
    if missing_binaries or missing_unit_filters:
        missing = ", ".join(
            [f"binary(={binary})" for binary in missing_binaries] + missing_unit_filters
        )
        errors.append(f"nextest config must assign LiveNode test paths to live-node group: missing {missing}")
    return errors


def verify_text(workflow_text: str, action_text: str, nextest_config_text: str) -> list[str]:
    return verify_workflows({"ci.yml": workflow_text}, action_text, nextest_config_text)


def verify_workflows(workflows: dict[str, str], action_text: str, nextest_config_text: str) -> list[str]:
    errors: list[str] = []
    for workflow_name, workflow_text in workflows.items():
        if workflow_name == "ci.yml" or workflow_name.endswith("/ci.yml"):
            errors.extend(verify_workflow(workflow_text))
        errors.extend(verify_managed_workflow(workflow_text, workflow_name))
        errors.extend(verify_build_artifacts(workflow_text, workflow_name))
    errors.extend(verify_setup_action(action_text))
    errors.extend(verify_nextest_config(nextest_config_text))
    return errors


def main() -> int:
    workflow_texts = {workflow.relative_to(REPO_ROOT).as_posix(): workflow.read_text() for workflow in DEFAULT_WORKFLOWS if workflow.exists()}
    action_text = DEFAULT_SETUP_ACTION.read_text()
    nextest_config_text = DEFAULT_NEXTEST_CONFIG.read_text()
    errors = verify_workflows(workflow_texts, action_text, nextest_config_text)
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: CI workflow hygiene verifier passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
