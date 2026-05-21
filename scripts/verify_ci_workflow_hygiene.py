#!/usr/bin/env python3
"""Verify CI workflow hygiene invariants for the current workflow topology."""

from __future__ import annotations

import pathlib
import re
import shlex
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
DEFAULT_NO_MISTAKES_CONFIG = REPO_ROOT / ".no-mistakes.yaml"
DEFAULT_REPO_AUTOMATION_FILES = (REPO_ROOT / "justfile",)
DEFAULT_REPO_AUTOMATION_GLOBS = (
    (REPO_ROOT / "scripts", "*.sh"),
    (REPO_ROOT / "tests", "*.sh"),
)
S3_ACTIVE_TARGET_CACHE_MESSAGE = "S3 active mutable target cache must be rejected"

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
TAG_SKIPPED_JOBS = ("fmt-check", "deny", "clippy", "source-fence", "test", "build")
TAG_SKIP_REQUIRED_JOBS = (
    "fmt-check",
    "deny",
    "clippy",
    "source-fence",
    "test-archive",
    "test-shards",
    "test",
)
TARGET_DIR_JOBS = ("clippy", "check-aarch64", "source-fence", "test-shards", "build")
CACHE_KEY_JOBS = ("deny", "clippy", "check-aarch64", "source-fence", "test-archive", "build")
JOB_REQUIRED_JUST_RECIPE = {
    "fmt-check": "fmt-check",
    "deny": "deny",
    "clippy": "clippy",
    "check-aarch64": "check-aarch64",
    "source-fence": "source-fence",
    "build": "build",
}
CI_PR_PATHS_IGNORE_BASELINE = (
    ".claude/**",
    ".codex/**",
    ".gemini/**",
    ".github/ISSUE_TEMPLATE/**",
    ".opencode/**",
    ".pi/**",
    ".specify/**",
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    "LICENSE",
    "REASONIX.md",
    "SECURITY.md",
)
LIVE_NODE_TEST_GROUP = "live-node"
LIVE_NODE_UNIT_TEST_FILTERS = (
    "binary(=bolt_v2)",
    "test(~bolt_v3_client_registration::tests::)",
    "test(~bolt_v3_live_node::tests::)",
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
    "lake_batch",
    "nt_runtime_capture",
    "venue_contract",
)
LIVE_NODE_NEXTEST_FILTER = " | ".join(f"binary(={binary})" for binary in LIVE_NODE_NEXTEST_BINARIES)
CHECK_AARCH64_JOB_LEVEL_IF_RE = re.compile(r"^    if:\s*.*$")
CHECK_AARCH64_STANDALONE_IF_RE = re.compile(
    r"^\s+(?:-\s*)?if:\s*(?:\$\{\{\s*)?needs\.detector\.outputs\.build_required\s*!=\s*['\"]true['\"]\s*(?:\}\})?\s*$"
)
TAG_SKIP_IF_RE = re.compile(r"^    if:\s*(?:\$\{\{\s*)?!startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)\s*(?:\}\})?\s*$")
TAG_SKIP_ALWAYS_IF_RE = re.compile(
    r"^    if:\s*\$\{\{\s*(?:"
    r"!startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)\s*&&\s*always\(\)"
    r"|always\(\)\s*&&\s*!startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)"
    r")\s*\}\}\s*$"
)
SAME_SHA_IF_RE = re.compile(r"^    if:\s*(?:\$\{\{\s*)?startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)\s*(?:\}\})?\s*$")
BUILD_IF_RE = re.compile(
    r"^    if:\s*\$\{\{\s*!startsWith\(github\.ref,\s*['\"]refs/tags/v['\"]\)\s*&&\s*"
    r"needs\.detector\.outputs\.build_required\s*==\s*['\"]true['\"]\s*\}\}\s*$"
)
PR_CONCURRENCY_EVENT_RE = re.compile(r"github\.event_name\s*==\s*['\"]pull_request['\"]")
PR_CONCURRENCY_PULL_REQUEST_BRANCH_RE = re.compile(
    r"github\.event_name\s*==\s*['\"]pull_request['\"]\s*&&\s*"
    r"format\(\s*['\"]pr-\{0\}['\"]\s*,\s*github\.event\.number\s*\)"
)
PR_CONCURRENCY_NON_PR_FALLBACK_RE = re.compile(
    r"\|\|\s*format\(\s*['\"]\{0\}-\{1\}['\"]\s*,\s*github\.ref_name\s*,\s*github\.sha\s*\)"
)
PR_CONCURRENCY_CANCEL_LINES = (
    "cancel-in-progress: ${{ github.event_name == 'pull_request' }}",
    'cancel-in-progress: ${{ github.event_name == "pull_request" }}',
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
    "just ci-lint-workflow",
    "awk -F'\\\"' '/^channel = / {print $2}' rust-toolchain.toml",
    "just --evaluate deny_version",
    "just --evaluate nextest_version",
    "just --evaluate target",
    "just --evaluate zig_version",
    "just --evaluate zigbuild_version",
    "just --evaluate zigbuild_x86_64_unknown_linux_gnu_sha256",
    "just --evaluate rust_verification_owner",
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
    "zigbuild_x86_64_unknown_linux_gnu_sha256": "steps.shared.outputs.zigbuild_x86_64_unknown_linux_gnu_sha256",
    "rust_verification_owner": "steps.shared.outputs.rust_verification_owner",
    "managed_target_dir": "steps.target_dir.outputs.managed_target_dir",
    "managed_target_dir_relative": "steps.target_dir.outputs.managed_target_dir_relative",
}
SETUP_ACTION_ORDERED_STEPS = (
    "Lint workflow contract",
    "Read shared values",
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
    "'ci/rust-verification.toml'",
    "'scripts/rust_verification.py'",
    "'justfile'",
    "'build.rs'",
    "'src/**'",
    "'tests/**'",
    "'benches/**'",
    "'examples/**'",
    "'crates/**'",
    "'specs/**/*.md'",
)
EXACT_HEAD_GOVERNANCE_CACHE_INPUTS = (
    "'.github/workflows/ci.yml'",
    "'.github/actions/setup-environment/action.yml'",
    "'.no-mistakes.yaml'",
)
TEST_ARCHIVE_PATH = "NEXTEST_ARCHIVE_PATH: .nextest-archive/nextest-archive.tar.zst"
TEST_ARCHIVE_CACHE_PATH = "path: ${{ env.NEXTEST_ARCHIVE_PATH }}"
TEST_ARCHIVE_CACHE_HIT_GUARD = "if: steps.nextest-archive-cache.outputs.cache-hit != 'true'"
TEST_ARCHIVE_RESTORE_ACTION = "uses: actions/cache/restore@27d5ce7f107fe9357f9df03efb73ab90386fccae"
TEST_ARCHIVE_SAVE_ACTION = "uses: actions/cache/save@27d5ce7f107fe9357f9df03efb73ab90386fccae"
TEST_ARCHIVE_UPLOAD_ACTION = "uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a"
TEST_ARCHIVE_DOWNLOAD_ACTION = "uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c"
CACHE_KEY_RE = re.compile(r"^\s+(?:key|shared-key):\s*\S+.*$")
SHARED_REGISTRY_CACHE_KEY = "cargo-registry-git-v1"
SHARED_REGISTRY_SAVE_IF = "${{ github.job == 'test-archive' }}"
REGISTRY_CACHE_JOBS = ("deny", "clippy", "check-aarch64", "source-fence", "test-archive", "build")
# Jobs that opt into the managed-target actions/cache. Each value is the
# job-specific key prefix segment between `managed-target-v1-${runner.os}-
# ${runner.arch}-` and the hashFiles suffix. Adding a new job that uses
# `steps.setup.outputs.managed_target_dir` requires (a) registering its
# expected prefix here so `managed_target_cache_errors` enforces key isolation
# AND a matching `restore-keys` prefix fallback (#400), and (b) updating the
# self-test fixture in `scripts/test_verify_ci_workflow_hygiene.py`.
MANAGED_TARGET_CACHE_KEYS = {
    "clippy": "clippy-host",
    "check-aarch64": "check-aarch64-dev",
    "source-fence": "source-fence-test",
    "build": "build-aarch64-release",
}
JUST_LANE_RE = re.compile(
    r"(^|[^A-Za-z0-9_./-])just\s+"
    r"(fmt-check|deny|deny-advisories|clippy|test-archive-run|test-archive|test|build|check-aarch64|source-fence)"
    r"([^A-Za-z0-9_]|$)"
)
REPO_LOCAL_ARTIFACT_RE = re.compile(r"(^|[^A-Za-z0-9_./-])target/(?:.*/)?release/bolt-v2(?:\.sha256)?([^A-Za-z0-9_./-]|$)")
BINARY_PATH_COMMAND = 'python3 "${{ steps.setup.outputs.rust_verification_owner }}" binary-path --repo "$GITHUB_WORKSPACE" --bin bolt-v2'
# taiki-e/install-action must be pinned to a 40-hex commit SHA (mutable tags
# like @v2 are rejected). The specific SHA is NOT enforced here — Dependabot
# opens a PR with release notes for every bump and PR review is the human
# gate. See tj-actions/changed-files (CVE-2025-30066, March 2025) for why
# SHA-pinning matters and why hardcoding a specific SHA here adds maintenance
# burden without real supply-chain value.
#
# Two regexes intentionally:
#   * TAIKI_INSTALL_ACTION_RE matches well-formed pinned single-line `uses:`
#     references. Optional matching quotes (single OR double, enforced by
#     backreference so mismatched quotes still fail) are accepted around the
#     reference. Uppercase hex is allowed in the match so the consistency
#     check can normalize via .lower() rather than silently rejecting valid
#     uppercase pins. The SHA is captured in group(2); group(1) is the
#     (possibly empty) opening quote used by the backreference.
#   * TAIKI_INSTALL_ACTION_MENTION_RE is a broad detector for any cleaned
#     line that mentions `taiki-e/install-action@` at all — whether the
#     `uses:` token sits on the same line (single-line form) or on a
#     preceding line (YAML multi-line scalar form). The consistency check
#     uses it to surface every reference, then requires the line to match
#     the strict single-line pinned form; anything else (mutable tag,
#     mismatched quotes, multi-line scalar) is reported with a precise
#     file:line.
TAIKI_INSTALL_ACTION_RE = re.compile(
    r"""^\s*(?:-\s*)?uses:\s*(['"]?)taiki-e/install-action@([0-9a-fA-F]{40})\1\s*$"""
)
TAIKI_INSTALL_ACTION_MENTION_RE = re.compile(r"\btaiki-e/install-action@")
CI_INSTALL_ACTION_TOOLS = {
    "deny": ("cargo-deny", "steps.setup.outputs.deny_version"),
    "advisories": ("cargo-deny", "steps.setup.outputs.deny_version"),
    "test-archive": ("cargo-nextest", "steps.setup.outputs.nextest_version"),
    "test-shards": ("cargo-nextest", "steps.setup.outputs.nextest_version"),
}
CI_SOURCE_BUILD_TOOLS = ("cargo-deny", "cargo-nextest", "cargo-zigbuild")
CI_INSTALL_ACTION_COMMANDS = {
    "deny": "just deny",
    "advisories": "just deny-advisories",
    "test-archive": 'just test-archive "$NEXTEST_ARCHIVE_PATH"',
    "test-shards": TEST_PARTITION_COMMAND,
}
CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT = {
    "--color",
    "--config",
    "--jobs",
    "--manifest-path",
    "--profile",
    "--target",
    "--target-dir",
    "-C",
    "-Z",
}
CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT = {"--frozen", "--locked", "--offline", "--quiet", "-q", "--verbose", "-v"}
ZIGBUILD_PREBUILT_LITERALS = (
    'version="${{ steps.setup.outputs.zigbuild_version }}"',
    'archive="cargo-zigbuild-x86_64-unknown-linux-gnu.tar.xz"',
    "https://github.com/rust-cross/cargo-zigbuild/releases/download/v${version}",
    "curl \\",
    "--retry 10",
    "--retry-delay 3",
    "--retry-all-errors",
    "--fail",
    "--location",
    "--show-error",
    "--silent",
    '--output "$archive"',
    '"$base_url/$archive"',
    'expected="${{ steps.setup.outputs.zigbuild_x86_64_unknown_linux_gnu_sha256 }}"',
    'actual="$(sha256sum "$archive" | awk \'{print $1}\')"',
    'test "$actual" = "$expected"',
    'tar --extract --xz --file "$archive"',
    'mkdir -p "$HOME/.cargo/bin"',
    'mv cargo-zigbuild-x86_64-unknown-linux-gnu/cargo-zigbuild "$HOME/.cargo/bin/cargo-zigbuild"',
    'chmod +x "$HOME/.cargo/bin/cargo-zigbuild"',
    'test -x "$HOME/.cargo/bin/cargo-zigbuild" && true',
)


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


def extract_paths_ignore_for_trigger(
    workflow_text: str, trigger: str
) -> tuple[str, ...] | None:
    """Return the paths-ignore list under `on.<trigger>`, or None if absent.

    Parses the block-style YAML this repo uses; flow-style maps are not supported.
    """

    lines = [strip_comment(line).rstrip() for line in workflow_text.splitlines()]

    def section_index(start: int, header: str, max_indent: int) -> int | None:
        i = start
        while i < len(lines):
            line = lines[i]
            if line and len(line) - len(line.lstrip(" ")) <= max_indent and line != header:
                return None
            if line == header:
                return i
            i += 1
        return None

    on_idx = section_index(0, "on:", max_indent=-1)
    if on_idx is None:
        return None
    trigger_idx = section_index(on_idx + 1, f"  {trigger}:", max_indent=0)
    if trigger_idx is None:
        return None
    pi_idx = section_index(trigger_idx + 1, "    paths-ignore:", max_indent=2)
    if pi_idx is None:
        return None

    items: list[str] = []
    for i in range(pi_idx + 1, len(lines)):
        line = lines[i]
        if line and len(line) - len(line.lstrip(" ")) <= 4:
            break
        stripped = line.lstrip()
        if stripped.startswith("- "):
            items.append(stripped[2:].strip().strip("'").strip('"'))
    return tuple(items)


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


def top_level_block(workflow_text: str, key: str) -> list[str]:
    lines = workflow_text.splitlines()
    start_line = f"{key}:"
    for index, line in enumerate(lines):
        clean = strip_comment(line)
        if clean != start_line:
            continue
        block: list[str] = []
        for child_line in lines[index + 1 :]:
            child_clean = strip_comment(child_line)
            if child_clean and not child_clean.startswith((" ", "\t")):
                break
            block.append(child_clean)
        return block
    return []


