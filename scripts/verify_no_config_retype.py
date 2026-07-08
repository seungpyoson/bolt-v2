#!/usr/bin/env python3
"""Fence copied config values in Python scripts."""

from __future__ import annotations

import ast
import dataclasses
import pathlib
import re
import sys
from collections.abc import Iterator
from typing import Any


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPTS_DIR = pathlib.Path(__file__).resolve().parent

GOVERNED_CONFIG_ARTIFACTS = (
    ".mergify.yml",
    ".no-mistakes.yaml",
    "ci/ai-review.toml",
    "ci/bolt-v3-boundary-exemptions.toml",
    "ci/chainlink-reference-fixture-capture-provenance.toml",
    "ci/developer-tool-storage-hygiene.toml",
    "ci/doc-decoupling-residuals.toml",
    "ci/fail-closed-contracts.toml",
    "ci/fail-closed-exceptions.toml",
    "ci/github-actions-runners.toml",
    "ci/nextest-fingerprint.toml",
    "ci/rust-ci-inputs.toml",
    "ci/rust-verification.toml",
    "ci/storage-tripwire.toml",
    "config/clean-merged.toml",
    "config/deploy.toml",
    "config/root.toml",
)

STRICT_SCRIPT_PATHS = frozenset(
    {
        # Current tranche.
        "scripts/contract_engine.py",
        "scripts/contract_rules.py",
        "scripts/test_merge_queue_preflight.py",
        "scripts/test_verify_ci_workflow_hygiene.py",
        "scripts/test_verify_fail_closed_contracts.py",
        "scripts/verify_ci_workflow_hygiene.py",
        "scripts/verify_fail_closed_contracts.py",
        "scripts/workflow_model.py",
        "scripts/verify_no_config_retype.py",
        "scripts/test_verify_no_config_retype.py",
        # #1301.
        "scripts/ci_provenance.py",
        "scripts/merge_queue_operator.py",
        "scripts/test_merge_queue_operator.py",
        "scripts/test_rust_verification_decoupling.py",
        "scripts/test_verifier_io.py",
        "scripts/test_verify_bolt_v3_core_boundary.py",
        "scripts/test_verify_bolt_v3_naming.py",
        "scripts/test_verify_ra_notebook_read_only_boundary.py",
        "scripts/test_verify_ra_single_engine_import_boundary.py",
        "scripts/verifier_io.py",
        "scripts/verify_bolt_v3_core_boundary.py",
        "scripts/verify_bolt_v3_naming.py",
        "scripts/verify_dashboard_read_only_contract.py",
        "scripts/verify_ra_notebook_read_only_boundary.py",
        "scripts/verify_ra_single_engine_import_boundary.py",
    }
)

RATCHET_BASELINE = 1467


@dataclasses.dataclass(frozen=True)
class RegisteredPayload:
    payload: str
    reason: str
    paths: tuple[str, ...] = ()


def registered_payload_group(
    reason: str,
    paths: tuple[str, ...],
    payloads: tuple[str, ...],
) -> tuple[RegisteredPayload, ...]:
    return tuple(RegisteredPayload(payload, reason, paths) for payload in payloads)


