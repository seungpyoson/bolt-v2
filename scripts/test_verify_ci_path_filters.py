#!/usr/bin/env python3
"""Self-tests for CI path-filter docs and pass-stub verifier."""

from __future__ import annotations

import importlib.util
import pathlib
import sys
import tempfile


REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "scripts" / "verify_ci_path_filters.py"


CI_FIXTURE = """
name: CI
on:
  pull_request:
    branches: [main]
    paths-ignore:
      - 'AGENTS.md'
      - 'CLAUDE.md'
      - 'GEMINI.md'
      - 'REASONIX.md'
      - 'LICENSE'
      - 'SECURITY.md'
      - '.github/ISSUE_TEMPLATE/**'
      - '.claude/**'
      - '.codex/**'
      - '.gemini/**'
      - '.opencode/**'
      - '.pi/**'
      - '.specify/**'
  push:
    branches: [main]
"""


PASS_STUB_HEADER = """
name: CI docs pass stub
on:
  pull_request:
    branches: [main]
    paths:
      - 'AGENTS.md'
      - 'CLAUDE.md'
      - 'GEMINI.md'
      - 'REASONIX.md'
      - 'LICENSE'
      - 'SECURITY.md'
      - '.github/ISSUE_TEMPLATE/**'
      - '.claude/**'
      - '.codex/**'
      - '.gemini/**'
      - '.opencode/**'
      - '.pi/**'
      - '.specify/**'
permissions:
  contents: read
jobs:
"""


PASS_STUB_JOB_TEMPLATE = """
  {job}:
    name: {job}
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@example
      - name: Classify changed files
        run: python3 scripts/verify_ci_path_filters.py --changed-files changed-files.txt --github-output "$GITHUB_OUTPUT"
"""


PASS_STUB_FIXTURE = PASS_STUB_HEADER + "".join(PASS_STUB_JOB_TEMPLATE.format(job=job) for job in ("build", "clippy", "test", "gate"))


PASS_STUB_GATE_ONLY_FIXTURE = PASS_STUB_HEADER + """
  gate:
    name: gate
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@example
      - name: Classify changed files
        run: python3 scripts/verify_ci_path_filters.py --changed-files changed-files.txt --github-output "$GITHUB_OUTPUT"
"""


DOCS_FIXTURE = """
| Scenario | Example path | Classification | CI behavior |
| --- | --- | --- | --- |
| docs-only root agent doc | `AGENTS.md` | ignored-safe | full CI skipped; pass-stub `build`, `clippy`, `test`, and `gate` run and succeed |
| root security policy | `SECURITY.md` | ignored-safe | full CI skipped; pass-stub `build`, `clippy`, `test`, and `gate` run and succeed |
| workflow change | `.github/workflows/ci.yml` | full-ci | full CI runs; pass-stub does not trigger |
| Rust source change | `src/lib.rs` | full-ci | full CI runs; pass-stub does not trigger |
| managed rust-verification config | `ci/rust-verification.toml` | full-ci | full CI runs; pass-stub does not trigger |
| forbidden legacy rust-verification config | `.claude/rust-verification.toml` | invalid | pass-stub classifier fails closed |
| lockfile change | `Cargo.lock` | full-ci | full CI runs; pass-stub does not trigger |
| mixed docs and source | `AGENTS.md` + `src/lib.rs` | full-ci | full CI runs; pass-stub records `docs_only=false` without blocking |
| ignored Claude agent dir | `.claude/skills/speckit-plan/SKILL.md` | ignored-safe | full CI skipped; pass-stub `build`, `clippy`, `test`, and `gate` run and succeed |
| ignored config dir | `.codex/config.toml` | ignored-safe | full CI skipped; pass-stub `build`, `clippy`, `test`, and `gate` run and succeed |
"""


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


def assert_raises(fragment: str, func) -> None:
    try:
        func()
    except Exception as exc:  # noqa: BLE001 - verifier exposes domain errors.
        if fragment not in str(exc):
            raise AssertionError(f"expected error containing {fragment!r}, got: {exc}") from exc
        return
    raise AssertionError(f"expected error containing {fragment!r}")