def verify_pr_concurrency(workflow_text: str) -> list[str]:
    block = top_level_block(workflow_text, "concurrency")
    if not block:
        return ["workflow must define PR-only concurrency"]

    group_lines: list[str] = []
    cancel_lines: list[str] = []
    seen_cancel = False
    for line in block:
        if line.strip().startswith("cancel-in-progress:"):
            seen_cancel = True
        if seen_cancel:
            cancel_lines.append(line)
        else:
            group_lines.append(line)

    group_text = " ".join(line.strip() for line in group_lines if line.strip())
    cancel_text = "\n".join(cancel_lines)
    errors: list[str] = []
    if not PR_CONCURRENCY_EVENT_RE.search(group_text):
        errors.append("concurrency group must branch on pull_request event")
    if not PR_CONCURRENCY_PULL_REQUEST_BRANCH_RE.search(group_text):
        errors.append("concurrency group must key pull_request runs by PR number")
    if not PR_CONCURRENCY_NON_PR_FALLBACK_RE.search(group_text):
        errors.append("concurrency group must keep non-PR runs isolated by ref and SHA")
    if not any(line in cancel_text for line in PR_CONCURRENCY_CANCEL_LINES):
        errors.append("cancel-in-progress must be limited to pull_request events")
    return errors


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


def action_blocks(job_lines: list[str], action: str) -> list[list[str]]:
    return [block for block in step_blocks(job_lines) if any(action in strip_comment(line) for line in block)]


def rust_cache_blocks(job_lines: list[str]) -> list[list[str]]:
    return action_blocks(job_lines, "Swatinem/rust-cache@")


def github_cache_blocks(job_lines: list[str]) -> list[list[str]]:
    return action_blocks(job_lines, "actions/cache@")


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


def unquote_yaml_scalar(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
        return value[1:-1]
    return value


def block_input_items(block: list[str]) -> list[tuple[str, str]]:
    items: list[tuple[str, str]] = []
    with_indent: int | None = None
    input_indent: int | None = None
    for line in block:
        clean = strip_comment(line).rstrip()
        if not clean.strip():
            continue
        if with_indent is None:
            match = re.match(r"^(\s*)with:\s*$", clean)
            if match is not None:
                with_indent = len(match.group(1))
                input_indent = with_indent + 2
            continue

        indent = len(clean) - len(clean.lstrip(" "))
        if indent <= with_indent:
            break
        if indent != input_indent:
            continue
        match = re.match(rf"^\s{{{input_indent}}}([A-Za-z0-9_.-]+):\s*(.*)$", clean)
        if match is not None:
            items.append((match.group(1), match.group(2).strip()))
    return items


def block_has_input(block: list[str], name: str, value: str | None = None) -> bool:
    expected = None if value is None else unquote_yaml_scalar(value)
    for item_name, item_value in block_input_items(block):
        if item_name != name:
            continue
        if expected is None or unquote_yaml_scalar(item_value) == expected:
            return True
    return False


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


def shared_registry_cache_errors(job: str, job_lines: list[str]) -> list[str]:
    blocks = rust_cache_blocks(job_lines)
    shared_blocks = [
        block for block in blocks if block_has_input(block, "shared-key", SHARED_REGISTRY_CACHE_KEY)
    ]
    if not shared_blocks:
        return [f"{job} must use shared Cargo registry/git cache key"]

    errors: list[str] = []
    for block in blocks:
        if not block_has_input(block, "shared-key", SHARED_REGISTRY_CACHE_KEY):
            errors.append(f"{job} must use only shared Cargo registry/git rust-cache blocks")
        if not block_has_input(block, "cache-targets", "false"):
            errors.append(f"{job} shared Cargo registry/git cache must disable target caching")
        if not block_has_input(block, "cache-bin", "false"):
            errors.append(f"{job} shared Cargo registry/git cache must disable cargo bin caching")
        if not block_has_input(block, "save-if", SHARED_REGISTRY_SAVE_IF):
            errors.append(f"{job} shared Cargo registry/git cache save must be single-owner")
        if block_has_input(block, "cache-directories"):
            errors.append(f"{job} shared Cargo registry/git cache must not include target directories")
    return errors


def block_is_shared_registry_cache(block: list[str]) -> bool:
    return (
        block_has_input(block, "shared-key", SHARED_REGISTRY_CACHE_KEY)
        and block_has_input(block, "cache-targets", "false")
        and block_has_input(block, "cache-bin", "false")
        and not block_has_input(block, "cache-directories")
    )


def block_uses_managed_target_cache(block: list[str]) -> bool:
    return any("actions/cache@" in strip_comment(line) for line in block) and block_has_input(
        block, "path", "${{ steps.setup.outputs.managed_target_dir }}"
    )


def block_key_value_has_prefix(block: list[str], prefix: str) -> bool:
    for name, value in block_input_items(block):
        if name == "key" and prefix in value:
            return True
    return False


def block_declares_restore_keys_prefix(block: list[str], prefix: str) -> bool:
    # Locate the `with:` line to determine the input indent. The marker for
    # `restore-keys:` is anchored at that exact indent so earlier lines whose
    # values happen to contain the substring `restore-keys:` (e.g., a quoted
    # step-level `name:`) cannot impersonate the input.
    input_indent: int | None = None
    for line in block:
        match = re.match(r"^(\s*)with:\s*$", strip_comment(line).rstrip())
        if match is not None:
            input_indent = len(match.group(1)) + 2
            break
    if input_indent is None:
        return False
    marker_re = re.compile(rf"^\s{{{input_indent}}}restore-keys:\s*(.*)$")
    for marker_idx, line in enumerate(block):
        match = marker_re.match(strip_comment(line))
        if not match:
            continue
        value = match.group(1).strip()
        # Inline-scalar form: `restore-keys: managed-target-v1-...-clippy-host-`.
        # Anything not starting with a block-scalar indicator is treated as an
        # inline value and matched directly.
        if not value.startswith(("|", ">")):
            return prefix in value
        # Block-scalar form: `restore-keys: |` (plus YAML 1.2 chomping or
        # explicit-indentation indicators like `|2`, `>+1`, `|-3`). Body lines
        # are indented strictly more than the marker line; the scan stops at
        # the first line whose indent is equal-or-lesser.
        for child in block[marker_idx + 1:]:
            child_text = strip_comment(child)
            if not child_text.strip():
                continue
            child_indent = len(child) - len(child.lstrip(" "))
            if child_indent <= input_indent:
                break
            if prefix in child_text:
                return True
        return False
    return False


def managed_target_cache_errors(job: str, job_lines: list[str]) -> list[str]:
    expected_key = MANAGED_TARGET_CACHE_KEYS[job]
    target_blocks = [
        block
        for block in github_cache_blocks(job_lines)
        if block_has_input(block, "path", "${{ steps.setup.outputs.managed_target_dir }}")
    ]
    if not target_blocks:
        return [f"{job} must use isolated managed target cache"]

    expected_prefix = (
        f"managed-target-v1-${{{{ runner.os }}}}-${{{{ runner.arch }}}}-{expected_key}-"
    )
    # The exact `key:` value must carry the job-specific prefix. Checking the
    # whole block's text would also match a prefix that only appears in
    # `restore-keys:`, masking key/restore-keys drift.
    if not any(block_key_value_has_prefix(block, expected_prefix) for block in target_blocks):
        return [f"{job} managed target cache key must isolate {expected_key}"]

    # #400: each managed-target cache MUST declare a restore-keys prefix fallback
    # matching the job's key prefix. Without it, any change to CI orchestration
    # files included in hashFiles (justfile, ci/rust-verification.toml,
    # scripts/rust_verification.py) misses the exact key and pays the full
    # ~22m aarch64 release cross-compile instead of an incremental rebuild.
    if not any(
        block_declares_restore_keys_prefix(block, expected_prefix) for block in target_blocks
    ):
        return [
            f"{job} managed target cache must declare restore-keys prefix {expected_prefix}"
        ]
    return []


def job_just_lanes(job_lines: list[str]) -> set[str]:
    return {match.group(2) for match in JUST_LANE_RE.finditer(uncommented_text(job_lines))}


def block_uses_pinned_install_action(block: list[str]) -> bool:
    return any(TAIKI_INSTALL_ACTION_RE.match(strip_comment(line)) for line in block)


def install_action_tool_step(job_lines: list[str], tool: str, output: str) -> tuple[int, list[str]] | None:
    expected_tool = f"{tool}@${{{{ {output} }}}}"
    for index, block in enumerate(step_blocks(job_lines)):
        if block_uses_pinned_install_action(block) and block_has_input(block, "tool", expected_tool):
            return index, block
    return None


def first_step_running_command(job_lines: list[str], command: str) -> int | None:
    for index, block in enumerate(step_blocks(job_lines)):
        if block_runs_command(block, command):
            return index
    return None


def first_step_containing_literals(job_lines: list[str], literals: tuple[str, ...]) -> int | None:
    for index, block in enumerate(step_blocks(job_lines)):
        text = uncommented_text(block)
        if all(literal in text for literal in literals):
            return index
    return None


def first_step_containing_literals_in_order(job_lines: list[str], literals: tuple[str, ...]) -> int | None:
    for index, block in enumerate(step_blocks(job_lines)):
        text = uncommented_text(block)
        position = 0
        for literal in literals:
            found = text.find(literal, position)
            if found < 0:
                break
            position = found + len(literal)
        else:
            return index
    return None


def shell_assignment_word(token: str) -> bool:
    return re.match(r"^[A-Za-z_][A-Za-z0-9_]*=[\s\S]*$", token) is not None


SUDO_OPTIONS_WITH_ARGUMENT = {
    "-a",
    "-C",
    "-c",
    "-D",
    "-g",
    "-h",
    "-p",
    "-R",
    "-r",
    "-T",
    "-t",
    "-U",
    "-u",
    "--auth-type",
    "--chdir",
    "--close-from",
    "--command-timeout",
    "--group",
    "--host",
    "--login-class",
    "--prompt",
    "--role",
    "--type",
    "--user",
}
SUDO_OPTIONS_WITH_OPTIONAL_ARGUMENT = {
    "--preserve-env",
}
SUDO_OPTIONS_WITHOUT_ARGUMENT = {
    "-A",
    "-b",
    "-E",
    "-e",
    "-H",
    "-i",
    "-K",
    "-k",
    "-l",
    "-n",
    "-P",
    "-S",
    "-s",
    "-V",
    "-v",
    "--askpass",
    "--background",
    "--bell",
    "--edit",
    "--help",
    "--ignore-ticket",
    "--list",
    "--login",
    "--non-interactive",
    "--remove-timestamp",
    "--reset-timestamp",
    "--stdin",
    "--validate",
    "--version",
}
ENV_OPTIONS_WITH_ARGUMENT = {
    "-S",
    "-u",
    "-C",
    "--split-string",
    "--unset",
    "--chdir",
}
ENV_SIGNAL_OPTIONS = {"--block-signal", "--default-signal", "--ignore-signal"}
ENV_OPTIONS_WITHOUT_ARGUMENT = {
    "-0",
    "-i",
    "-v",
    "--debug",
    "--ignore-environment",
    "--null",
}
TIME_OPTIONS_WITH_ARGUMENT = {"-f", "-o", "--format", "--output"}
TIME_OPTIONS_WITHOUT_ARGUMENT = {"-a", "-p", "-v", "--append", "--portability", "--verbose"}
SHELL_PUNCTUATION_CHARS = ";&|(){}!"
SHELL_COMMAND_BOUNDARIES = {";", "&", "&&", "||", "|", "if", "elif", "then", "else", "while", "until", "do", "!", "(", "{", ")", "}"}
CARGO_PROCESS_SUBCOMMANDS = {
    "bench",
    "build",
    "check",
    "clean",
    "clippy",
    "doc",
    "fetch",
    "fmt",
    "install",
    "nextest",
    "run",
    "rustc",
    "test",
}


def consume_assignment_words(tokens: list[str], index: int) -> int:
    while index < len(tokens) and shell_assignment_word(tokens[index]):
        index += 1
    return index


def consume_option_prefix(
    tokens: list[str],
    index: int,
    options_with_argument: set[str],
    options_without_argument: set[str],
    options_with_optional_argument: set[str] | None = None,
) -> int | None:
    options_with_optional_argument = options_with_optional_argument or set()
    short_options_with_argument = {option for option in options_with_argument if re.match(r"^-[A-Za-z0-9]$", option)}
    short_options_without_argument = {option for option in options_without_argument if re.match(r"^-[A-Za-z0-9]$", option)}
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token in options_with_argument:
            if index + 1 >= len(tokens):
                return None
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in options_with_argument if option.startswith("--")):
            index += 1
            continue
        if any(token.startswith(f"{option}=") for option in options_with_optional_argument if option.startswith("--")):
            index += 1
            continue
        if token in options_with_optional_argument:
            index += 1
            continue
        if token in options_without_argument:
            index += 1
            continue
        if len(token) > 2 and token.startswith("-") and not token.startswith("--"):
            offset = 1
            while offset < len(token):
                option = f"-{token[offset]}"
                if option in short_options_without_argument:
                    offset += 1
                    continue
                if option in short_options_with_argument:
                    if offset + 1 < len(token):
                        index += 1
                    elif index + 1 < len(tokens):
                        index += 2
                    else:
                        return None
                    break
                return None
            else:
                index += 1
            continue
        break
    return index


def command_prefix_allows_cargo(prefix: list[str]) -> bool:
    index = consume_assignment_words(prefix, 0)
    while index < len(prefix):
        token = prefix[index]
        if token == "command":
            index += 1
        elif token == "time":
            index = consume_option_prefix(prefix, index + 1, TIME_OPTIONS_WITH_ARGUMENT, TIME_OPTIONS_WITHOUT_ARGUMENT)
        elif token == "nice":
            index = nice_command_index(prefix, index + 1)
        elif token == "sudo":
            index = consume_option_prefix(
                prefix,
                index + 1,
                SUDO_OPTIONS_WITH_ARGUMENT,
                SUDO_OPTIONS_WITHOUT_ARGUMENT,
                SUDO_OPTIONS_WITH_OPTIONAL_ARGUMENT,
            )
        elif token == "doas":
            index = consume_option_prefix(prefix, index + 1, SUDO_OPTIONS_WITH_ARGUMENT, SUDO_OPTIONS_WITHOUT_ARGUMENT)
        elif token == "env":
            index = env_command_prefix_index(prefix, index + 1)
        elif token == "flock":
            inner = flock_inner_tokens(prefix[index:])
            if inner is not None:
                index = len(prefix) - len(inner)
            else:
                return False
        elif token == "eval":
            index += 1
            if index < len(prefix) and prefix[index] == "--":
                index += 1
        elif token in {"catchsegv", "chrt", "exec", "ionice", "nohup", "setsid", "stdbuf", "taskset", "timeout", "xargs"}:
            inner = wrapper_inner_tokens(prefix[index:])
            if inner is None:
                return False
            index = len(prefix) - len(inner)
        else:
            return False
        if index is None:
            return False
        index = consume_assignment_words(prefix, index)
    return True