REGISTERED_VIOLATION_PAYLOADS: tuple[RegisteredPayload, ...] = (
    *registered_payload_group(
        "Legacy touched test fixture embeds governed config text; path-scoped until the fixture derives it from the owning source.",
        (
            "scripts/test_merge_queue_operator.py",
        ),
        (
            "[merge_queue_preflight.operator]",
            "[merge_queue_preflight.verifier_profiles.local]",
            "[merge_queue_preflight.verifier_profiles.static]",
            'commands = ["just fmt-check", "just source-fence-static", "just ci-lint-workflow"]',
            'commands = ["just source-fence-static"]',
            'default_verifier_profile = "static"',
            'queue_command = "@mergifyio queue"',
        ),
    ),
    *registered_payload_group(
        "Legacy touched test fixture embeds governed config text; path-scoped until the fixture derives it from the owning source.",
        (
            "scripts/test_merge_queue_operator.py",
            "scripts/test_merge_queue_preflight.py",
        ),
        (
            "[merge_queue_preflight.timeouts]",
            "[merge_queue_preflight]",
            'base = "main"',
            'origin = "origin"',
        ),
    ),
    *registered_payload_group(
        "Legacy touched test fixture embeds governed config text; path-scoped until the fixture derives it from the owning source.",
        (
            "scripts/test_merge_queue_preflight.py",
        ),
        (
            '"actionlint" = "actionlint"',
            '"backtester-gate" = "Backtester CI"',
            '"gate" = "CI"',
            '"host-health" = "CI"',
            '"just source-fence-static" = "just source-fence-static-fences-only"',
            "[fail_closed_contracts]",
            "[fail_closed_exceptions]",
            "[merge_queue_preflight.output]",
            "[merge_queue_preflight.required_check_workflows]",
            "[merge_queue_preflight.source_check_aliases]",
            "[merge_queue_preflight.source_fence_fences_only_rewrites]",
            "commands = []",
            "host-health",
            "source_fence_full_profile_pathspecs = [",
        ),
    ),
    *registered_payload_group(
        "Legacy touched test fixture embeds governed config text; path-scoped until the fixture derives it from the owning source.",
        (
            "scripts/test_merge_queue_preflight.py",
            "scripts/test_verify_ci_workflow_hygiene.py",
        ),
        (
            "[local_lane_policy]",
            "hotfix",
            "label = hotfix",
        ),
    ),
    *registered_payload_group(
        "Legacy touched test fixture embeds governed config text; path-scoped until the fixture derives it from the owning source.",
        (
            "scripts/test_verify_ci_workflow_hygiene.py",
        ),
        (
            "      - check-success = gate",
            "      max: 6",
            "    batch_size:",
            "    batch_size: 1",
            "    conditions:",
            "    queue_conditions:",
            "    queue_conditions: []",
            '  ".github/actions/setup-environment/**",',
            '  ".github/workflows/backtester-ci.yml",',
            '  ".gitignore",',
            '  "backtester_ci",',
            '  "ci/rust-ci-inputs.toml",',
            '  "gated_source_roots.manifest",',
            '  "scripts/ci_input_sets.py",',
            '  "scripts/rust_test_targets.py",',
            '  "specs/023-nt-research-analytics-platform/reference/**",',
            "  - name: default",
            "30 seconds",
            "[artifact_retention.classes.reuse-bound]",
            "[artifact_retention.classes.transient]",
            "[artifact_retention.lookback_bindings.build_deploy]",
            '[artifact_retention.uploads.".github/workflows/ci.yml::build::upload-bolt-v2-binary"]',
            "[ci_provenance.dispatch]",
            "[ci_provenance.full_ci.jobs.test]",
            "[ci_provenance.mergify]",
            "[local_compile_policy]",
            "[meter.api_limits]",
            "[remote_verification]",
            "[sets.backtester_detect]",
            "[storage_audit.cleanup_feasibility_alert]",
            "acquire_timeout_seconds = 1800",
            'allowed_ci_env = "GITHUB_ACTIONS"',
            'artifact_class = "capture-provenance"',
            'artifact_class = "deployable"',
            'artifact_class = "reuse-bound"',
            "artifact_lookback_age_seconds = 259200",
            'artifact_name_config_file = "ci/github-actions-runners.toml"',
            'artifact_name_config_ref = "ci_provenance.deploy.artifact_name"',
            'artifact_name_template = "ci-provenance-attempt-{run_attempt}"',
            'artifact_name_template_config_file = "ci/github-actions-runners.toml"',
            'artifact_name_template_config_ref = "ci_provenance.artifact_name_template"',
            'artifact_name_template_vars_config_ref = "backtester.issue_789.artifact_name_template_vars"',
            'artifact_name_template_vars_config_ref = "ci_provenance.artifact_name_template_vars"',
            "artifact_retention_days = 3",
            'artifact_upload_if = "${{ github.event_name == \'push\' && github.ref == \'refs/heads/main\' }}"',
            'backtester_iteration = "backtester-gate-iteration"',
            "branch_pull_requests_per_page = 20",
            'break_glass_env = "BOLT_ALLOW_LOCAL_RUST"',
            'check_name = "test"',
            "checks_appear_timeout_seconds = 300",
            "commands:",
            'config_file = "ci/github-actions-runners.toml"',
            'converted_to_draft = "iteration"',
            "defer",
            "diagnostic_log_max_bytes = 20000",
            "diagnostic_log_max_lines = 160",
            "diagnostic_unavailable_notice_interval_polls = 4",
            'draft_pr_synchronize = "iteration"',
            "draft_timeline_items = 100",
            "enabled = true",
            'fingerprint_artifact_prefix = "nextest-archive-fingerprint-"',
            'fingerprint_source = "meter"',
            "force_full_ci = false",
            'gate_iteration = "gate-iteration"',
            'gate_required = "gate"',
            "heartbeat_seconds = 15",
            "ignore_emit_failure = false",
            'lock_dir = "/tmp/rust-verification-lanes"',
            'lookback_ref = "ci_provenance.deploy.artifact_lookback_age_seconds"',
            'main_push = "full"',
            "max_lookback_age_seconds = 1209600",
            "max_retention_days = 14",
            "max_retention_days = 7",
            "merge",
            'merge_group = "full"',
            "merge_queue:",
            'mergify_temp_pr = "full"',
            'nextest-fingerprint = "managed_heavy"',
            "noop",
            "overall_timeout_seconds = 3600",
            "poll_interval_seconds = 1",
            "poll_interval_seconds = 15",
            "priority_rules:",
            'project_id = "bolt-v2"',
            'proof_gate_job = "gate"',
            'ready_for_review = "full"',
            'ready_pr = "full"',
            'ready_pr_edited_no_base = "noop"',
            'ready_pr_reopened = "noop"',
            'refused_cargo_subcommands = ["b", "bench", "build", "c", "check", "clippy", "d", "doc", "fetch", "install", "nextest", "r", "run", "rustc", "t", "test", "zigbuild"]',
            'refused_managed_commands = ["test", "clippy", "build"]',
            'required_if_config_file = "ci/github-actions-runners.toml"',
            'required_if_config_ref = "ci_provenance.deploy.artifact_upload_if"',
            "retention_days = 14",
            'retention_days_config_file = "ci/chainlink-reference-fixture-capture-provenance.toml"',
            'retention_days_config_ref = "ci_provenance.artifacts.retention_days"',
            'retention_ref = "ci_provenance.deploy.artifact_retention_days"',
            "run_artifacts_per_page = 100",
            'run_attempt = "${{ github.run_attempt }}"',
            "run_jobs_per_page = 100",
            "schema_version = 1",
            "schema_version = 2",
            "sp-reviewer",
            "squash",
            'target_namespace = "bolt-v2"',
            'unknown_event = "full"',
            'upload = ".github/workflows/ci.yml::build::upload-bolt-v2-binary"',
            'workflow_dispatch = "iteration"',
            'workflow_dispatch_full_ci = "full"',
            'workflow_name = "CI"',
            "workflow_runs_per_page = 100",
        ),
    ),
    *registered_payload_group(
        "Legacy touched verifier literal predates this fence; path-scoped so new files cannot reuse it silently.",
        (
            "scripts/test_merge_queue_preflight.py",
            "scripts/test_verify_ci_workflow_hygiene.py",
            "scripts/verify_ci_workflow_hygiene.py",
        ),
        (
            "backtester-gate",
            "gate",
            "tag",
        ),
    ),
    RegisteredPayload(
        "default",
        "Legacy queue-rule fixture value and Python AST keyword-name check collide with a governed table value; path-scoped until callers derive it from source tables or grammar helpers.",
        (
            "scripts/test_merge_queue_preflight.py",
            "scripts/test_verify_ci_workflow_hygiene.py",
            "scripts/verify_ci_workflow_hygiene.py",
            "scripts/verify_fail_closed_contracts.py",
        ),
    ),
    *registered_payload_group(
        "Legacy touched verifier literal predates this fence; path-scoped so new files cannot reuse it silently.",
        (
            "scripts/test_merge_queue_preflight.py",
            "scripts/verify_ci_workflow_hygiene.py",
        ),
        (
            "actionlint",
        ),
    ),
    *registered_payload_group(
        "Legacy touched verifier literal predates this fence; path-scoped so new files cannot reuse it silently.",
        (
            "scripts/test_verify_ci_workflow_hygiene.py",
            "scripts/verify_ci_workflow_hygiene.py",
        ),
        (
            "converted_to_draft",
            "docs",
            "draft_pr_edited",
            "draft_pr_opened",
            "draft_pr_reopened",
            "draft_pr_synchronize",
            "full",
            "iteration",
            "main_push",
            "merge_group",
            "mergify_temp_pr",
            "ready_for_review",
            "ready_pr",
            "ready_pr_edited_no_base",
            "ready_pr_reopened",
            "tag_reuse",
            "unknown_event",
            "workflow_dispatch",
        ),
    ),
    *registered_payload_group(
        "Mergify shape-hardening malformed payload; the violation fragment intentionally has no source table.",
        (
            "scripts/test_verify_ci_workflow_hygiene.py",
        ),
        (
            '    "queue_conditions":',
            "    <<: *default_queue",
            "    name: sneaky",
            "    queue_conditions: *default_conditions",
            "    skip_intermediate_results: true",
            '  - "name": sneaky',
            "---",
            ": value",
            "? [not, scalar]",
            "extra: true",
        ),
    ),
)


