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
      - '.github/ISSUE_TEMPLATE/**'
      - '.codex/**'
      - '.gemini/**'
      - '.opencode/**'
      - '.pi/**'
      - '.specify/**'
  push:
    branches: [main]
"""


PASS_STUB_FIXTURE = """
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
      - '.github/ISSUE_TEMPLATE/**'
      - '.codex/**'
      - '.gemini/**'
      - '.opencode/**'
      - '.pi/**'
      - '.specify/**'
permissions:
  contents: read
jobs:
  gate:
    name: gate
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@example
      - name: Classify changed files
        run: python3 scripts/verify_ci_path_filters.py --changed-files changed-files.txt --github-output "$GITHUB_OUTPUT" --require-docs-only
"""


DOCS_FIXTURE = """
| Scenario | Example path | Classification | CI behavior |
| --- | --- | --- | --- |
| docs-only root agent doc | `AGENTS.md` | ignored-safe | full CI skipped; pass-stub `gate` runs and succeeds |
| workflow change | `.github/workflows/ci.yml` | full-ci | full CI runs; pass-stub does not trigger |
| Rust source change | `src/lib.rs` | full-ci | full CI runs; pass-stub does not trigger |
| managed rust-verification config | `.claude/rust-verification.toml` | full-ci | full CI runs; pass-stub does not trigger |
| lockfile change | `Cargo.lock` | full-ci | full CI runs; pass-stub does not trigger |
| mixed docs and source | `AGENTS.md` + `src/lib.rs` | full-ci | full CI runs; pass-stub triggers and fails closed |
| ignored config dir | `.codex/config.toml` | ignored-safe | full CI skipped; pass-stub `gate` runs and succeeds |
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
        ".github/ISSUE_TEMPLATE/**",
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
        (".codex/settings.json", ".specify/feature.json"): True,
        (".github/ISSUE_TEMPLATE/bug.yml",): True,
        ("src/lib.rs",): False,
        (".github/workflows/ci.yml",): False,
        (".claude/rust-verification.toml",): False,
        ("Cargo.lock",): False,
        ("AGENTS.md", "src/lib.rs"): False,
        ("docs/ci/paths-ignore-behavior.md",): False,
        ("specs/009-ci-residual-minute-work/spec.md",): False,
    }
    for changed, expected in cases.items():
        actual = module.docs_only_safe(changed, safe)
        if actual != expected:
            raise AssertionError((changed, actual, expected))
    assert_raises("changed file list is empty", lambda: module.docs_only_safe((), safe))


def assert_verifies_pass_stub_workflow() -> None:
    module = load_script()
    module.verify_pass_stub_workflow(PASS_STUB_FIXTURE)
    assert_raises("pass-stub gate job must be named gate", lambda: module.verify_pass_stub_workflow(PASS_STUB_FIXTURE.replace("name: gate", "name: docs-gate")))
    assert_raises("pass-stub must run changed-file classifier", lambda: module.verify_pass_stub_workflow(PASS_STUB_FIXTURE.replace("python3 scripts/verify_ci_path_filters.py", "echo ok")))
    assert_raises("pass-stub workflow missing --require-docs-only", lambda: module.verify_pass_stub_workflow(PASS_STUB_FIXTURE.replace(" --require-docs-only", "")))
    assert_raises("pass-stub gate job must fail directly", lambda: module.verify_pass_stub_workflow(PASS_STUB_FIXTURE.replace("runs-on: ubuntu-latest", "needs: classify-docs-only\n    runs-on: ubuntu-latest", 1)))


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


def main() -> int:
    assert_extracts_ci_paths_ignore()
    assert_classifies_changed_paths()
    assert_verifies_pass_stub_workflow()
    assert_verifies_docs_rows()
    assert_writes_github_output()
    assert_require_docs_only_fails_closed()
    print("OK: CI path-filter verifier self-tests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