def cargo_token_is_command(tokens: list[str], index: int) -> bool:
    cursor = index - 1
    while cursor >= 0 and tokens[cursor] not in SHELL_COMMAND_BOUNDARIES:
        cursor -= 1
    prefix = tokens[cursor + 1 : index]
    return command_prefix_allows_cargo(prefix)


def split_shell_punctuation_tokens(tokens: list[str]) -> list[str]:
    split_tokens: list[str] = []
    for token in tokens:
        if not token or any(char not in SHELL_PUNCTUATION_CHARS for char in token):
            split_tokens.append(token)
            continue
        cursor = 0
        while cursor < len(token):
            if token[cursor : cursor + 2] in {"&&", "||"}:
                split_tokens.append(token[cursor : cursor + 2])
                cursor += 2
            else:
                split_tokens.append(token[cursor])
                cursor += 1
    return split_tokens


def command_tokens(command: str) -> list[str]:
    try:
        lexer = shlex.shlex(command, posix=True, punctuation_chars=SHELL_PUNCTUATION_CHARS)
        lexer.whitespace_split = True
        return split_shell_punctuation_tokens(list(lexer))
    except ValueError:
        return command.split()


def shell_quotes_are_balanced(text: str) -> bool:
    quote: str | None = None
    escaped = False
    for char in text:
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
    return quote is None


def shell_logical_lines(text: str) -> list[str]:
    lines: list[str] = []
    pending = ""
    for line in text.replace("\\\n", " ").splitlines():
        pending = f"{pending}\n{line}" if pending else line
        if shell_quotes_are_balanced(pending):
            lines.append(pending)
            pending = ""
    if pending:
        lines.append(pending)
    return lines


def shell_command(tokens: list[str]) -> str | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if index + 1 < len(tokens):
            if token == "-c":
                return tokens[index + 1]
            if token.startswith("-") and not token.startswith("--") and "c" in token[1:]:
                return tokens[index + 1]
        index += 1
    return None


def source_build_tool_from_token(token: str) -> str | None:
    token = token.rstrip("/")
    lower_token = token.lower()
    for tool in CI_SOURCE_BUILD_TOOLS:
        lower_tool = tool.lower()
        if lower_token == lower_tool or lower_token.startswith(f"{lower_tool}@"):
            return tool
        if lower_token.endswith(f"/{lower_tool}") or lower_token.endswith(f"/{lower_tool}.git"):
            return tool
    return None


def normalized_source_path(token: str) -> str:
    return token.rstrip("/")


def source_build_tool_for_path(
    token: str,
    source_path_tools: dict[str, str] | None,
    cwd_source_tool: str | None = None,
) -> str | None:
    normalized = normalized_source_path(token)
    if normalized == "." and cwd_source_tool is not None:
        return cwd_source_tool
    if source_path_tools and normalized in source_path_tools:
        return source_path_tools[normalized]
    return source_build_tool_from_token(token)


def executable_name(token: str) -> str:
    return pathlib.Path(token).name


def cargo_install_source_build_tools(
    tokens: list[str],
    command_index: int,
    source_path_tools: dict[str, str] | None = None,
    cwd_source_tool: str | None = None,
) -> set[str]:
    tools: set[str] = set()
    index = command_index + 1
    while index < len(tokens) and tokens[index] not in SHELL_COMMAND_BOUNDARIES:
        token = tokens[index]
        if token in ("--package", "-p") and index + 1 < len(tokens):
            tool = source_build_tool_for_path(tokens[index + 1], source_path_tools, cwd_source_tool)
            if tool is not None:
                tools.add(tool)
            index += 2
            continue
        if token.startswith("--package="):
            tool = source_build_tool_for_path(token.removeprefix("--package="), source_path_tools, cwd_source_tool)
            if tool is not None:
                tools.add(tool)
            index += 1
            continue
        if token == "--path" and index + 1 < len(tokens):
            tool = source_build_tool_for_path(tokens[index + 1], source_path_tools, cwd_source_tool)
            if tool is not None:
                tools.add(tool)
            index += 2
            continue
        if token.startswith("--path="):
            tool = source_build_tool_for_path(token.removeprefix("--path="), source_path_tools, cwd_source_tool)
            if tool is not None:
                tools.add(tool)
            index += 1
            continue
        tool = source_build_tool_for_path(token, source_path_tools, cwd_source_tool)
        if tool is not None:
            tools.add(tool)
        index += 1
    return tools


def source_build_tools_from_depth_exceeded_tokens(
    tokens: list[str],
    source_path_tools: dict[str, str] | None,
    cwd_source_tool: str | None,
) -> set[str]:
    if "install" not in tokens:
        return set()
    tools: set[str] = set()
    for token in tokens:
        tool = source_build_tool_for_path(token, source_path_tools, cwd_source_tool)
        if tool is not None:
            tools.add(tool)
    return tools


def cd_source_tool(tokens: list[str], source_path_tools: dict[str, str] | None) -> tuple[bool, str | None]:
    if not tokens or tokens[0] != "cd":
        return False, None
    index = 1
    while index < len(tokens) and tokens[index].startswith("-"):
        index += 1
    if index >= len(tokens):
        return True, None
    return True, source_build_tool_for_path(tokens[index], source_path_tools)


def cargo_install_source_build_tools_from_tokens(
    tokens: list[str],
    *,
    depth: int = 0,
    source_path_tools: dict[str, str] | None = None,
    cwd_source_tool: str | None = None,
) -> set[str]:
    if not tokens:
        return set()
    if depth > 6:
        return source_build_tools_from_depth_exceeded_tokens(tokens, source_path_tools, cwd_source_tool)
    tools: set[str] = set()
    if any(token in SHELL_COMMAND_BOUNDARIES for token in tokens):
        segment: list[str] = []
        segment_cwd_source_tool = cwd_source_tool
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                tools.update(
                    cargo_install_source_build_tools_from_tokens(
                        segment,
                        depth=depth + 1,
                        source_path_tools=source_path_tools,
                        cwd_source_tool=segment_cwd_source_tool,
                    )
                )
                changed, cd_tool = cd_source_tool(segment, source_path_tools)
                if changed:
                    segment_cwd_source_tool = cd_tool
                segment = []
                continue
            segment.append(token)
        tools.update(
            cargo_install_source_build_tools_from_tokens(
                segment,
                depth=depth + 1,
                source_path_tools=source_path_tools,
                cwd_source_tool=segment_cwd_source_tool,
            )
        )
        return tools
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index:
        return cargo_install_source_build_tools_from_tokens(
            tokens[assignment_index:],
            depth=depth + 1,
            source_path_tools=source_path_tools,
            cwd_source_tool=cwd_source_tool,
        )
    executable = pathlib.Path(tokens[0]).name
    if executable in ("bash", "dash", "fish", "sh", "zsh"):
        nested = shell_command(tokens)
        if nested is None:
            return tools
        return cargo_install_source_build_tools_from_tokens(
            command_tokens(nested),
            depth=depth + 1,
            source_path_tools=source_path_tools,
            cwd_source_tool=cwd_source_tool,
        )
    if executable in {
        "catchsegv",
        "chrt",
        "command",
        "doas",
        "exec",
        "ionice",
        "nice",
        "nohup",
        "setsid",
        "stdbuf",
        "sudo",
        "taskset",
        "time",
        "timeout",
        "xargs",
    }:
        inner = wrapper_inner_tokens(tokens)
        if inner is not None:
            return cargo_install_source_build_tools_from_tokens(
                inner,
                depth=depth + 1,
                source_path_tools=source_path_tools,
                cwd_source_tool=cwd_source_tool,
            )
        return tools
    if executable == "cargo":
        command_index = consume_cargo_global_options(tokens, 1)
        if command_index < len(tokens) and tokens[command_index] == "install":
            tools.update(cargo_install_source_build_tools(tokens, command_index, source_path_tools, cwd_source_tool))
    elif path_invocation_has_cargo_subcommand(tokens):
        command_index = consume_cargo_global_options(tokens, 1)
        if command_index < len(tokens) and tokens[command_index] == "install":
            tools.update(cargo_install_source_build_tools(tokens, command_index, source_path_tools, cwd_source_tool))
    return tools


def source_build_clone_path_tools(text: str) -> dict[str, str]:
    path_tools: dict[str, str] = {}
    for line in text.replace("\\\n", " ").splitlines():
        tokens = command_tokens(line)
        for index, token in enumerate(tokens[:-2]):
            if executable_name(token) != "git" or tokens[index + 1] != "clone":
                continue
            cursor = index + 2
            while cursor < len(tokens) and tokens[cursor].startswith("-"):
                if cursor + 1 < len(tokens) and not tokens[cursor + 1].startswith("-"):
                    cursor += 2
                else:
                    cursor += 1
            if cursor >= len(tokens):
                continue
            tool = source_build_tool_from_token(tokens[cursor])
            if tool is None:
                continue
            if cursor + 1 < len(tokens) and tokens[cursor + 1] not in SHELL_COMMAND_BOUNDARIES:
                path_tools[normalized_source_path(tokens[cursor + 1])] = tool
    return path_tools


def cargo_install_source_build_tools_in_text(text: str) -> set[str]:
    tools: set[str] = set()
    source_path_tools = source_build_clone_path_tools(text)
    cwd_source_tool: str | None = None
    for line in text.replace("\\\n", " ").splitlines():
        lexer = shlex.shlex(line, posix=True, punctuation_chars=True)
        lexer.whitespace_split = True
        try:
            tokens = list(lexer)
        except ValueError:
            continue
        if "install" in line:
            tools.update(
                cargo_install_source_build_tools_from_tokens(
                    tokens,
                    source_path_tools=source_path_tools,
                    cwd_source_tool=cwd_source_tool,
                )
            )
        for index, token in enumerate(tokens[:-1]):
            if executable_name(token) != "cargo":
                continue
            if not cargo_token_is_command(tokens, index):
                continue
            command_index = consume_cargo_global_options(tokens, index + 1)
            if command_index >= len(tokens) or tokens[command_index] != "install":
                continue
            tools.update(cargo_install_source_build_tools(tokens, command_index, source_path_tools, cwd_source_tool))
        changed, cd_tool = cd_source_tool(tokens, source_path_tools)
        if changed:
            cwd_source_tool = cd_tool
    return tools


def managed_rust_verification_tokens(tokens: list[str]) -> bool:
    return (
        len(tokens) >= 3
        and pathlib.Path(tokens[0]).name.startswith("python")
        and pathlib.Path(tokens[1]).name == "rust_verification.py"
        and tokens[2] in {"cargo", "run"}
    )


def tokens_have_target_routing_override(tokens: list[str]) -> bool:
    env_prefixes = (
        "BOLT_MANAGED_JUST=",
        "CARGO_BUILD_RUSTFLAGS=",
        "CARGO_BUILD_TARGET_DIR=",
        "CARGO_ENCODED_RUSTFLAGS=",
        "CARGO_HOME=",
        "CARGO_INCREMENTAL=",
        "CARGO_INSTALL_ROOT=",
        "CARGO_TARGET_DIR=",
        "CARGO_TARGET_TMPDIR=",
        "RUSTFLAGS=",
        "RUSTUP_HOME=",
    )
    value_options = {"--artifact-dir", "--out-dir", "--root", "--target-dir"}
    for index, token in enumerate(tokens):
        if token.startswith(env_prefixes):
            return True
        if token in value_options:
            return True
        if any(token.startswith(f"{option}=") for option in value_options):
            return True
        if token == "--config" and index + 1 < len(tokens) and cargo_config_has_storage_override(tokens[index + 1]):
            return True
        if token.startswith("--config=") and cargo_config_has_storage_override(token.split("=", 1)[1]):
            return True
    return False


def cargo_config_has_storage_override(config: str) -> bool:
    if cargo_config_looks_like_path(config):
        return True
    scan_config = decode_toml_unicode_escapes(config)
    if "target-dir" in scan_config and ("build" in scan_config or "[build]" in scan_config):
        return True
    return "rustflags" in scan_config and ("--out-dir" in scan_config or "--artifact-dir" in scan_config)


def decode_toml_unicode_escapes(value: str) -> str:
    def replace(match: re.Match[str]) -> str:
        digits = match.group(1) or match.group(2)
        return chr(int(digits, 16))

    return re.sub(r"\\u([0-9A-Fa-f]{4})|\\U([0-9A-Fa-f]{8})", lambda match: replace(match), value)


def cargo_config_looks_like_path(config: str) -> bool:
    stripped = config.strip()
    if not stripped:
        return False
    if stripped.startswith(("[", "{")):
        return False
    if "=" not in stripped:
        return True
    key_prefix = stripped.split("=", 1)[0]
    return "/" in key_prefix or "\\" in key_prefix or key_prefix.endswith(".toml")


def rustup_run_inner_tokens(tokens: list[str]) -> list[str]:
    index = 2
    while index < len(tokens) and tokens[index].startswith("-"):
        index += 1
    if index >= len(tokens):
        return []
    index += 1
    while index < len(tokens) and tokens[index] == "--":
        index += 1
    return tokens[index:]