@dataclasses.dataclass(frozen=True)
class ProtectedString:
    value: str
    source: str


@dataclasses.dataclass(frozen=True)
class LiteralHit:
    path: str
    line: int
    payload: str
    source: str
    strict: bool


def repo_relative(root: pathlib.Path, path: pathlib.Path) -> str:
    return path.relative_to(root).as_posix()


def flatten_string_values(value: Any) -> Iterator[str]:
    if isinstance(value, str):
        yield value
        return
    if isinstance(value, dict):
        for nested in value.values():
            yield from flatten_string_values(nested)
        return
    if isinstance(value, (tuple, list, set, frozenset)):
        for nested in value:
            yield from flatten_string_values(nested)
        return
    if value is None:
        return
    if isinstance(value, (bool, int, float)):
        # Numeric and boolean leaves are deliberately ignored; this fence protects retyped text.
        return
    raise TypeError(f"unsupported single-source table value type {type(value).__name__}")


def single_source_table_values() -> tuple[ProtectedString, ...]:
    import ci_provenance

    tables = (
        ("ci_provenance.MERGIFY_CONFIG_EXPECTATIONS", ci_provenance.MERGIFY_CONFIG_EXPECTATIONS),
        ("ci_provenance.POLICY_ROWS", ci_provenance.POLICY_ROWS),
        ("ci_provenance.POLICY_VALUES", ci_provenance.POLICY_VALUES),
        ("ci_provenance.POLICY_REQUIRED_VALUES", ci_provenance.POLICY_REQUIRED_VALUES),
        ("ci_provenance.POLICY_ALLOWED_VALUES", ci_provenance.POLICY_ALLOWED_VALUES),
    )
    values: list[ProtectedString] = []
    seen: set[str] = set()
    for source, table in tables:
        for value in flatten_string_values(table):
            if value and value not in seen:
                values.append(ProtectedString(value=value, source=source))
                seen.add(value)
    return tuple(values)


