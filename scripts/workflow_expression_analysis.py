#!/usr/bin/env python3
"""Workflow expression helpers relocated from verify_ci_workflow_hygiene."""

from __future__ import annotations

import functools
import re

YAML_ANCHOR_PATTERN = r"&[A-Za-z0-9_.-]+"
YAML_KEY_PATTERN = r"""(?:[A-Za-z0-9_.-]+|'[^']*(?:''[^']*)*'|"(?:[^"\\]|\\.)*")"""
IF_OR_ELIF_RE = re.compile(r"^\s*(if|elif)\s+\[\[\s*(?P<condition>.*?)\s*\]\];\s*then\s*$")
ELSE_RE = re.compile(r"^\s*else\s*$")
FI_RE = re.compile(r"^\s*fi\s*$")

# verify_text re-parses the same workflow lines many times across a run; this
# helper is a pure function of a single str, so an unbounded short-lived cache is safe.
@functools.cache
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
        if char == "#" and (index == 0 or line[index - 1].isspace()):
            return line[:index].rstrip()
    return line.rstrip()
def _normalize_concurrency_text(text: str) -> str:
    """Collapse all runs of whitespace to single spaces so formatting (YAML
    folding, indentation, line wrapping) does not affect the allowlist match."""
    return " ".join(text.split())
SAFE_CANCEL_EVENT_RE = re.compile(
    r"github\.event_name\s*==\s*(['\"])(pull_request|workflow_dispatch)\1"
)
KNOWN_SAFE_CANCEL_FORMS = frozenset(
    {
        "${{ github.event_name == 'pull_request' && !(github.event.pull_request.draft == false "
        "&& (github.event.action == 'reopened' || (github.event.action == 'edited' "
        "&& !(github.event.changes.base.ref.from && true || false)))) || github.event_name == 'workflow_dispatch' }}",
        "${{ github.event_name == 'pull_request' "
        "&& !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/') "
        "|| startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) "
        "&& !(github.event.pull_request.draft == false && (github.event.action == 'reopened' "
        "|| (github.event.action == 'edited' && !(github.event.changes.base.ref.from && true || false)))) "
        "|| github.event_name == 'workflow_dispatch' }}",
        "${{ github.event_name == 'pull_request' "
        "&& !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/') "
        "|| startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) }}",
        "${{ github.event_name == 'pull_request' "
        "&& !(startsWith(github.event.pull_request.head.ref, 'mergify/merge-queue/') "
        "|| startsWith(github.event.pull_request.head.ref, 'tmp-mergify/merge-queue/')) "
        "&& !(github.event.pull_request.draft == false && (github.event.action == 'reopened' "
        "|| (github.event.action == 'edited' && !(github.event.changes.base.ref.from && true || false)))) }}",
    }
)


def _cancel_in_progress_value(cancel_text: str) -> str:
    """Extract the cancel-in-progress scalar as one normalized line, dropping
    the key and any YAML folding indicator (>-, |, …)."""
    marker = "cancel-in-progress:"
    idx = cancel_text.find(marker)
    raw = cancel_text[idx + len(marker):] if idx != -1 else cancel_text
    tokens = raw.split()
    if tokens and tokens[0] in {">-", ">+", ">", "|-", "|+", "|"}:
        tokens = tokens[1:]
    return " ".join(tokens)


def cancel_in_progress_is_merge_group_safe(cancel_text: str) -> bool:
    """True only when cancel-in-progress is provably false for the merge_group
    event: the literal false, or a ${{ }} expression whose only truthy operands
    are pull_request/workflow_dispatch equality arms. Any negation, function
    call, literal true, or other event name leaves residue and fails closed."""
    value = _cancel_in_progress_value(cancel_text)
    if value == "false":
        return True
    if _normalize_concurrency_text(value) in KNOWN_SAFE_CANCEL_FORMS:
        return True
    match = re.fullmatch(r"\$\{\{(.*)\}\}", value, re.DOTALL)
    if not match:
        return False
    inner = match.group(1)
    if "!" in inner:
        return False
    residue = SAFE_CANCEL_EVENT_RE.sub("", inner)
    for token in ("||", "(", ")"):
        residue = residue.replace(token, " ")
    return residue.strip() == ""
def unquote_yaml_scalar(value: str) -> str:
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {"'", '"'}:
        return value[1:-1]
    return value