def wrapper_inner_tokens(tokens: list[str]) -> list[str] | None:
    executable = pathlib.Path(tokens[0]).name if tokens else ""
    if executable == "command":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token == "-p":
                index += 1
                continue
            if token in ("-v", "-V"):
                return []
            return tokens[index:]
        return []
    if executable in {"sudo", "doas"}:
        index = consume_option_prefix(
            tokens,
            1,
            SUDO_OPTIONS_WITH_ARGUMENT,
            SUDO_OPTIONS_WITHOUT_ARGUMENT,
            SUDO_OPTIONS_WITH_OPTIONAL_ARGUMENT if executable == "sudo" else None,
        )
        return tokens[index:] if index is not None else None
    if executable == "timeout":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in ("-k", "--kill-after", "-s", "--signal") and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--kill-after=", "--signal=")):
                index += 1
                continue
            if token.startswith("-"):
                index += 1
                continue
            return tokens[index + 1 :]
        return []
    if executable == "stdbuf":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in ("-i", "-o", "-e", "--input", "--output", "--error") and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--input=", "--output=", "--error=")):
                index += 1
                continue
            if re.fullmatch(r"-[ioe].+", token):
                index += 1
                continue
            return tokens[index:]
        return []
    if executable in {"catchsegv", "exec", "nohup"}:
        return tokens[1:]
    if executable == "time":
        index = consume_option_prefix(tokens, 1, TIME_OPTIONS_WITH_ARGUMENT, TIME_OPTIONS_WITHOUT_ARGUMENT)
        return tokens[index:] if index is not None else None
    if executable == "setsid":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in ("-c", "--ctty", "-f", "--fork", "-w", "--wait"):
                index += 1
                continue
            if token.startswith("-") and not token.startswith("--") and set(token[1:]) <= {"c", "f", "w"}:
                index += 1
                continue
            return tokens[index:]
        return []
    if executable == "taskset":
        index = 1
        cpu_list_mode = False
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in ("-c", "--cpu-list") and index + 1 < len(tokens):
                index += 2
                cpu_list_mode = True
                continue
            if token.startswith("--cpu-list=") or re.fullmatch(r"-c.+", token):
                index += 1
                cpu_list_mode = True
                continue
            if token in ("-a", "--all-tasks"):
                index += 1
                continue
            if token in ("-p", "--pid"):
                return []
            if token.startswith("-"):
                index += 1
                continue
            if not cpu_list_mode:
                index += 1
            return tokens[index:]
        return []
    if executable == "ionice":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in ("-c", "--class", "-n", "--classdata") and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--class=", "--classdata=")) or re.fullmatch(r"-[cn].+", token):
                index += 1
                continue
            if token in ("-p", "--pid"):
                return []
            if token in ("-t", "--ignore"):
                index += 1
                continue
            if token.startswith("-") and not token.startswith("--"):
                cluster = token[1:]
                if cluster and (set(cluster) <= {"t"} or re.fullmatch(r"t*[cn].+", cluster)):
                    index += 1
                    continue
            return tokens[index:]
        return []
    if executable == "chrt":
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                index += 1
                break
            if token in ("-p", "--pid"):
                return []
            if token in ("-T", "--sched-runtime", "-P", "--sched-period", "-D", "--sched-deadline") and index + 1 < len(tokens):
                index += 2
                continue
            if token.startswith(("--sched-runtime=", "--sched-period=", "--sched-deadline=")):
                index += 1
                continue
            if token.startswith("-"):
                index += 1
                continue
            break
        if index < len(tokens):
            index += 1
        return tokens[index:]
    if executable == "xargs":
        options_with_argument = {
            "-a",
            "--arg-file",
            "-d",
            "--delimiter",
            "-E",
            "-I",
            "-L",
            "-n",
            "--max-args",
            "-P",
            "--max-procs",
            "-s",
            "--max-chars",
        }
        index = 1
        while index < len(tokens):
            token = tokens[index]
            if token == "--":
                return tokens[index + 1 :]
            if token in options_with_argument and index + 1 < len(tokens):
                index += 2
                continue
            if any(token.startswith(f"{option}=") for option in options_with_argument if option.startswith("--")):
                index += 1
                continue
            if re.fullmatch(r"-(?:a|d|E|I|L|n|P|s).+", token):
                index += 1
                continue
            if token.startswith("-"):
                index += 1
                continue
            return tokens[index:]
        return []
    starters = {
        "bash",
        "catchsegv",
        "cargo",
        "cargo-clippy",
        "cargo-fmt",
        "cargo-nextest",
        "env",
        "flock",
        "nice",
        "python",
        "python3",
        "rustup",
        "sh",
        "stdbuf",
        "time",
        "zsh",
    }
    for index, token in enumerate(tokens[1:], start=1):
        if pathlib.Path(token).name in starters:
            return tokens[index:]
    return None


def env_short_cluster_next_index(tokens: list[str], index: int, cluster: str) -> int | None:
    offset = 0
    while offset < len(cluster):
        option = cluster[offset]
        if option in "i0v":
            offset += 1
            continue
        if option in "uC":
            if offset + 1 < len(cluster):
                return index + 1
            if index + 1 < len(tokens):
                return index + 2
            return index + 1
        return None
    return index + 1


def env_short_split_tokens(tokens: list[str], index: int) -> list[str] | None:
    token = tokens[index]
    if not token.startswith("-") or token.startswith("--"):
        return None
    cluster = token[1:]
    if "S" not in cluster:
        return None
    suffix = cluster.split("S", 1)[1]
    if suffix:
        return command_tokens(" ".join([suffix, *tokens[index + 1 :]]))
    if index + 1 < len(tokens):
        return command_tokens(tokens[index + 1]) + tokens[index + 2 :]
    return []


def env_command_prefix_index(tokens: list[str], index: int) -> int | None:
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return index + 1
        if token in ENV_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        if token in ENV_SIGNAL_OPTIONS:
            index += 1
            continue
        if any(token.startswith(f"{option}=") for option in ENV_SIGNAL_OPTIONS):
            index += 1
            continue
        if token in ENV_OPTIONS_WITH_ARGUMENT:
            if index + 1 >= len(tokens):
                return None
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in ENV_OPTIONS_WITH_ARGUMENT if option.startswith("--")):
            index += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            if "S" in token[1:]:
                return index
            parsed_index = env_short_cluster_next_index(tokens, index, token[1:])
            if parsed_index is not None:
                index = parsed_index
                continue
        if shell_assignment_word(token):
            index += 1
            continue
        return index
    return index


def env_inner_tokens(tokens: list[str]) -> list[str] | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return tokens[index + 1 :]
        if token in ENV_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        if token in ENV_SIGNAL_OPTIONS:
            index += 1
            continue
        if any(token.startswith(f"{option}=") for option in ENV_SIGNAL_OPTIONS):
            index += 1
            continue
        if token in ("-S", "--split-string") and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1]) + tokens[index + 2 :]
        if token.startswith("--split-string="):
            return command_tokens(token.split("=", 1)[1]) + tokens[index + 1 :]
        if token in ENV_OPTIONS_WITH_ARGUMENT and index + 1 < len(tokens):
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in ENV_OPTIONS_WITH_ARGUMENT if option.startswith("--")):
            index += 1
            continue
        if token.startswith("-") and not token.startswith("--"):
            split_tokens = env_short_split_tokens(tokens, index)
            if split_tokens is not None:
                return split_tokens
            parsed_index = env_short_cluster_next_index(tokens, index, token[1:])
            if parsed_index is not None:
                index = parsed_index
                continue
        if shell_assignment_word(token):
            index += 1
            continue
        return tokens[index:]
    return []


def nice_command_index(tokens: list[str], index: int) -> int | None:
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            continue
        if token == "-n" and index + 1 < len(tokens):
            index += 2
            continue
        if token == "--adjustment" and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith("--adjustment="):
            index += 1
            continue
        if re.fullmatch(r"-n-?\d+", token) or re.fullmatch(r"-?\d+", token):
            index += 1
            continue
        return index
    return index


def flock_inner_tokens(tokens: list[str]) -> list[str] | None:
    command_option_tokens = flock_command_option_tokens(tokens)
    if command_option_tokens is not None:
        return command_option_tokens
    index = 1
    separator_seen = False
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            separator_seen = True
            break
        if token in ("-c", "--command") and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1])
        if token.startswith("--command="):
            return command_tokens(token.split("=", 1)[1])
        if token in ("-E", "--conflict-exit-code", "-w", "--wait", "--timeout") and index + 1 < len(tokens):
            index += 2
            continue
        if token.startswith(("--conflict-exit-code=", "--wait=", "--timeout=")):
            index += 1
            continue
        if token.startswith("-"):
            index += 1
            continue
        return tokens[index + 1 :]
    if separator_seen and index < len(tokens):
        return tokens[index + 1 :]
    return tokens[index:]


def flock_command_option_tokens(tokens: list[str]) -> list[str] | None:
    index = 1
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            return None
        if token in ("-c", "--command") and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1])
        if token.startswith("--command="):
            return command_tokens(token.split("=", 1)[1])
        if token.startswith("-") and not token.startswith("--") and "c" in token[1:] and index + 1 < len(tokens):
            return command_tokens(tokens[index + 1])
        index += 1
    return None


def simple_cargo_aliases(tokens: list[str], known_aliases: set[str] | None = None) -> set[str]:
    known_aliases = known_aliases or set()
    aliases: set[str] = set()
    for token in tokens[1:]:
        name, separator, value = token.partition("=")
        name = name.strip("\"'")
        value_tokens = command_tokens(value) if separator else []
        value_names = {pathlib.Path(value_token).name for value_token in value_tokens}
        if separator and re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", name) and any(
            raw_rust_tool_token(value_name) or value_name in known_aliases for value_name in value_names
        ):
            aliases.add(name)
    return aliases


def expand_cargo_aliases(tokens: list[str], aliases: set[str]) -> list[str]:
    if not aliases:
        return tokens
    return ["cargo" if token in aliases else token for token in tokens]


def no_mistakes_inner_tokens(tokens: list[str]) -> list[str] | None:
    for index, token in enumerate(tokens):
        if token == "--":
            return tokens[index + 1 :]
    return None


def raw_rust_tool_token(name: str) -> bool:
    return name in {"cargo", "clippy", "nextest", "rustc", "rustdoc"} or name.startswith(
        ("cargo-", "clippy-", "rust-")
    )


def path_name_looks_like_renamed_cargo(name: str) -> bool:
    return name == "c" or name.endswith("cargo") or raw_rust_tool_token(name)


def path_executable_looks_like_cargo(token: str) -> bool:
    if "/" not in token:
        return False
    path = pathlib.Path(token)
    if path_name_looks_like_renamed_cargo(path.name):
        return True
    try:
        resolved = path.expanduser().resolve(strict=True)
    except (OSError, RuntimeError):
        return False
    return path_name_looks_like_renamed_cargo(resolved.name)


def path_invocation_has_cargo_subcommand(tokens: list[str]) -> bool:
    if not tokens or not path_executable_looks_like_cargo(tokens[0]):
        return False
    command_index = consume_cargo_global_options(tokens, 1)
    return command_index < len(tokens) and tokens[command_index] in CARGO_PROCESS_SUBCOMMANDS


def shell_assignment_values_from_tokens(tokens: list[str]) -> tuple[dict[str, str], int]:
    assignments: dict[str, str] = {}
    cursor = 0
    while cursor < len(tokens):
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is None:
            break
        name, value, cursor = assignment
        assignments[name] = storage_strip_quotes(value)
    return assignments, cursor


def export_assignment_values_from_tokens(tokens: list[str]) -> tuple[dict[str, str], int]:
    if not tokens or pathlib.Path(tokens[0]).name != "export":
        return {}, 0
    assignments: dict[str, str] = {}
    cursor = 1
    while cursor < len(tokens):
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is None:
            break
        name, value, cursor = assignment
        assignments[name] = storage_strip_quotes(value)
    return assignments, cursor


def persistent_shell_assignment_values(tokens: list[str]) -> tuple[dict[str, str], bool]:
    assignments, assignment_index = shell_assignment_values_from_tokens(tokens)
    if assignments and assignment_index == len(tokens):
        return assignments, True
    assignments, assignment_index = export_assignment_values_from_tokens(tokens)
    if assignments and assignment_index == len(tokens):
        return assignments, True
    return {}, False


def shell_variable_reference_token(token: str) -> str | None:
    clean = storage_strip_quotes(token)
    match = re.fullmatch(r"\$([A-Za-z_][A-Za-z0-9_]*)", clean)
    if match:
        return match.group(1)
    match = re.fullmatch(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}", clean)
    if match:
        return match.group(1)
    match = re.fullmatch(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?::?[-?+=].*)\}", clean)
    if match:
        return match.group(1)
    return None


def expand_known_shell_variables(tokens: list[str], variables: dict[str, str]) -> list[str]:
    expanded: list[str] = []
    for token in tokens:
        variable = shell_variable_reference_token(token)
        if variable is not None and variable in variables:
            expanded.extend(command_tokens(variables[variable]))
        else:
            expanded.append(token)
    return expanded


def expand_known_shell_assignment_names(tokens: list[str], variables: dict[str, str]) -> list[str]:
    expanded: list[str] = []
    for token in tokens:
        if "=" not in token:
            expanded.append(token)
            continue
        name, value = token.split("=", 1)
        variable = shell_variable_reference_token(name)
        if (
            variable is not None
            and variable in variables
            and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", variables[variable])
        ):
            expanded.append(f"{variables[variable]}={value}")
            continue
        expanded.append(token)
    return expanded


def expand_known_shell_command_variables(tokens: list[str], variables: dict[str, str]) -> list[str]:
    if not tokens:
        return tokens
    executable = pathlib.Path(tokens[0]).name
    if executable == "eval":
        return [tokens[0], *expand_known_shell_variables(tokens[1:], variables)]
    if executable in ("bash", "dash", "fish", "sh", "zsh"):
        expanded = list(tokens)
        index = 1
        while index + 1 < len(expanded):
            token = expanded[index]
            if token == "-c" or (token.startswith("-") and not token.startswith("--") and "c" in token[1:]):
                variable = shell_variable_reference_token(expanded[index + 1])
                if variable is not None and variable in variables:
                    expanded[index + 1] = variables[variable]
                return expanded
            index += 1
        return expanded
    variable = shell_variable_reference_token(tokens[0])
    if variable is not None and variable in variables:
        return [*command_tokens(variables[variable]), *tokens[1:]]
    return tokens