CONFIG_LINE_NAME_RE = re.compile(r"^[A-Za-z0-9_.\"'/-]+$")
CONFIG_LINE_KEY_RE = re.compile(r"^-?\s*[\"']?[A-Za-z0-9_.-]+[\"']?\s*:")


def config_line_has_information(raw_line: str) -> bool:
    line = raw_line.strip()
    if not line or line.startswith("#"):
        return False
    if not any(char.isalnum() for char in line):
        return False
    if "=" in line:
        left, _right = line.split("=", maxsplit=1)
        return any(char.isalnum() for char in left)
    if line.startswith("[") and line.endswith("]"):
        return any(char.isalnum() for char in line.strip("[]"))
    if CONFIG_LINE_KEY_RE.match(line):
        return True
    list_value = line.removeprefix("-").strip().rstrip(",").strip("\"'")
    return len(list_value) >= 3 and CONFIG_LINE_NAME_RE.match(list_value) is not None


def config_line_values(
    root: pathlib.Path,
    artifact_paths: tuple[str, ...] = GOVERNED_CONFIG_ARTIFACTS,
) -> tuple[ProtectedString, ...]:
    values: list[ProtectedString] = []
    seen: set[str] = set()

    def add_value(value: str, source: str) -> None:
        if value not in seen:
            values.append(ProtectedString(value=value, source=source))
            seen.add(value)

    for rel_path in artifact_paths:
        path = root / rel_path
        if not path.is_file():
            raise ValueError(f"declared governed config artifact missing: {rel_path}")
        for raw_line in path.read_text(encoding="utf-8").splitlines():
            if not config_line_has_information(raw_line):
                continue
            add_value(raw_line, rel_path)
            stripped_line = raw_line.strip()
            if stripped_line != raw_line:
                add_value(stripped_line, rel_path)
    return tuple(values)


def protected_strings(root: pathlib.Path) -> tuple[ProtectedString, ...]:
    values: list[ProtectedString] = []
    seen: set[str] = set()
    for protected in (*config_line_values(root), *single_source_table_values()):
        if protected.value not in seen:
            values.append(protected)
            seen.add(protected.value)
    return tuple(values)