GATE_NAME_OUTPUT = "name: ${{ needs.ci-policy.outputs.gate_name }}"
TAG_SKIPPED_JOBS = (
    "deny",
    "clippy",
    "source-fence",
    "nextest-fingerprint",
    "test-archive",
    "nextest-fingerprint-reuse",
    "test",
    "build",
    "ci-provenance-emit",
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
    for job in (*TAG_SKIPPED_JOBS, "same-sha-main-evidence", "check-aarch64"):
        required_arg = f"--job {job}=${{{{ needs.{job}.result }}}}"
        if required_arg not in gate_text:
            errors.append(f"gate shared verdict call must include {required_arg}")
    return errors


def gate_checks_nextest_fingerprint_reuse(gate_text: str) -> list[str]:
    errors: list[str] = []
    for required in (
        "--reuse-found",
        "needs.nextest-fingerprint-reuse.outputs.reuse_found",
        "--job nextest-fingerprint-reuse=${{ needs.nextest-fingerprint-reuse.result }}",
        "--job ci-provenance-emit=${{ needs.ci-provenance-emit.result }}",
    ):
        if required not in gate_text:
            errors.append(f"gate shared verdict call must include {required}")
    return errors


def gate_policy_truth_table_errors(gate_text: str) -> list[str]:
    errors: list[str] = []
    if GATE_NAME_OUTPUT not in gate_text:
        errors.append("gate name must come from ci-policy gate_name output")
    for required in (
        "steps.verdict_base.outputs.script",
        'python3 "$verdict_script" check-ci-gate',
    ):
        if required not in gate_text:
            errors.append(f"gate must use trusted base-tree ci_provenance.py check-ci-gate verdict ({required})")
    for required in (
        "--policy-path \"${{ needs.ci-policy.outputs.ci_policy_path }}\"",
        "--expected-event-class \"${{ needs.ci-policy.outputs.expected_event_class }}\"",
        "--full-ci-deferred \"${{ needs.ci-policy.outputs.full_ci_deferred }}\"",
        "--ignore-emit-failure \"${{ needs.ci-policy.outputs.ignore_emit_failure }}\"",
        "carry_forward_args=()",
        "carry_forward_verified=\"${{ steps.carry_forward.outputs.carry_forward_verified }}\"",
        "if [[ -n \"$carry_forward_verified\" ]]; then",
        "carry_forward_args+=(--carry-forward-verified \"$carry_forward_verified\")",
        "\"${carry_forward_args[@]}\"",
        "--build-required \"${{ needs.detector.outputs.build_required || 'false' }}\"",
        "--job ci-policy=${{ needs.ci-policy.result }}",
        "--job detector=${{ needs.detector.result }}",
        "--job same-sha-main-evidence=${{ needs.same-sha-main-evidence.result }}",
    ):
        if required not in gate_text:
            errors.append(f"gate shared verdict call must include {required}")
    for job in (
        "deny",
        "clippy",
        "check-aarch64",
        "source-fence",
        "nextest-fingerprint",
        "test-archive",
        "nextest-fingerprint-reuse",
        "test",
        "build",
        "ci-provenance-emit",
    ):
        required = f"--job {job}=${{{{ needs.{job}.result }}}}"
        if required not in gate_text:
            errors.append(f"gate shared verdict call must include {required}")
    if 'python3 "$verdict_script" resolve-gate-carry-forward' not in gate_text:
        errors.append("gate must verify carry-forward through trusted base-tree ci_provenance.py")
    if "--require-provenance-base true" not in gate_text:
        errors.append("gate carry-forward must require provenance base match")
    return errors
def one_indexed_sequence(values: tuple[int, ...]) -> bool:
    return values == tuple(range(1, len(values) + 1))
def simple_shell_lines(run_text: str) -> tuple[str, ...]:
    return tuple(line.strip() for line in run_text.splitlines() if line.strip())
def simple_bte_run_block_partition_denominators(run_block: str) -> tuple[int, ...]:
    # Allowlist the whole BVS shell block instead of predicting shell wrapper syntax.
    lines = simple_shell_lines(run_block)
    if lines and lines[0].startswith('log="$RUNNER_TEMP/'):
        lines = lines[1:]
    expected_head = ("rc=0", "set +e")
    if lines[: len(expected_head)] != expected_head:
        return ()
    lines = lines[len(expected_head) :]

    run_prefix = (
        'just bte-test --config-file "$RUNNER_TEMP/nextest-junit.toml" '
        '--partition "count:${{ matrix.shard }}/'
    )
    run_suffix = '" -- --skip issue_789_first_real_free_data_taker_pl'
    run_tee_suffix = f'{run_suffix} 2>&1 | tee -a "$log"'

    def partition_denominator(command: str, prefix: str, suffix: str) -> str | None:
        if not command.startswith(prefix) or not command.endswith(suffix):
            return None
        value = command[len(prefix) : -len(suffix)]
        if not value or value[0] == "0" or not value.isdecimal():
            return None
        return value

    old_execution_tails = (
        (run_suffix, ("rc=$?", "set -e", "printf 'MERGIFY_TEST_EXIT_CODE=%s\\n' \"$rc\" >> \"$GITHUB_ENV\"", 'exit "$rc"')),
        (
            run_tee_suffix,
            ('rc="${PIPESTATUS[0]}"', "set -e", "printf 'MERGIFY_TEST_EXIT_CODE=%s\\n' \"$rc\" >> \"$GITHUB_ENV\"", 'exit "$rc"'),
        ),
    )
    for command_suffix, tail in old_execution_tails:
        if len(lines) == 1 + len(tail) and lines[1:] == tail:
            denominator = partition_denominator(lines[0], run_prefix, command_suffix)
            return (int(denominator),) if denominator is not None else ()
    return ()