def tokens_have_raw_cargo(tokens: list[str], *, depth: int = 0, allow_storage_only: bool = True) -> bool:
    if not tokens:
        return False
    if depth > 6:
        return True
    if allow_storage_only and tokens_have_target_routing_override(tokens):
        return True
    if any(token in SHELL_COMMAND_BOUNDARIES for token in tokens):
        segment: list[str] = []
        cargo_aliases: set[str] = set()
        shell_variables: dict[str, str] = {}
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                if segment and segment[0] == "alias":
                    cargo_aliases.update(simple_cargo_aliases(segment, cargo_aliases))
                    segment = []
                    continue
                shell_assignments, is_persistent_assignment = persistent_shell_assignment_values(segment)
                if is_persistent_assignment:
                    shell_variables.update(shell_assignments)
                    segment = []
                    continue
                segment = expand_known_shell_assignment_names(segment, shell_variables)
                segment = expand_known_shell_command_variables(segment, shell_variables)
                segment = expand_cargo_aliases(segment, cargo_aliases)
                if segment and tokens_have_raw_cargo(segment, depth=depth + 1, allow_storage_only=allow_storage_only):
                    return True
                segment = []
                continue
            segment.append(token)
        if segment and segment[0] == "alias":
            return False
        shell_assignments, is_persistent_assignment = persistent_shell_assignment_values(segment)
        if is_persistent_assignment:
            return False
        segment = expand_known_shell_assignment_names(segment, shell_variables)
        segment = expand_known_shell_command_variables(segment, shell_variables)
        segment = expand_cargo_aliases(segment, cargo_aliases)
        return bool(segment) and tokens_have_raw_cargo(segment, depth=depth + 1, allow_storage_only=allow_storage_only)
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index:
        return assignment_index < len(tokens) and tokens_have_raw_cargo(
            tokens[assignment_index:],
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
        )
    if managed_rust_verification_tokens(tokens):
        return tokens_have_target_routing_override(tokens[3:])
    executable = pathlib.Path(tokens[0]).name
    if path_invocation_has_cargo_subcommand(tokens):
        return True
    if executable in ("bash", "dash", "fish", "sh", "zsh"):
        nested = shell_command(tokens)
        return nested is not None and tokens_have_raw_cargo(
            command_tokens(nested),
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
        )
    if executable == "eval":
        inner = tokens[1:]
        if inner and inner[0] == "--":
            inner = inner[1:]
        return bool(inner) and tokens_have_raw_cargo(
            command_tokens(" ".join(inner)),
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
        )
    if executable == "no-mistakes":
        inner = no_mistakes_inner_tokens(tokens)
        if inner is None:
            return False
        if inner and raw_rust_tool_token(pathlib.Path(inner[0]).name):
            return True
        return tokens_have_raw_cargo(inner, depth=depth + 1, allow_storage_only=allow_storage_only)
    if executable == "env":
        inner = env_inner_tokens(tokens)
        return inner is not None and tokens_have_raw_cargo(inner, depth=depth + 1, allow_storage_only=allow_storage_only)
    if executable == "rustup" and len(tokens) >= 3 and tokens[1] == "run":
        return tokens_have_raw_cargo(
            rustup_run_inner_tokens(tokens),
            depth=depth + 1,
            allow_storage_only=allow_storage_only,
        )
    if executable.startswith("python"):
        for index, token in enumerate(tokens):
            if token == "-c" and index + 1 < len(tokens) and "cargo" in tokens[index + 1]:
                return True
    if executable == "flock":
        inner = flock_inner_tokens(tokens)
        if inner is not None:
            return tokens_have_raw_cargo(inner, depth=depth + 1, allow_storage_only=allow_storage_only)
    if executable in {
        "catchsegv",
        "chrt",
        "command",
        "doas",
        "exec",
        "ionice",
        "nice",
        "nohup",
        "setsid",
        "stdbuf",
        "sudo",
        "taskset",
        "time",
        "timeout",
        "xargs",
    }:
        inner = wrapper_inner_tokens(tokens)
        if inner is not None:
            return tokens_have_raw_cargo(inner, depth=depth + 1, allow_storage_only=allow_storage_only)
    for index, token in enumerate(tokens):
        name = pathlib.Path(token).name
        if name == "cargo" and cargo_token_is_command(tokens, index):
            return True
        if name in {"clippy", "nextest", "rustc", "rustdoc"} and command_prefix_allows_cargo(tokens[:index]):
            return True
        if name.startswith("cargo-") and command_prefix_allows_cargo(tokens[:index]):
            return True
    return False


def command_has_raw_cargo(command: str) -> bool:
    return tokens_have_raw_cargo(command_tokens(command))


def tokens_have_raw_cargo_launch(tokens: list[str]) -> bool:
    return tokens_have_raw_cargo(tokens, allow_storage_only=False)


def tokens_are_rust_version_probe(tokens: list[str]) -> bool:
    if not tokens:
        return False
    assignment_index = consume_assignment_words(tokens, 0)
    if assignment_index:
        return tokens_are_rust_version_probe(tokens[assignment_index:])
    executable = pathlib.Path(tokens[0]).name
    if executable == "cargo":
        command_index = consume_cargo_global_options(tokens, 1)
        probe_commands = {"--version", "-V", "version", "--help", "-h", "help"}
        return command_index < len(tokens) and tokens[command_index] in probe_commands
    if raw_rust_tool_token(executable):
        return any(token in {"--version", "-V", "--help", "-h"} for token in tokens[1:])
    return False


def tokens_have_repo_automation_raw_cargo(tokens: list[str]) -> bool:
    if not tokens:
        return False
    if any(token in SHELL_COMMAND_BOUNDARIES for token in tokens):
        segment: list[str] = []
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                if tokens_have_repo_automation_raw_cargo(segment):
                    return True
                segment = []
                continue
            segment.append(token)
        return tokens_have_repo_automation_raw_cargo(segment)
    if tokens_are_rust_version_probe(tokens):
        return False
    return tokens_have_raw_cargo_launch(tokens)


def repo_automation_raw_cargo_errors(file_name: str, text: str) -> list[str]:
    errors: list[str] = []
    managed_just_recipe = False
    current_just_recipe = ""
    shell_variables: dict[str, str] = {}
    is_justfile = file_name == "justfile" or file_name.startswith("justfile.")
    for line in shell_logical_lines(text):
        stripped = strip_comment(line).strip()
        if not stripped:
            continue
        if is_justfile and not line[:1].isspace():
            if stripped.startswith("["):
                continue
            if ":" in stripped:
                recipe = stripped.split(":", 1)[0].strip()
                current_just_recipe = recipe.split()[0] if recipe else ""
                managed_just_recipe = False
        if (
            is_justfile
            and current_just_recipe in {"managed-build", "managed-clippy", "managed-test"}
            and "BOLT_MANAGED_JUST" in stripped
            and "rust_verification.py run" in stripped
            and "exit 2" in stripped
        ):
            managed_just_recipe = True
            continue
        if is_justfile and managed_just_recipe:
            continue
        tokens = command_tokens(stripped)
        assignments, is_persistent_assignment = persistent_shell_assignment_values(tokens)
        if is_persistent_assignment:
            shell_variables.update(assignments)
            continue
        tokens = expand_known_shell_assignment_names(tokens, shell_variables)
        tokens = expand_known_shell_command_variables(tokens, shell_variables)
        if tokens_have_repo_automation_raw_cargo(tokens):
            errors.append("repo automation raw Cargo must use managed rust_verification wrapper")
            break
    return errors


def cargo_config_storage_override_message(tokens: list[str]) -> str | None:
    for index, token in enumerate(tokens):
        if token == "--config" and index + 1 < len(tokens) and cargo_config_has_storage_override(tokens[index + 1]):
            if cargo_config_looks_like_path(tokens[index + 1]):
                return "cargo --config file raw target override must be classified"
            return "cargo --config build.target-dir raw target override must be classified"
        if token.startswith("--config="):
            config = token.split("=", 1)[1]
            if cargo_config_has_storage_override(config):
                if cargo_config_looks_like_path(config):
                    return "cargo --config file raw target override must be classified"
                return "cargo --config build.target-dir raw target override must be classified"
    return None


def direct_raw_cargo_storage_override_messages(tokens: list[str]) -> set[str]:
    messages: set[str] = set()
    if any(token == "--target-dir" or token.startswith("--target-dir=") for token in tokens):
        messages.add("cargo --target-dir raw target override must be classified")
    config_message = cargo_config_storage_override_message(tokens)
    if config_message is not None:
        messages.add(config_message)
    if any(token == "install" for token in tokens):
        if any(token == "--root" and index + 1 < len(tokens) and tokens[index + 1].startswith("s3://") for index, token in enumerate(tokens)):
            messages.add("cargo install S3 install root must be classified")
        if any(token.startswith("--root=s3://") for token in tokens):
            messages.add("cargo install S3 install root must be classified")
    return messages


def raw_cargo_storage_override_messages_from_tokens(
    tokens: list[str],
    *,
    aliases: set[str] | None = None,
    depth: int = 0,
) -> set[str]:
    if not tokens:
        return set()
    aliases = aliases or set()
    expanded = expand_cargo_aliases(tokens, aliases)
    if depth > 6:
        if tokens_have_raw_cargo_launch(expanded):
            return direct_raw_cargo_storage_override_messages(expanded)
        return set()
    messages: set[str] = set()
    if any(token in SHELL_COMMAND_BOUNDARIES for token in tokens):
        segment: list[str] = []
        segment_aliases = set(aliases)
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                messages.update(
                    raw_cargo_storage_override_messages_from_tokens(
                        segment,
                        aliases=segment_aliases,
                        depth=depth + 1,
                    )
                )
                if segment and segment[0] == "alias":
                    segment_aliases.update(simple_cargo_aliases(segment, segment_aliases))
                segment = []
                continue
            segment.append(token)
        messages.update(
            raw_cargo_storage_override_messages_from_tokens(
                segment,
                aliases=segment_aliases,
                depth=depth + 1,
            )
        )
        return messages
    if tokens and tokens[0] == "alias":
        return messages
    assignment_index = consume_assignment_words(expanded, 0)
    if assignment_index:
        return raw_cargo_storage_override_messages_from_tokens(
            expanded[assignment_index:],
            aliases=aliases,
            depth=depth + 1,
        )
    executable = pathlib.Path(expanded[0]).name
    if executable in ("bash", "dash", "fish", "sh", "zsh"):
        nested = shell_command(expanded)
        if nested is not None:
            messages.update(
                raw_cargo_storage_override_messages_from_tokens(
                    command_tokens(nested),
                    aliases=aliases,
                    depth=depth + 1,
                )
            )
        return messages
    if executable in {
        "catchsegv",
        "chrt",
        "command",
        "doas",
        "exec",
        "ionice",
        "nice",
        "nohup",
        "setsid",
        "stdbuf",
        "sudo",
        "taskset",
        "time",
        "timeout",
        "xargs",
    }:
        inner = wrapper_inner_tokens(expanded)
        if inner is not None:
            messages.update(
                raw_cargo_storage_override_messages_from_tokens(
                    inner,
                    aliases=aliases,
                    depth=depth + 1,
                )
            )
        return messages
    if not tokens_have_raw_cargo_launch(expanded):
        return messages
    messages.update(direct_raw_cargo_storage_override_messages(expanded))
    return messages


def text_raw_cargo_storage_override_messages(text: str) -> set[str]:
    messages: set[str] = set()
    aliases: set[str] = set()
    for line in text.splitlines():
        if not line.strip():
            continue
        tokens = command_tokens(line)
        messages.update(raw_cargo_storage_override_messages_from_tokens(tokens, aliases=aliases))
        segment: list[str] = []
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                if segment and segment[0] == "alias":
                    aliases.update(simple_cargo_aliases(segment, aliases))
                segment = []
                continue
            segment.append(token)
        if segment and segment[0] == "alias":
            aliases.update(simple_cargo_aliases(segment, aliases))
    return messages


def strip_yaml_anchor(value: str) -> tuple[str | None, str]:
    match = re.match(r"&([A-Za-z0-9_.-]+)(?:\s+|$)(.*)", value)
    if match is None:
        return None, value
    return match.group(1), match.group(2).strip()


def resolve_no_mistakes_scalar(value: str, anchors: dict[str, str]) -> tuple[str, str | None]:
    value = value.strip()
    alias = re.fullmatch(r"\*([A-Za-z0-9_.-]+)", value)
    if alias is not None:
        return anchors.get(alias.group(1), value), None
    anchor, value = strip_yaml_anchor(value)
    if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
        value = value[1:-1]
    return value, anchor


def no_mistakes_commands(config_text: str) -> dict[str, str]:
    commands: dict[str, str] = {}
    anchors: dict[str, str] = {}
    in_commands = False
    lines = config_text.splitlines()
    index = 0
    while index < len(lines):
        raw_line = lines[index]
        line = raw_line.rstrip()
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            index += 1
            continue
        indent = len(line) - len(line.lstrip(" "))
        if indent == 0:
            name, separator, value = stripped.partition(":")
            in_commands = bool(separator) and name.strip() == "commands" and (
                not value.strip() or value.strip().startswith("#")
            )
            index += 1
            continue
        if not in_commands:
            index += 1
            continue
        if indent <= 2 and ":" in stripped:
            name, _, value = stripped.partition(":")
            value = value.strip()
            anchor, stripped_value = strip_yaml_anchor(value)
            if anchor is not None:
                value = stripped_value
            if value in ("|", ">") or value.startswith(("|", ">")):
                block_lines: list[str] = []
                index += 1
                while index < len(lines):
                    candidate = lines[index].rstrip()
                    candidate_stripped = candidate.strip()
                    if not candidate_stripped or candidate_stripped.startswith("#"):
                        index += 1
                        continue
                    candidate_indent = len(candidate) - len(candidate.lstrip(" "))
                    if candidate_indent <= indent:
                        break
                    block_lines.append(candidate_stripped)
                    index += 1
                command = "\n".join(block_lines).strip()
                commands[name.strip()] = command
                if anchor is not None:
                    anchors[anchor] = command
                continue
            scalar_parts = [value]
            index += 1
            while index < len(lines):
                candidate = lines[index].rstrip()
                candidate_stripped = candidate.strip()
                if not candidate_stripped or candidate_stripped.startswith("#"):
                    index += 1
                    continue
                candidate_indent = len(candidate) - len(candidate.lstrip(" "))
                if candidate_indent <= indent:
                    break
                scalar_parts.append(candidate_stripped)
                index += 1
            value = " ".join(part for part in scalar_parts if part).strip()
            value, scalar_anchor = resolve_no_mistakes_scalar(value if anchor is None else f"&{anchor} {value}", anchors)
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
        if not stripped or stripped.startswith("#"):
            continue
        indent = len(line) - len(line.lstrip(" "))
        if indent != 0:
            continue
        name, separator, value = stripped.partition(":")
        if not separator or name.strip() != "commands":
            continue
        value = value.strip()
        if value and not value.startswith("#"):
            errors.append(f"{config_name} commands section must use block mapping")
    return errors


def verify_no_mistakes_config(config_text: str, config_name: str = ".no-mistakes.yaml") -> list[str]:
    errors: list[str] = no_mistakes_command_section_errors(config_text, config_name)
    for command_name, command in no_mistakes_commands(config_text).items():
        command_segments = [command, *command.splitlines()]
        if any(command_has_raw_cargo(segment) for segment in command_segments if segment.strip()):
            errors.append(f"{config_name} commands.{command_name} raw Cargo drift must be classified")
    return errors


def exact_head_governance_cache_errors(workflow_text: str) -> list[str]:
    for line in workflow_text.splitlines():
        clean = strip_comment(line)
        if "hashFiles(" not in clean:
            continue
        if "managed-target-v1-" not in clean and "nextest-archive-v1-" not in clean:
            continue
        if any(cache_input not in clean for cache_input in EXACT_HEAD_GOVERNANCE_CACHE_INPUTS):
            return ["cache keys must include exact-head CI/no-mistakes governance inputs"]
    return []


def text_has_path_style_cargo_config(text: str) -> bool:
    for match in re.finditer(r"\bcargo\b[^\n;&|]*", text):
        tokens = command_tokens(match.group(0))
        for index, token in enumerate(tokens):
            if pathlib.Path(token).name != "cargo":
                continue
            cursor = index + 1
            while cursor < len(tokens) and tokens[cursor] not in SHELL_COMMAND_BOUNDARIES:
                option = tokens[cursor]
                if option == "--config" and cursor + 1 < len(tokens):
                    if cargo_config_looks_like_path(tokens[cursor + 1]):
                        return True
                    cursor += 2
                    continue
                if option.startswith("--config=") and cargo_config_looks_like_path(option.split("=", 1)[1]):
                    return True
                cursor += 1
    return False