def registered_payloads(
    entries: tuple[RegisteredPayload, ...] = REGISTERED_VIOLATION_PAYLOADS,
) -> dict[str, RegisteredPayload]:
    payloads: dict[str, RegisteredPayload] = {}
    for entry in entries:
        if not entry.payload:
            raise ValueError("registered payload must be nonempty")
        if not entry.reason.strip():
            raise ValueError(f"registered payload {entry.payload!r} must include a reason")
        if entry.payload in payloads:
            raise ValueError(f"duplicate registered payload {entry.payload!r}")
        payloads[entry.payload] = entry
    return payloads


def registered_payload_allowed(
    rel_path: str,
    payload: str,
    registered: dict[str, RegisteredPayload],
) -> bool:
    entry = registered.get(payload)
    if entry is None:
        return False
    if rel_path == "scripts/verify_no_config_retype.py":
        return True
    return not entry.paths or rel_path in entry.paths


def is_source_table_owner(rel_path: str, protected: ProtectedString) -> bool:
    return rel_path == "scripts/ci_provenance.py" and protected.source.startswith("ci_provenance.")


def literal_strings(path: pathlib.Path) -> Iterator[tuple[int, str]]:
    try:
        tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    except SyntaxError as exc:
        raise ValueError(f"{path}: cannot parse Python source: {exc}") from exc
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            yield node.lineno, node.value


def script_paths(scripts_dir: pathlib.Path) -> tuple[pathlib.Path, ...]:
    return tuple(sorted(scripts_dir.rglob("*.py")))


def scan_literals(
    *,
    root: pathlib.Path = REPO_ROOT,
    scripts_dir: pathlib.Path = SCRIPTS_DIR,
    strict_paths: frozenset[str] = STRICT_SCRIPT_PATHS,
    registered: dict[str, RegisteredPayload] | None = None,
    protected: tuple[ProtectedString, ...] | None = None,
) -> tuple[LiteralHit, ...]:
    registered_values = registered_payloads() if registered is None else registered
    protected_values = protected_strings(root) if protected is None else protected
    protected_by_value = {entry.value: entry for entry in protected_values}
    hits: list[LiteralHit] = []
    for path in script_paths(scripts_dir):
        rel_path = repo_relative(root, path)
        strict = rel_path in strict_paths
        for line_number, literal in literal_strings(path):
            candidates = dict.fromkeys((literal, *literal.splitlines()))
            for candidate in candidates:
                if registered_payload_allowed(rel_path, candidate, registered_values):
                    continue
                protected_entry = protected_by_value.get(candidate)
                if protected_entry is None:
                    continue
                if is_source_table_owner(rel_path, protected_entry):
                    continue
                hits.append(
                    LiteralHit(
                        path=rel_path,
                        line=line_number,
                        payload=candidate,
                        source=protected_entry.source,
                        strict=strict,
                    )
                )
    return tuple(hits)


def evaluate_hits(
    hits: tuple[LiteralHit, ...],
    *,
    ratchet_baseline: int = RATCHET_BASELINE,
) -> tuple[list[str], int]:
    errors: list[str] = []
    strict_hits = [hit for hit in hits if hit.strict]
    ratchet_hits = [hit for hit in hits if not hit.strict]
    for hit in strict_hits:
        errors.append(
            f"{hit.path}:{hit.line}: retypes protected config payload {hit.payload!r} from {hit.source}"
        )
    if len(ratchet_hits) > ratchet_baseline:
        errors.append(
            f"ratchet retype count increased: {len(ratchet_hits)} current > {ratchet_baseline} baseline"
        )
        for hit in ratchet_hits[:20]:
            errors.append(
                f"{hit.path}:{hit.line}: ratchet payload {hit.payload!r} from {hit.source}"
            )
    return errors, len(ratchet_hits)


def verify(
    *,
    root: pathlib.Path = REPO_ROOT,
    scripts_dir: pathlib.Path = SCRIPTS_DIR,
    strict_paths: frozenset[str] = STRICT_SCRIPT_PATHS,
    ratchet_baseline: int = RATCHET_BASELINE,
) -> tuple[list[str], int]:
    try:
        hits = scan_literals(root=root, scripts_dir=scripts_dir, strict_paths=strict_paths)
    except (OSError, TypeError, ValueError) as exc:
        return [str(exc)], 0
    return evaluate_hits(hits, ratchet_baseline=ratchet_baseline)


def main() -> int:
    errors, ratchet_count = verify()
    if errors:
        for error in errors:
            print(f"FAIL: {error}", file=sys.stderr)
        return 1
    print(
        "OK: no strict config retypes and ratchet count "
        f"{ratchet_count} <= baseline {RATCHET_BASELINE}."
    )
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
