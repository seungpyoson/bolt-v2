#!/usr/bin/env python3
"""Self-tests for CI path-filter docs and policy verifier."""

from __future__ import annotations

import importlib.util
import pathlib
import sys
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "verify_ci_path_filters.py"
SPECKIT_CONTEXT_SCRIPT_PATH = REPO_ROOT / "scripts" / "verify_speckit_context.py"


CI_FIXTURE = """
name: CI
on:
  pull_request:
    branches: [main]
  push:
    branches: [main]
"""


CONFIG_FIXTURE = """
schema_version = 1

[meter]
fingerprint_artifact_prefix = "nextest-archive-fingerprint-"
fingerprint_workflow = "ci"

[ci_provenance]
schema_version = 1
artifact_name_template = "ci-provenance-attempt-{run_attempt}"
workflow_key = "ci"
workflow_name = "CI"
workflow_path = ".github/workflows/ci.yml"
fingerprint_source = "meter"

[ci_provenance.docs]
safe_paths = [
  "AGENTS.md",
  "CLAUDE.md",
  "GEMINI.md",
  "REASONIX.md",
  "LICENSE",
  "SECURITY.md",
  ".github/ISSUE_TEMPLATE/**",
  ".claude/**",
  ".codex/**",
  ".gemini/**",
  ".opencode/**",
  ".pi/**",
  ".specify/**",
]
forbidden_ignored_build_paths = [
  ".claude/rust-verification.toml",
]
non_heavy_required_jobs = ["detector"]
"""


DOCS_FIXTURE = """
| Scenario | Example path | Classification | CI behavior |
| --- | --- | --- | --- |
| docs-only root agent doc | `AGENTS.md` | docs | SpecKit context verifier runs in actionlint; heavy lanes skipped; `gate` records docs proof |
| root security policy | `SECURITY.md` | docs | heavy lanes skipped; `gate` records docs proof |
| workflow change | `.github/workflows/ci.yml` | full-ci | full CI runs |
| Rust source change | `src/lib.rs` | full-ci | full CI runs |
| managed rust-verification config | `ci/rust-verification.toml` | full-ci | full CI runs |
| forbidden legacy rust-verification config | `.claude/rust-verification.toml` | full-ci | full CI runs |
| feature registry input | `.specify/feature.json` | docs | SpecKit context verifier runs in actionlint; heavy lanes skipped; `gate` records docs proof |
| lockfile change | `Cargo.lock` | full-ci | full CI runs |
| mixed docs and source | `AGENTS.md` + `src/lib.rs` | full-ci | full CI runs |
| ignored Claude agent dir | `.claude/skills/speckit-plan/SKILL.md` | docs | heavy lanes skipped; `gate` records docs proof |
| ignored config dir | `.codex/config.toml` | docs | heavy lanes skipped; `gate` records docs proof |
"""

STALE_AGENTS_FIXTURE = """
<!-- SPECKIT START -->
For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan:
`specs/026-nt-backed-iv-engine/plan.md`
`specs/023-nt-research-analytics-platform/plan.md`
<!-- SPECKIT END -->
"""
CURRENT_FEATURE_JSON_FIXTURE = '{"feature_directory": "specs/026-nt-backed-iv-engine"}'