STORAGE_ROLE_S3 = "s3"
STORAGE_ROLE_ACTIVE_TARGET = "active_target"
AWS_S3_TRANSFER_COMMANDS = {"cp", "mv", "sync"}
AWS_S3_OPTIONS_WITH_ARGUMENT = {
    "--acl",
    "--cache-control",
    "--content-disposition",
    "--content-encoding",
    "--content-language",
    "--content-type",
    "--copy-props",
    "--exclude",
    "--expires",
    "--expected-size",
    "--include",
    "--metadata",
    "--metadata-directive",
    "--page-size",
    "--profile",
    "--region",
    "--request-payer",
    "--sse",
    "--sse-c",
    "--sse-c-copy-source",
    "--sse-c-copy-source-key",
    "--sse-c-key",
    "--sse-kms-key-id",
    "--storage-class",
    "--website-redirect",
}


def storage_strip_quotes(value: str) -> str:
    return value.strip().strip("\"'")


def storage_without_trailing_current_dir(value: str) -> str:
    normalized = storage_strip_quotes(value).replace('"', "").replace("'", "")
    while normalized.endswith("/.") or normalized.endswith("/"):
        normalized = normalized[:-2] if normalized.endswith("/.") else normalized[:-1]
    return normalized


def storage_variable_names(value: str) -> set[str]:
    names = {match.group(1) for match in re.finditer(r"\$([A-Za-z_][A-Za-z0-9_]*)\b", value)}
    names.update(match.group(1) for match in re.finditer(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?:[^}]*)\}", value))
    names.update(match.group(1) for match in re.finditer(r"\$\{\{\s*env\.([A-Za-z_][A-Za-z0-9_]*)\s*\}\}", value))
    return names


def storage_command_substitution_has_target(value: str) -> bool:
    compact = storage_strip_quotes(value).replace('"', "").replace("'", "")
    for match in re.finditer(r"`([^`]*)`", compact):
        if re.search(r"(?:^|[\s/])target(?:[\s/]|$)", match.group(1)):
            return True
    return (
        ("$(" in compact or compact.startswith("$ ("))
        and re.search(r"(?:^|[\s/])target(?:[\s/]|$)", compact) is not None
    )


def storage_value_has_target_component(value: str) -> bool:
    normalized = storage_strip_quotes(value).replace('"', "").replace("'", "")
    if not normalized or normalized.startswith("s3://"):
        return False
    parts = [part for part in re.split(r"[\\/]+", normalized) if part and part not in {".", ".."}]
    return "target" in parts


def storage_value_roles(
    value: str,
    variable_roles: dict[str, set[str]],
    *,
    cwd_is_active_target: bool = False,
) -> set[str]:
    compact = storage_strip_quotes(value).replace('"', "").replace("'", "")
    root_compact = storage_without_trailing_current_dir(value)
    roles: set[str] = set()
    if "s3://" in compact:
        roles.add(STORAGE_ROLE_S3)
    if "rust_verification.py" in compact and "target-dir" in compact:
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if storage_command_substitution_has_target(compact):
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    for variable in storage_variable_names(compact):
        if variable in {"CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_TARGET_TMPDIR"}:
            roles.add(STORAGE_ROLE_ACTIVE_TARGET)
        if variable in {"GITHUB_WORKSPACE", "PWD"} and root_compact in {
            f"${variable}",
            f"${{{variable}}}",
        }:
            roles.add(STORAGE_ROLE_ACTIVE_TARGET)
        roles.update(variable_roles.get(variable, set()))
    if re.search(r"\$\{\{\s*(?:env\.CARGO_TARGET_DIR|steps\.setup\.outputs\.managed_target_dir(?:_relative)?)\s*\}\}", compact):
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if re.search(r"\$\{\{\s*github\.workspace\s*\}\}", root_compact) and (
        re.fullmatch(r"\$\{\{\s*github\.workspace\s*\}\}", root_compact.strip()) is not None
        or storage_value_has_target_component(compact)
    ):
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if root_compact in {".", "*", "$PWD", "${PWD}", "$GITHUB_WORKSPACE", "${GITHUB_WORKSPACE}"}:
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if storage_value_has_target_component(compact):
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    if cwd_is_active_target and compact and not compact.startswith("-") and STORAGE_ROLE_S3 not in roles:
        roles.add(STORAGE_ROLE_ACTIVE_TARGET)
    return roles


def shell_assignment_from_tokens(tokens: list[str], index: int) -> tuple[str, str, int] | None:
    if index >= len(tokens) or not shell_assignment_word(tokens[index]):
        return None
    name, value = tokens[index].split("=", 1)
    cursor = index + 1
    if value == "$" and cursor < len(tokens) and tokens[cursor] == "(":
        depth = 1
        parts = [value, tokens[cursor]]
        cursor += 1
        while cursor < len(tokens) and depth:
            token = tokens[cursor]
            parts.append(token)
            if token == "(":
                depth += 1
            elif token == ")":
                depth -= 1
            cursor += 1
        value = " ".join(parts)
    elif value == "$" and cursor < len(tokens) and tokens[cursor] == "{":
        depth = 1
        parts = [value, tokens[cursor]]
        cursor += 1
        while cursor < len(tokens) and depth:
            token = tokens[cursor]
            parts.append(token)
            if token == "{":
                depth += 1
            elif token == "}":
                depth -= 1
            cursor += 1
        value = " ".join(parts)
    elif value.startswith("`") and not value.endswith("`"):
        parts = [value]
        while cursor < len(tokens):
            token = tokens[cursor]
            parts.append(token)
            cursor += 1
            if token.endswith("`"):
                break
        value = " ".join(parts)
    return name, value, cursor


def storage_assignment_values(text: str) -> list[tuple[str, str]]:
    assignments: list[tuple[str, str]] = []
    tokens = command_tokens(text)
    cursor = 0
    while cursor < len(tokens):
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is None:
            cursor += 1
            continue
        name, value, cursor = assignment
        assignments.append((name, value))
    for line in text.splitlines():
        clean = strip_comment(line).strip()
        match = re.match(r"^([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(.+?)\s*$", clean)
        if match:
            assignments.append((match.group(1), match.group(2)))
            continue
        line_tokens = command_tokens(clean)
        if len(line_tokens) >= 4 and pathlib.Path(line_tokens[0]).name == "echo":
            for redirect_index, token in enumerate(line_tokens):
                if token != ">>":
                    continue
                target = storage_strip_quotes(line_tokens[redirect_index + 1]) if redirect_index + 1 < len(line_tokens) else ""
                if target not in {"$GITHUB_ENV", "${GITHUB_ENV}"}:
                    continue
                payload = " ".join(line_tokens[1:redirect_index])
                if "=" not in payload:
                    continue
                name, value = payload.split("=", 1)
                if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
                    assignments.append((name, value))
                break
    return assignments


def storage_variable_roles(text: str) -> dict[str, set[str]]:
    assignments = storage_assignment_values(text)
    roles: dict[str, set[str]] = {}
    for _ in range(max(1, len(assignments))):
        changed = False
        for name, value in assignments:
            new_roles = storage_value_roles(value, roles)
            if new_roles and not new_roles.issubset(roles.get(name, set())):
                roles.setdefault(name, set()).update(new_roles)
                changed = True
        if not changed:
            break
    return roles


def consume_storage_option(tokens: list[str], index: int, options_with_argument: set[str]) -> int:
    token = tokens[index]
    if token in options_with_argument and index + 1 < len(tokens):
        return index + 2
    return index + 1


def aws_service_index(tokens: list[str], start: int) -> int | None:
    cursor = start + 1
    while cursor < len(tokens) and tokens[cursor] not in SHELL_COMMAND_BOUNDARIES:
        token = tokens[cursor]
        if token in {"s3", "s3api"}:
            return cursor
        if token.startswith("-"):
            if (
                "=" not in token
                and cursor + 1 < len(tokens)
                and tokens[cursor + 1] not in {"s3", "s3api"}
                and not tokens[cursor + 1].startswith("-")
            ):
                cursor += 2
            else:
                cursor += 1
            continue
        cursor += 1
    return None


def aws_s3_operands(tokens: list[str]) -> list[str]:
    operands: list[str] = []
    cursor = 0
    while cursor < len(tokens):
        token = tokens[cursor]
        if token in SHELL_COMMAND_BOUNDARIES:
            break
        if token.startswith("-"):
            cursor = consume_storage_option(tokens, cursor, AWS_S3_OPTIONS_WITH_ARGUMENT)
            continue
        if token == "$" and cursor + 1 < len(tokens) and tokens[cursor + 1] == "(":
            depth = 1
            parts = [token, tokens[cursor + 1]]
            cursor += 2
            while cursor < len(tokens) and depth:
                parts.append(tokens[cursor])
                if tokens[cursor] == "(":
                    depth += 1
                elif tokens[cursor] == ")":
                    depth -= 1
                cursor += 1
            operands.append(" ".join(parts))
            continue
        if token.startswith("`") and not token.endswith("`"):
            parts = [token]
            cursor += 1
            while cursor < len(tokens):
                parts.append(tokens[cursor])
                cursor += 1
                if parts[-1].endswith("`"):
                    break
            operands.append(" ".join(parts))
            continue
        operands.append(token)
        cursor += 1
    return operands


def aws_s3_transfer_touches_active_target(
    tokens: list[str],
    index: int,
    variable_roles: dict[str, set[str]],
    *,
    cwd_is_active_target: bool,
) -> bool:
    service_index = aws_service_index(tokens, index)
    if service_index is None:
        return False
    service = tokens[service_index]
    op_index = service_index + 1
    if op_index >= len(tokens) or tokens[op_index] in SHELL_COMMAND_BOUNDARIES:
        return False
    operation = tokens[op_index]
    tail: list[str] = []
    cursor = op_index + 1
    command_substitution_depth = 0
    while cursor < len(tokens):
        token = tokens[cursor]
        if token == "$" and cursor + 1 < len(tokens) and tokens[cursor + 1] == "(":
            tail.extend([token, tokens[cursor + 1]])
            command_substitution_depth += 1
            cursor += 2
            continue
        if token == "(" and command_substitution_depth:
            command_substitution_depth += 1
        elif token == ")" and command_substitution_depth:
            command_substitution_depth -= 1
        elif token in SHELL_COMMAND_BOUNDARIES:
            break
        tail.append(token)
        cursor += 1
    if service == "s3api":
        return any(
            STORAGE_ROLE_ACTIVE_TARGET in storage_value_roles(token, variable_roles, cwd_is_active_target=cwd_is_active_target)
            for token in tail
        )
    if operation not in AWS_S3_TRANSFER_COMMANDS:
        return False
    operands = aws_s3_operands(tail)
    if len(operands) < 2:
        return False
    endpoint_roles = [
        storage_value_roles(endpoint, variable_roles, cwd_is_active_target=cwd_is_active_target)
        for endpoint in operands
    ]
    return (
        any(STORAGE_ROLE_S3 in roles for roles in endpoint_roles)
        and any(STORAGE_ROLE_ACTIVE_TARGET in roles for roles in endpoint_roles)
    )


def storage_transfer_policy_errors(text: str) -> list[str]:
    variable_roles = storage_variable_roles(text)
    tokens = command_tokens(text)
    cursor = 0
    cwd_is_active_target = False
    while cursor < len(tokens):
        assignment = shell_assignment_from_tokens(tokens, cursor)
        if assignment is not None:
            cursor = assignment[2]
            continue
        token = tokens[cursor]
        if token in SHELL_COMMAND_BOUNDARIES:
            cursor += 1
            continue
        name = executable_name(token)
        if name in {"cd", "pushd"} and cursor + 1 < len(tokens):
            target_roles = storage_value_roles(tokens[cursor + 1], variable_roles, cwd_is_active_target=cwd_is_active_target)
            cwd_is_active_target = STORAGE_ROLE_ACTIVE_TARGET in target_roles
            cursor += 2
            continue
        if name == "aws" and aws_s3_transfer_touches_active_target(
            tokens,
            cursor,
            variable_roles,
            cwd_is_active_target=cwd_is_active_target,
        ):
            return [S3_ACTIVE_TARGET_CACHE_MESSAGE]
        cursor += 1
    return []


def target_env_key_alias(value: str, target_keys: dict[str, str]) -> str | None:
    clean = storage_strip_quotes(value)
    compact = re.sub(r"\s+", "", clean)
    if clean in target_keys:
        return clean
    for target_key in target_keys:
        if target_key not in clean:
            continue
        if compact.startswith("$(") or compact.startswith("`") or compact.startswith("${"):
            return target_key
    return None


def target_env_key_from_assignment_name(
    name: str,
    assignments: dict[str, str],
    target_keys: dict[str, str],
) -> str | None:
    clean = storage_strip_quotes(name)
    if clean in target_keys:
        return clean
    variable = shell_variable_reference_token(clean)
    if variable is not None and variable in assignments:
        return assignments[variable]
    return None


def dynamic_env_assignment_message(
    token: str,
    assignments: dict[str, str],
    target_keys: dict[str, str],
) -> str | None:
    if "=" not in token:
        return None
    name, _value = token.split("=", 1)
    target_key = target_env_key_from_assignment_name(name, assignments, target_keys)
    return target_keys[target_key] if target_key is not None else None


def dynamic_env_segment_messages(
    segment: list[str],
    assignments: dict[str, str],
    target_keys: dict[str, str],
    *,
    depth: int = 0,
) -> set[str]:
    if not segment or depth > 4:
        return set()
    messages: set[str] = set()
    expanded = expand_known_shell_assignment_names(segment, assignments)
    cursor = 0
    while cursor < len(expanded) and shell_assignment_word(expanded[cursor]):
        message = dynamic_env_assignment_message(expanded[cursor], assignments, target_keys)
        if message is not None:
            messages.add(message)
        cursor += 1
    if cursor >= len(expanded):
        return messages
    command = pathlib.Path(expanded[cursor]).name
    if command == "export":
        for argument in expanded[cursor + 1 :]:
            if argument in SHELL_COMMAND_BOUNDARIES:
                break
            message = dynamic_env_assignment_message(argument, assignments, target_keys)
            if message is not None:
                messages.add(message)
        return messages
    if command == "env":
        index = cursor + 1
        while index < len(expanded):
            argument = expanded[index]
            if argument in SHELL_COMMAND_BOUNDARIES:
                break
            if argument == "--":
                index += 1
                continue
            if argument in ENV_OPTIONS_WITHOUT_ARGUMENT or argument in ENV_SIGNAL_OPTIONS:
                index += 1
                continue
            if any(argument.startswith(f"{option}=") for option in ENV_SIGNAL_OPTIONS):
                index += 1
                continue
            if argument in ENV_OPTIONS_WITH_ARGUMENT and index + 1 < len(expanded):
                index += 2
                continue
            if any(
                argument.startswith(f"{option}=")
                for option in ENV_OPTIONS_WITH_ARGUMENT
                if option.startswith("--")
            ):
                index += 1
                continue
            message = dynamic_env_assignment_message(argument, assignments, target_keys)
            if message is None:
                break
            messages.add(message)
            index += 1
        return messages
    if command == "eval":
        inner = expanded[cursor + 1 :]
        if inner and inner[0] == "--":
            inner = inner[1:]
        if inner:
            messages.update(
                dynamic_env_tokens_messages(
                    command_tokens(" ".join(inner)),
                    assignments,
                    target_keys,
                    depth=depth + 1,
                )
            )
    return messages