def assert_extracts_ci_paths_ignore() -> None:
    module = load_script()
    paths = module.extract_ci_paths_ignore(CI_FIXTURE)
    expected = (
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
    )
    if tuple(paths) != expected:
        raise AssertionError(paths)


def assert_classifies_changed_paths() -> None:
    module = load_script()
    safe = module.extract_ci_paths_ignore(CI_FIXTURE)
    cases = {
        ("AGENTS.md",): True,
        ("SECURITY.md",): True,
        (".codex/settings.json", ".specify/init-options.json"): True,
        (".claude/skills/speckit-plan/SKILL.md",): True,
        (".github/ISSUE_TEMPLATE/bug.yml",): True,
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
        actual = module.docs_only_safe(changed, safe)
        if actual != expected:
            raise AssertionError((changed, actual, expected))
    assert_raises(
        "forbidden ignored build path",
        lambda: module.docs_only_safe((".claude/rust-verification.toml",), safe),
    )
    assert_raises(
        "forbidden ignored build path",
        lambda: module.docs_only_safe(("./.claude/rust-verification.toml",), safe),
    )
    assert_raises("changed file list is empty", lambda: module.docs_only_safe((), safe))


def assert_verifies_pass_stub_workflow() -> None:
    module = load_script()
    module.verify_pass_stub_workflow(PASS_STUB_FIXTURE)
    step_if_fixture = PASS_STUB_FIXTURE.replace(
        "      - name: Classify changed files",
        "      - name: Optional diagnostic\n        if: always()\n        run: echo ok\n      - name: Classify changed files",
    )
    module.verify_pass_stub_workflow(step_if_fixture)
    assert_raises("pass-stub required stub job gate must be named gate", lambda: module.verify_pass_stub_workflow(PASS_STUB_FIXTURE.replace("name: gate", "name: docs-gate")))
    assert_raises("pass-stub required stub job build must run changed-file classifier", lambda: module.verify_pass_stub_workflow(PASS_STUB_FIXTURE.replace("python3 scripts/verify_ci_path_filters.py", "echo ok", 1)))
    require_docs_only_fixture = PASS_STUB_FIXTURE.replace("$GITHUB_OUTPUT", "$GITHUB_OUTPUT\" --require-docs-only")
    assert_raises("pass-stub must not require docs-only", lambda: module.verify_pass_stub_workflow(require_docs_only_fixture))
    assert_raises("pass-stub build job must fail directly", lambda: module.verify_pass_stub_workflow(PASS_STUB_FIXTURE.replace("runs-on: ubuntu-latest", "needs: classify-docs-only\n    runs-on: ubuntu-latest", 1)))
    job_if_fixture = PASS_STUB_FIXTURE.replace("    runs-on: ubuntu-latest", "    if: always()\n    runs-on: ubuntu-latest", 1)
    assert_raises("pass-stub build job must not use job-level if", lambda: module.verify_pass_stub_workflow(job_if_fixture))


def assert_rejects_missing_required_pass_stub_contexts() -> None:
    module = load_script()
    assert_raises("pass-stub workflow missing required stub job build", lambda: module.verify_pass_stub_workflow(PASS_STUB_GATE_ONLY_FIXTURE))


def assert_verifies_docs_rows() -> None:
    module = load_script()
    module.verify_docs_table(DOCS_FIXTURE)
    assert_raises("docs missing required scenario", lambda: module.verify_docs_table(DOCS_FIXTURE.replace("mixed docs and source", "mixed row removed")))


def assert_writes_github_output() -> None:
    module = load_script()
    with tempfile.TemporaryDirectory() as tmpdir:
        output = pathlib.Path(tmpdir) / "github-output"
        changed = pathlib.Path(tmpdir) / "changed.txt"
        changed.write_text("AGENTS.md\n.codex/config.toml\n", encoding="utf-8")
        module.classify_changed_file_path(changed, output, verbose=False)
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
        changed.write_text("AGENTS.md\nsrc/lib.rs\n", encoding="utf-8")
        assert_raises(
            "changed files are not docs-only ignored-safe",
            lambda: module.classify_changed_file_path(changed, output, require_docs_only=True, verbose=False),
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
    assert_extracts_ci_paths_ignore()
    assert_classifies_changed_paths()
    assert_rejects_missing_required_pass_stub_contexts()
    assert_verifies_pass_stub_workflow()
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