def load_script():
    if not SCRIPT_PATH.exists():
        raise AssertionError(f"missing script: {SCRIPT_PATH}")
    spec = importlib.util.spec_from_file_location("verify_ci_path_filters", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise AssertionError("could not load verify_ci_path_filters.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def load_speckit_context_script():
    if not SPECKIT_CONTEXT_SCRIPT_PATH.exists():
        raise AssertionError(f"missing script: {SPECKIT_CONTEXT_SCRIPT_PATH}")
    spec = importlib.util.spec_from_file_location(
        "verify_speckit_context",
        SPECKIT_CONTEXT_SCRIPT_PATH,
    )
    if spec is None or spec.loader is None:
        raise AssertionError("could not load verify_speckit_context.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def assert_raises(fragment: str, func) -> None:
    try:
        func()
    except Exception as exc:  # noqa: BLE001 - verifier exposes domain errors.
        if fragment not in str(exc):
            raise AssertionError(f"expected error containing {fragment!r}, got: {exc}") from exc
        return
    raise AssertionError(f"expected error containing {fragment!r}")


def write_config(tmpdir: pathlib.Path, text: str = CONFIG_FIXTURE) -> pathlib.Path:
    path = tmpdir / "github-actions-runners.toml"
    path.write_text(text, encoding="utf-8")
    return path


def assert_loads_registry_safe_paths() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        tmp_path = pathlib.Path(tmpdir)
        config = write_config(tmp_path)
        registry = module.load_docs_path_registry(config)
        if registry.safe_paths[:2] != ("AGENTS.md", "CLAUDE.md"):
            raise AssertionError(registry)
        if "docs/**" in registry.safe_paths or "specs/**" in registry.safe_paths:
            raise AssertionError(f"build-input docs/spec paths must not be safe: {registry}")
        if ".claude/rust-verification.toml" not in registry.forbidden_ignored_build_paths:
            raise AssertionError(registry)
        if ".specify/feature.json" in registry.forbidden_ignored_build_paths:
            raise AssertionError(registry)
        assert_raises(
            "ci_provenance.docs.safe_paths must not include build-input path docs/**",
            lambda: module.load_docs_path_registry(
                write_config(
                    tmp_path,
                    CONFIG_FIXTURE.replace('  ".specify/**",', '  ".specify/**",\n  "docs/**",'),
                )
            ),
        )


def assert_classifies_changed_paths() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        registry = module.load_docs_path_registry(write_config(pathlib.Path(tmpdir)))
    safe = registry.safe_paths
    forbidden = registry.forbidden_ignored_build_paths
    cases = {
        ("AGENTS.md",): True,
        ("SECURITY.md",): True,
        (".codex/settings.json", ".specify/notes.md"): True,
        (".claude/skills/speckit-plan/SKILL.md",): True,
        (".github/ISSUE_TEMPLATE/bug.yml",): True,
        (".specify/feature.json",): True,
        ("./.specify/feature.json",): True,
        (".claude/rust-verification.toml",): False,
        ("./.claude/rust-verification.toml",): False,
        ("src/lib.rs",): False,
        (".github/workflows/ci.yml",): False,
        ("ci/rust-verification.toml",): False,
        ("Cargo.lock",): False,
        ("AGENTS.md", "src/lib.rs"): False,
        ("SECURITY.md", "src/lib.rs"): False,
        ("docs/ci/paths-ignore-behavior.md",): False,
        ("specs/009-ci-residual-minute-work/spec.md",): False,
        (".codex_malicious/config.toml",): False,
        (".codex-backup/config.toml",): False,
        (".github/ISSUE_TEMPLATE_BACKUP/bug.yml",): False,
        ("AGENTS.md", ".codex_malicious/config.toml"): False,
    }
    for changed, expected in cases.items():
        actual = module.docs_only_safe(changed, safe, forbidden)
        if actual != expected:
            raise AssertionError((changed, actual, expected))
    assert_raises("changed file list is empty", lambda: module.docs_only_safe((), safe, forbidden))


def assert_forbidden_rust_policy_path_forces_full_ci() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        tmp_path = pathlib.Path(tmpdir)
        output = tmp_path / "github-output"
        changed = tmp_path / "changed.txt"
        config = write_config(tmp_path)
        changed.write_text(".claude/rust-verification.toml\n", encoding="utf-8")
        assert_raises(
            "changed files are not docs-only ignored-safe",
            lambda: module.classify_changed_file_path(
                changed,
                output,
                config_path=config,
                require_docs_only=True,
                verbose=False,
            ),
        )
        text = output.read_text(encoding="utf-8")
    if "docs_only=false" not in text:
        raise AssertionError(text)


def assert_feature_registry_path_stays_docs_only() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        tmp_path = pathlib.Path(tmpdir)
        output = tmp_path / "github-output"
        changed = tmp_path / "changed.txt"
        config = write_config(tmp_path)
        changed.write_text(".specify/feature.json\n", encoding="utf-8")
        module.classify_changed_file_path(
            changed,
            output,
            config_path=config,
            require_docs_only=True,
            verbose=False,
        )
        text = output.read_text(encoding="utf-8")
    if "docs_only=true" not in text:
        raise AssertionError(text)


def assert_verifies_docs_rows() -> None:
    module = load_script()
    module.verify_docs_table(DOCS_FIXTURE)
    assert_raises(
        "docs missing required scenario",
        lambda: module.verify_docs_table(DOCS_FIXTURE.replace("mixed docs and source", "mixed row removed")),
    )
    assert_raises(
        "docs table scenario is not enforced",
        lambda: module.verify_docs_table(
            DOCS_FIXTURE
            + "| future docs-only row | `AGENTS.md` | docs | untracked behavior |\n"
        ),
    )


def assert_docs_only_speckit_context_runs_in_always_on_lane() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        registry = module.load_docs_path_registry(write_config(pathlib.Path(tmpdir)))
    if not module.docs_only_safe(("AGENTS.md",), registry.safe_paths, registry.forbidden_ignored_build_paths):
        raise AssertionError("AGENTS.md-only changes must remain docs-only")

    actionlint_workflow = (REPO_ROOT / ".github" / "workflows" / "actionlint.yml").read_text(
        encoding="utf-8"
    )
    required_fragments = (
        "pull_request:",
        "actions/checkout",
        "python3 scripts/verify_speckit_context.py",
    )
    for fragment in required_fragments:
        if fragment not in actionlint_workflow:
            raise AssertionError(f"actionlint must include always-on SpecKit context check: {fragment}")
    if "full_ci_required" in actionlint_workflow:
        raise AssertionError("actionlint SpecKit context check must not be gated by full_ci_required")

    entrypoint = load_speckit_context_script()
    source_module = sys.modules.get("verify_bolt_v3_schema_current")
    if source_module is None:
        raise AssertionError("verify_speckit_context.py must import verify_bolt_v3_schema_current")
    if entrypoint.validate_speckit_context is not source_module.validate_speckit_context:
        raise AssertionError("SpecKit context entrypoint must call the existing validator")

    with tempfile.TemporaryDirectory() as tmpdir:
        tmp_path = pathlib.Path(tmpdir)
        agents_doc = tmp_path / "AGENTS.md"
        feature_json = tmp_path / "feature.json"
        agents_doc.write_text(STALE_AGENTS_FIXTURE, encoding="utf-8")
        feature_json.write_text(CURRENT_FEATURE_JSON_FIXTURE, encoding="utf-8")
        findings = entrypoint.validate_current_speckit_context(agents_doc, feature_json)
    if not any("AGENTS.md" in finding and "active Speckit block" in finding for finding in findings):
        raise AssertionError(f"expected stale AGENTS.md fixture to be caught, got {findings!r}")


def assert_writes_github_output() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        output = pathlib.Path(tmpdir) / "github-output"
        changed = pathlib.Path(tmpdir) / "changed.txt"
        config = write_config(pathlib.Path(tmpdir))
        changed.write_text("AGENTS.md\n.codex/config.toml\n", encoding="utf-8")
        module.classify_changed_file_path(changed, output, config_path=config, verbose=False)
        text = output.read_text(encoding="utf-8")
    if "docs_only=true" not in text:
        raise AssertionError(text)


def assert_input_reads_are_bounded() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        oversized = pathlib.Path(tmpdir) / "changed.txt"
        oversized.write_text("AGENTS.md\n", encoding="utf-8")
        assert_raises("exceeds size limit", lambda: module.read_changed_files(oversized, limit=1))
        assert_raises("exceeds size limit", lambda: module.read_text_bounded(oversized, "fixture", limit=1))


def assert_require_docs_only_fails_closed() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        output = pathlib.Path(tmpdir) / "github-output"
        changed = pathlib.Path(tmpdir) / "changed.txt"
        config = write_config(pathlib.Path(tmpdir))
        changed.write_text("AGENTS.md\nsrc/lib.rs\n", encoding="utf-8")
        assert_raises(
            "changed files are not docs-only ignored-safe",
            lambda: module.classify_changed_file_path(
                changed,
                output,
                config_path=config,
                require_docs_only=True,
                verbose=False,
            ),
        )
        text = output.read_text(encoding="utf-8")
    if "docs_only=false" not in text:
        raise AssertionError(text)


def assert_verifies_rust_policy_location() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        root = pathlib.Path(tmpdir)
        policy = root / "ci" / "rust-verification.toml"
        legacy = root / ".claude" / "rust-verification.toml"
        policy.parent.mkdir()
        policy.write_text("[commands]\n", encoding="utf-8")
        module.verify_rust_policy_location(policy, legacy)
        legacy.parent.mkdir()
        legacy.write_text("[commands]\n", encoding="utf-8")
        assert_raises("legacy managed rust-verification config must not exist", lambda: module.verify_rust_policy_location(policy, legacy))
        legacy.unlink()
        policy.unlink()
        assert_raises("managed rust-verification config missing", lambda: module.verify_rust_policy_location(policy, legacy))


def main() -> int:
    assert_docs_only_speckit_context_runs_in_always_on_lane()
    assert_loads_registry_safe_paths()
    assert_classifies_changed_paths()
    assert_forbidden_rust_policy_path_forces_full_ci()
    assert_feature_registry_path_stays_docs_only()
    assert_verifies_docs_rows()
    assert_writes_github_output()
    assert_input_reads_are_bounded()
    assert_require_docs_only_fails_closed()
    assert_verifies_rust_policy_location()
    print("OK: CI path-filter verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