def dynamic_env_tokens_messages(
    tokens: list[str],
    assignments: dict[str, str],
    target_keys: dict[str, str],
    *,
    depth: int = 0,
) -> set[str]:
    messages: set[str] = set()
    segment: list[str] = []
    for token in tokens + [";"]:
        if token in SHELL_COMMAND_BOUNDARIES:
            messages.update(dynamic_env_segment_messages(segment, assignments, target_keys, depth=depth))
            segment = []
            continue
        segment.append(token)
    return messages


def dynamic_env_target_override_messages(text: str) -> set[str]:
    messages: set[str] = set()
    target_keys = {
        "CARGO_TARGET_DIR": "CARGO_TARGET_DIR raw target override must be classified",
        "CARGO_BUILD_TARGET_DIR": "CARGO_BUILD_TARGET_DIR raw target override must be classified",
        "CARGO_TARGET_TMPDIR": "CARGO_TARGET_TMPDIR raw target override must be classified",
    }
    assignments: dict[str, str] = {}
    for name, value in storage_assignment_values(text):
        target_key = target_env_key_alias(value, target_keys)
        if target_key is not None:
            assignments[name] = target_key
    for line in text.splitlines():
        line_tokens = command_tokens(strip_comment(line))
        line_assignments, is_persistent_assignment = persistent_shell_assignment_values(line_tokens)
        for name, value in line_assignments.items():
            target_key = target_env_key_alias(value, target_keys)
            if target_key is not None and is_persistent_assignment:
                assignments[name] = target_key
    segment: list[str] = []
    for token in command_tokens(text) + [";"]:
        if token in SHELL_COMMAND_BOUNDARIES:
            segment_assignments, is_persistent_assignment = persistent_shell_assignment_values(segment)
            for name, value in segment_assignments.items():
                target_key = target_env_key_alias(value, target_keys)
                if target_key is not None and is_persistent_assignment:
                    assignments[name] = target_key
            segment = []
            continue
        segment.append(token)
    messages.update(dynamic_env_tokens_messages(command_tokens(text), assignments, target_keys))
    return messages


def tokens_define_cargo_alias(tokens: list[str]) -> bool:
    segment: list[str] = []
    for token in tokens:
        if token in SHELL_COMMAND_BOUNDARIES:
            if segment and segment[0] == "alias" and simple_cargo_aliases(segment):
                return True
            segment = []
            continue
        segment.append(token)
    return bool(segment and segment[0] == "alias" and simple_cargo_aliases(segment))


def text_has_alias_cargo_target_routing_override(text: str) -> bool:
    cargo_aliases: set[str] = set()
    for line in text.splitlines():
        if not line.strip():
            continue
        tokens = command_tokens(line)
        segment: list[str] = []
        for token in tokens:
            if token in SHELL_COMMAND_BOUNDARIES:
                if segment and segment[0] == "alias":
                    cargo_aliases.update(simple_cargo_aliases(segment, cargo_aliases))
                elif any(token in cargo_aliases for token in segment):
                    expanded = expand_cargo_aliases(segment, cargo_aliases)
                    if tokens_have_target_routing_override(expanded) and tokens_have_raw_cargo_launch(expanded):
                        return True
                segment = []
                continue
            segment.append(token)
        if segment and segment[0] == "alias":
            cargo_aliases.update(simple_cargo_aliases(segment, cargo_aliases))
            continue
        if not any(token in cargo_aliases for token in segment):
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
        match = re.match(r"^(\s*)(?:-\s*)?run:\s*>[+-]?\s*(?:#.*)?$", line)
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
            indent = len(candidate) - len(candidate.lstrip(" "))
            if indent <= base_indent:
                break
            block.append(candidate.strip())
            index += 1
        if block:
            commands.append(" ".join(block))
    return commands


def raw_rust_storage_errors(workflow_text: str) -> list[str]:
    uncommented = uncommented_text(workflow_text.splitlines())
    folded_commands = "\n".join(folded_yaml_run_commands(uncommented))
    text = re.sub(r"\\\s*\n\s*", " ", "\n".join(part for part in (uncommented, folded_commands) if part))
    checks: tuple[tuple[str, str], ...] = (
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_TARGET_DIR[\"']?\s*(?:=|:)", "CARGO_TARGET_DIR raw target override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_BUILD_TARGET_DIR[\"']?\s*(?:=|:)", "CARGO_BUILD_TARGET_DIR raw target override must be classified"),
        (r"(?:target-dir|build\.target-dir)[^\n]*>\s*\.cargo/config\.toml|\.cargo/config\.toml[^\n]*(?:target-dir|build\.target-dir)", ".cargo/config.toml build.target-dir raw target override must be classified"),
        (r"\bcargo\b[^\n;&|]*\s--config(?:\s+|=)[\"']?build\.target-dir", "cargo --config build.target-dir raw target override must be classified"),
        (r"\bcargo\b[^\n;&|]*\s--config(?:\s+|=)[^\n;&|]*(?:\[build\]|build\s*=|build\.)[^\n;&|]*target-dir", "cargo --config build.target-dir raw target override must be classified"),
        (r"\bcargo\b[^\n;&|]*\s--config(?:\s+|=)[^\n;&|]*(?:\[build\]|build\s*=|build\.)[^\n;&|]*target\\(?:u002[Dd]|U0000002[Dd])dir", "cargo --config build.target-dir raw target override must be classified"),
        (r"\bcargo\b[^\n;&|]*\s--config(?:\s+|=)[^\n;&|]*(?:build\.rustflags|rustflags\s*=)[^\n;&|]*(?:--out-dir|--artifact-dir)", "cargo --config build.rustflags raw output override must be classified"),
        (r"\bcargo\b[^\n;&|]*\s--config(?:\s+|=)[\"']?(?:/|\./|[^\s\n;&|\"']+\.toml\b)", "cargo --config file raw target override must be classified"),
        (r"\bcargo\b[^\n;&|]*\s--target-dir\b", "cargo --target-dir raw target override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_TARGET_TMPDIR[\"']?\s*(?:=|:)", "CARGO_TARGET_TMPDIR raw target override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_INCREMENTAL[\"']?\s*(?:=|:)", "CARGO_INCREMENTAL raw cache override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_BUILD_RUSTFLAGS[\"']?\s*(?:=|:).*(?:--out-dir|--artifact-dir)", "CARGO_BUILD_RUSTFLAGS raw output override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_ENCODED_RUSTFLAGS[\"']?\s*(?:=|:).*(?:--out-dir|--artifact-dir)", "CARGO_ENCODED_RUSTFLAGS raw output override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_INSTALL_ROOT[\"']?\s*(?:=|:)", "CARGO_INSTALL_ROOT install output override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?CARGO_HOME[\"']?\s*(?:=|:)", "CARGO_HOME raw cache override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?RUSTUP_HOME[\"']?\s*(?:=|:)", "RUSTUP_HOME raw toolchain override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?RUSTFLAGS[\"']?\s*(?:=|:).*(?:--out-dir|--artifact-dir)", "RUSTFLAGS raw output override must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?RUSTC_WRAPPER[\"']?\s*(?:=|:)", "RUSTC_WRAPPER raw compiler wrapper must be classified"),
        (r"(^|[^A-Za-z0-9_])[\"']?RUSTC_WORKSPACE_WRAPPER[\"']?\s*(?:=|:)", "RUSTC_WORKSPACE_WRAPPER raw compiler wrapper must be classified"),
        (r"\bcargo\b[^\n;&|]*\brustc\b[^\n;&|]*\s--out-dir\b", "cargo rustc --out-dir raw output override must be classified"),
        (r"\bcargo\b[^\n;&|]*\brustc\b[^\n;&|]*\s--artifact-dir\b", "cargo rustc --artifact-dir raw output override must be classified"),
        (r"\bcargo\b[^\n;&|]*\binstall\b(?=[^\n;&|]*\s--target-dir\b)(?=[^\n;&|]*\s--root\b)", "cargo install build target and install root ownership must be classified separately"),
        (r"\bcargo\b[^\n;&|]*\binstall\b[^\n;&|]*\s--root\b[^\n;&|]*\bs3://", "cargo install S3 install root must be classified"),
        (r"(^|[^A-Za-z0-9_$\{])[\"']?BOLT_MANAGED_JUST[\"']?\s*(?:=|:|<<)", "BOLT_MANAGED_JUST private just recipe bypass must be classified"),
        (r"\bno-mistakes\b[^\n]*\bcargo\b", "no-mistakes raw Cargo drift must be classified"),
        (r"\bno-mistakes\b[^\n]*--worktree[^\n]*(?:--target-dir\s+target|\btarget\b)", "no-mistakes worktree-local target path evidence must be reported"),
        (r"\bcargo\b[^\n|]*\$@[^|]*\|\s*bash\b[^\n;&|]*\s-s\b[^\n;&|]*\s--target-dir\b", "cargo --target-dir raw target override must be classified"),
    )
    errors: list[str] = []
    for pattern, message in checks:
        if re.search(pattern, text):
            errors.append(message)
    for message in sorted(text_raw_cargo_storage_override_messages(text)):
        if message not in errors:
            errors.append(message)
    for message in sorted(dynamic_env_target_override_messages(text)):
        if message not in errors:
            errors.append(message)
    config_file_message = "cargo --config file raw target override must be classified"
    if text_has_path_style_cargo_config(text) and config_file_message not in errors:
        errors.append(config_file_message)
    target_override_message = "cargo --target-dir raw target override must be classified"
    if (
        text_has_alias_cargo_target_routing_override(text)
        and target_override_message not in errors
    ):
        errors.append(target_override_message)
    for message in storage_transfer_policy_errors(text):
        if message not in errors:
            errors.append(message)
    return errors


def consume_cargo_global_options(tokens: list[str], index: int) -> int:
    while index < len(tokens):
        token = tokens[index]
        if token.startswith("+"):
            index += 1
            continue
        if token in CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT:
            index += 2
            continue
        if any(token.startswith(f"{option}=") for option in CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT):
            index += 1
            continue
        if token in CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT:
            index += 1
            continue
        if token.startswith("-"):
            index += 1
            continue
        break
    return index


def test_has_shard_reproduction_command(job_lines: list[str]) -> bool:
    return job_runs_command(job_lines, TEST_REPRODUCTION_ECHO)


def test_has_inline_shard_reproduction_command(job_lines: list[str]) -> bool:
    for block in step_blocks(job_lines):
        for line in block:
            clean = strip_comment(line).strip()
            if clean.startswith(("run:", "- run:")) and "reproduce" in clean.lower() and TEST_REPRODUCTION_COMMAND in clean:
                return True
    return False


def job_skips_tag_reuse(job_lines: list[str]) -> bool:
    return has_line_matching(job_lines, TAG_SKIP_IF_RE) or has_line_matching(job_lines, TAG_SKIP_ALWAYS_IF_RE)


def job_if_uses_always(job_lines: list[str]) -> bool:
    return has_line_matching(job_lines, GATE_IF_RE) or has_line_matching(job_lines, TAG_SKIP_ALWAYS_IF_RE)


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


def check_aarch64_has_coverage_owner_step(job_lines: list[str]) -> bool:
    for block in step_blocks(job_lines):
        text = uncommented_text(block)
        if "Resolve aarch64 coverage owner" not in text:
            continue
        return (
            "needs.detector.outputs.build_required" in text
            and "aarch64 coverage is provided by build" in text
            and "running standalone aarch64 check" in text
        )
    return False


def check_aarch64_standalone_guard_errors(job_lines: list[str]) -> list[str]:
    errors: list[str] = []
    checks = (
        (
            "check-aarch64 setup must run only when build_required is not true",
            lambda block: any("./.github/actions/setup-environment" in line for line in block),
        ),
        (
            "check-aarch64 compiler install must run only when build_required is not true",
            lambda block: "gcc-aarch64-linux-gnu" in uncommented_text(block)
            or "libc6-dev-arm64-cross" in uncommented_text(block),
        ),
        (
            "check-aarch64 cache must run only when build_required is not true",
            lambda block: any("Swatinem/rust-cache" in line for line in block),
        ),
        (
            "check-aarch64 managed target cache must run only when build_required is not true",
            block_uses_managed_target_cache,
        ),
        (
            "check-aarch64 command must run only when build_required is not true",
            lambda block: block_runs_command(block, "just check-aarch64"),
        ),
    )
    blocks = step_blocks(job_lines)
    for message, matches in checks:
        for block in blocks:
            if matches(block) and not has_line_matching(block, CHECK_AARCH64_STANDALONE_IF_RE):
                errors.append(message)
                break
    return errors


GATE_TAG_REUSE_CONDITION = '"$tag_ref" == "true"'


def gate_checks_lane_success(gate_text: str, job: str) -> bool:
    condition = f'"${{{{ needs.{job}.result }}}}" != "success"'
    return branch_exits_reachable(gate_text, "if", condition)


def top_level_if_body_and_remainder(gate_text: str, condition: str) -> tuple[str, str] | None:
    lines = gate_text.splitlines()
    for start, line in enumerate(lines):
        match = IF_OR_ELIF_RE.match(line)
        if not match or match.group(1) != "if" or match.group("condition") != condition:
            continue
        depth = 0
        for index in range(start + 1, len(lines)):
            nested_match = IF_OR_ELIF_RE.match(lines[index])
            if nested_match and nested_match.group(1) == "if":
                depth += 1
                continue
            if not FI_RE.match(lines[index]):
                continue
            if depth == 0:
                return "\n".join(lines[start + 1 : index]), "\n".join(lines[index + 1 :])
            depth -= 1
    return None


def gate_tag_reuse_body(gate_text: str) -> str:
    sections = top_level_if_body_and_remainder(gate_text, GATE_TAG_REUSE_CONDITION)
    return sections[0] if sections is not None else ""


def gate_standard_body(gate_text: str) -> str:
    sections = top_level_if_body_and_remainder(gate_text, GATE_TAG_REUSE_CONDITION)
    return sections[1] if sections is not None else ""


def gate_checks_standard_lane_success(gate_text: str, job: str) -> bool:
    return gate_checks_lane_success(gate_standard_body(gate_text), job)


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
        and branch_exits_reachable(chain.get(("if", required_condition), ""), "if", true_result_condition)
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
    tag_body = gate_tag_reuse_body(gate_text)
    standard_body = gate_standard_body(gate_text)
    if 'tag_ref="${{ startsWith(github.ref, \'refs/tags/v\') }}"' not in gate_text and (
        'tag_ref="${{ startsWith(github.ref, "refs/tags/v") }}"' not in gate_text
    ):
        errors.append("gate must compute tag_ref")
    if not branch_exits_reachable(tag_body, "if", '"${{ needs.same-sha-main-evidence.result }}" != "success"'):
        errors.append("gate must check same-sha-main-evidence success")
    if not branch_exits_reachable(standard_body, "if", '"${{ needs.same-sha-main-evidence.result }}" != "skipped"'):
        errors.append("gate must check same-sha-main-evidence skip on non-tag")
    for job in TAG_SKIPPED_JOBS:
        if not branch_exits_reachable(tag_body, "if", f'"${{{{ needs.{job}.result }}}}" != "skipped"'):
            errors.append(f"gate must require {job} skipped on tag reuse")
    if not branch_exits_reachable(tag_body, "if", '"${{ needs.check-aarch64.result }}" != "success"'):
        errors.append("gate must require check-aarch64 success on tag reuse")
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


def deploy_verifies_downloaded_artifact_checksum(job_lines: list[str]) -> bool:
    text = uncommented_text(job_lines)
    return "cd artifact" in text and "sha256sum -c bolt-v2.sha256" in text


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


def shell_line_exit_codes(line: str) -> list[str | None]:
    codes: list[str | None] = []
    tokens = command_tokens(line)
    cursor = 0
    at_command_start = True
    previous_boundary: str | None = None
    while cursor < len(tokens):
        token = tokens[cursor]
        if token in SHELL_COMMAND_BOUNDARIES:
            at_command_start = True
            previous_boundary = token
            cursor += 1
            continue
        if at_command_start and pathlib.Path(token).name == "exit":
            if previous_boundary != "||":
                code = tokens[cursor + 1] if cursor + 1 < len(tokens) and re.fullmatch(r"[0-9]+", tokens[cursor + 1]) else None
                codes.append(code)
        at_command_start = False
        cursor += 1
    return codes


def shell_line_has_exit_command(line: str) -> bool:
    tokens = command_tokens(line)
    at_command_start = True
    for token in tokens:
        if token in SHELL_COMMAND_BOUNDARIES:
            at_command_start = True
            continue
        if at_command_start and pathlib.Path(token).name == "exit":
            return True
        at_command_start = False
    return False


def shell_line_is_simple_exit(line: str) -> bool:
    tokens = command_tokens(line)
    if not tokens or pathlib.Path(tokens[0]).name != "exit":
        return False
    if len(tokens) == 1:
        return True
    return len(tokens) == 2 and re.fullmatch(r"[0-9]+", tokens[1]) is not None


def branch_is_reachable_before_top_level_exit(gate_text: str, keyword: str, condition: str) -> bool:
    depth = 0
    for line in gate_text.splitlines():
        clean = strip_comment(line).strip()
        if not clean:
            continue
        if FI_RE.match(line):
            depth = max(0, depth - 1)
            continue
        branch_match = IF_OR_ELIF_RE.match(line)
        if branch_match:
            if (
                depth == 0
                and branch_match.group(1) == keyword
                and branch_match.group("condition") == condition
            ):
                return True
            if branch_match.group(1) == "if":
                depth += 1
            continue
        if ELSE_RE.match(line):
            continue
        if depth == 0 and shell_line_exit_codes(clean):
            return False
    return False


def branch_exits_reachable(gate_text: str, keyword: str, condition: str) -> bool:
    if not branch_is_reachable_before_top_level_exit(gate_text, keyword, condition):
        return False
    return branch_exits(gate_text, keyword, condition)


def body_exits(body: str) -> bool:
    exit_codes: list[str | None] = []
    depth = 0
    for line in body.splitlines():
        clean = strip_comment(line).strip()
        if not clean:
            continue
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
        line_exit_codes = shell_line_exit_codes(clean)
        if depth != 0:
            continue
        if shell_line_has_exit_command(clean):
            if not shell_line_is_simple_exit(clean):
                return False
            exit_codes.extend(line_exit_codes)
            continue
        if clean.startswith("echo "):
            continue
        return False
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
    errors.extend(raw_rust_storage_errors(workflow_text))
    errors.extend(exact_head_governance_cache_errors(workflow_text))

    actual_pr_paths_ignore = extract_paths_ignore_for_trigger(workflow_text, "pull_request")
    if actual_pr_paths_ignore is None or tuple(sorted(actual_pr_paths_ignore)) != CI_PR_PATHS_IGNORE_BASELINE:
        errors.append(
            "on.pull_request paths-ignore must match baseline "
            f"{CI_PR_PATHS_IGNORE_BASELINE} (got {actual_pr_paths_ignore!r})"
        )
    actual_push_paths_ignore = extract_paths_ignore_for_trigger(workflow_text, "push")
    if actual_push_paths_ignore is not None:
        errors.append(
            "on.push must have no paths-ignore (push to main/tags must always run full CI); "
            f"got {actual_push_paths_ignore!r}"
        )

    errors.extend(verify_pr_concurrency(workflow_text))

    if not workflow_permissions_have_actions_read(workflow_text):
        errors.append("workflow permissions must include actions: read")

    for job in REQUIRED_JOBS:
        if job not in jobs:
            errors.append(f"missing required job {job}")

    if "fmt-check" in jobs and "detector" in extract_needs(jobs["fmt-check"]):
        errors.append("fmt-check must not need detector")

    for job in TAG_SKIP_REQUIRED_JOBS:
        if job in jobs and not job_skips_tag_reuse(jobs[job]):
            errors.append(f"{job} must skip on tag reuse")

    if "source-fence" in jobs and "detector" not in extract_needs(jobs["source-fence"]):
        # FR-005: #342 owns the early-fail source-fence lane, so it remains detector-gated.
        errors.append("source-fence needs detector")

    for job_name, recipe in JOB_REQUIRED_JUST_RECIPE.items():
        if job_name in jobs and not job_runs_command(jobs[job_name], f"just {recipe}"):
            errors.append(f"{job_name} must run just {recipe}")

    if "test-archive" in jobs:
        test_archive_needs = extract_needs(jobs["test-archive"])
        if "detector" not in test_archive_needs:
            errors.append("test-archive needs detector")
        # #400: source-fence and test-archive run in parallel. The aggregate
        # `gate` job is the sole merge enforcer for both lanes; reintroducing a
        # serial dep would re-create the fail-fast cost #400 eliminated.
        if "source-fence" in test_archive_needs:
            errors.append("test-archive must not need source-fence")
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
        if has_line_matching(jobs["check-aarch64"], CHECK_AARCH64_JOB_LEVEL_IF_RE):
            errors.append("check-aarch64 must have no job-level if condition")
        if not check_aarch64_has_coverage_owner_step(jobs["check-aarch64"]):
            errors.append("check-aarch64 must document build-lane aarch64 coverage delegation")
        if not check_aarch64_installs_cross_compiler_packages(jobs["check-aarch64"]):
            errors.append("check-aarch64 must install aarch64 cross compiler packages")
        errors.extend(check_aarch64_standalone_guard_errors(jobs["check-aarch64"]))

    if "test-archive" in jobs:
        archive_lines = jobs["test-archive"]
        archive_text = uncommented_text(archive_lines)
        if TEST_ARCHIVE_PATH not in archive_text:
            errors.append("test-archive must declare nextest archive path")
        if not all(input_fragment in archive_text for input_fragment in TEST_ARCHIVE_KEY_INPUTS):
            errors.append("test-archive cache key must include Rust and test graph inputs")
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
        if not job_if_uses_always(jobs["test"]):
            errors.append("test must use always()")

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
            elif job == "detector":
                checks_result = gate_checks_lane_success(gate_text, job)
            else:
                checks_result = gate_checks_standard_lane_success(gate_text, job)
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
        if not deploy_verifies_downloaded_artifact_checksum(jobs["deploy"]):
            errors.append("deploy must verify downloaded artifact checksum")

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

    for job in REGISTRY_CACHE_JOBS:
        if job in jobs:
            errors.extend(shared_registry_cache_errors(job, jobs[job]))

    for job in MANAGED_TARGET_CACHE_KEYS:
        if job in jobs:
            errors.extend(managed_target_cache_errors(job, jobs[job]))

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


def verify_prebuilt_tool_installs(workflow_text: str, workflow_name: str) -> list[str]:
    errors: list[str] = []

    jobs = parse_jobs(workflow_text)
    for job, job_lines in jobs.items():
        for tool in sorted(cargo_install_source_build_tools_in_text(uncommented_text(job_lines))):
            errors.append(f"{workflow_name} {job} must not compile {tool} from source")

    for job, (tool, output) in CI_INSTALL_ACTION_TOOLS.items():
        job_lines = jobs.get(job)
        if job_lines is None:
            continue
        step = install_action_tool_step(job_lines, tool, output)
        if step is None:
            errors.append(f"{workflow_name} {job} must install {tool} with pinned taiki-e/install-action")
            continue
        install_index, block = step
        if not block_has_input(block, "fallback", "none"):
            errors.append(f"{workflow_name} {job} install-action fallback must be none")
        command = CI_INSTALL_ACTION_COMMANDS[job]
        command_index = first_step_running_command(job_lines, command)
        if command_index is not None and install_index >= command_index:
            errors.append(f"{workflow_name} {job} must install {tool} before {command}")

    build_lines = jobs.get("build")
    if build_lines is None:
        return errors
    build_text = uncommented_text(build_lines)
    if "archive.sha256" in build_text or "steps.setup.outputs.zigbuild_x86_64_unknown_linux_gnu_sha256" not in build_text:
        errors.append(f"{workflow_name} build must use pinned cargo-zigbuild archive sha256")
    zigbuild_install_index = first_step_containing_literals_in_order(build_lines, ZIGBUILD_PREBUILT_LITERALS)
    if zigbuild_install_index is None:
        errors.append(f"{workflow_name} build must install cargo-zigbuild from checksum-verified prebuilt release")
    else:
        build_command_index = first_step_running_command(build_lines, "just build")
        if build_command_index is not None and zigbuild_install_index >= build_command_index:
            errors.append(f"{workflow_name} build must install cargo-zigbuild before just build")
    if 'test "$actual" = "$expected"' not in build_text:
        errors.append(f"{workflow_name} build must verify cargo-zigbuild archive checksum")
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


def repo_automation_source_build_errors(text: str) -> list[str]:
    return [
        f"repo automation must not compile {tool} from source"
        for tool in sorted(cargo_install_source_build_tools_in_text(text))
    ]


def verify_repo_automation_texts(texts: dict[str, str]) -> list[str]:
    errors: list[str] = []
    for file_name, text in texts.items():
        errors.extend(f"{file_name}: {error}" for error in raw_rust_storage_errors(text))
        errors.extend(f"{file_name}: {error}" for error in repo_automation_raw_cargo_errors(file_name, text))
        errors.extend(f"{file_name}: {error}" for error in repo_automation_source_build_errors(text))
    return errors


def verify_workflows(workflows: dict[str, str], action_text: str, nextest_config_text: str) -> list[str]:
    errors: list[str] = []
    for workflow_name, workflow_text in workflows.items():
        if workflow_name == "ci.yml" or workflow_name.endswith("/ci.yml"):
            errors.extend(verify_workflow(workflow_text))
        else:
            errors.extend(raw_rust_storage_errors(workflow_text))
        errors.extend(verify_managed_workflow(workflow_text, workflow_name))
        errors.extend(verify_build_artifacts(workflow_text, workflow_name))
        errors.extend(verify_prebuilt_tool_installs(workflow_text, workflow_name))
    errors.extend(raw_rust_storage_errors(action_text))
    errors.extend(verify_setup_action(action_text))
    errors.extend(verify_nextest_config(nextest_config_text))
    errors.extend(verify_install_action_pin_consistency(workflows))
    return errors


def verify_install_action_pin_consistency(workflows: dict[str, str]) -> list[str]:
    # Dependabot groups action bumps so all taiki-e/install-action pins move
    # together; this guards against half-bumps in human-authored PRs that
    # leave workflow files referencing inconsistent SHAs. Scan line-by-line
    # after stripping comments so commentary containing the action ref does
    # not produce false positives.
    #
    # The broad detector (TAIKI_INSTALL_ACTION_MENTION_RE) finds every line
    # that mentions the action ref at all — including YAML multi-line scalar
    # form where `uses:` sits on a preceding line. Any such line that does
    # not match the strict single-line pinned form is reported with a precise
    # file:line so mutable tags (e.g. @v2), multi-line scalars, mismatched
    # quotes, and other malformed pins fail loudly instead of being silently
    # skipped. SHAs are lowercased before bucketing so the consistency check
    # treats uppercase and lowercase hex as the same pin. Lines that fail
    # the strict form do NOT contribute to the bucket map — a malformed
    # reference must not phantom-bucket and mask a real drift.
    errors: list[str] = []
    sha_to_files: dict[str, list[str]] = {}
    for workflow_name, workflow_text in workflows.items():
        for line_index, line in enumerate(workflow_text.splitlines(), start=1):
            clean = strip_comment(line)
            if not TAIKI_INSTALL_ACTION_MENTION_RE.search(clean):
                continue
            match = TAIKI_INSTALL_ACTION_RE.match(clean)
            if match is None:
                errors.append(
                    f"{workflow_name}:{line_index}: taiki-e/install-action must be referenced as "
                    f"'uses: taiki-e/install-action@<40-hex-SHA>' on a single line, got: {clean.strip()}"
                )
                continue
            sha = match.group(2).lower()
            sha_to_files.setdefault(sha, []).append(workflow_name)
    if len(sha_to_files) > 1:
        parts = sorted(
            f"{sha} in {','.join(sorted(set(files)))}"
            for sha, files in sha_to_files.items()
        )
        errors.append("taiki-e/install-action pin drift: " + "; ".join(parts))
    return errors


def main() -> int:
    workflow_texts = {workflow.relative_to(REPO_ROOT).as_posix(): workflow.read_text() for workflow in DEFAULT_WORKFLOWS if workflow.exists()}
    action_text = DEFAULT_SETUP_ACTION.read_text()
    nextest_config_text = DEFAULT_NEXTEST_CONFIG.read_text()
    repo_automation_texts = {
        path.relative_to(REPO_ROOT).as_posix(): path.read_text()
        for path in DEFAULT_REPO_AUTOMATION_FILES
        if path.exists()
    }
    for directory, pattern in DEFAULT_REPO_AUTOMATION_GLOBS:
        if not directory.exists():
            continue
        for path in sorted(directory.glob(pattern)):
            repo_automation_texts[path.relative_to(REPO_ROOT).as_posix()] = path.read_text()
    errors = verify_workflows(workflow_texts, action_text, nextest_config_text)
    errors.extend(verify_repo_automation_texts(repo_automation_texts))
    if DEFAULT_NO_MISTAKES_CONFIG.exists():
        errors.extend(verify_no_mistakes_config(DEFAULT_NO_MISTAKES_CONFIG.read_text()))
    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: CI workflow hygiene verifier passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
