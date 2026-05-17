# CI Workflow Hygiene Quickstart

## Local Red/Green Checks

From the current workflow-hygiene worktree:

```bash
python3 scripts/test_verify_ci_workflow_hygiene.py
python3 scripts/verify_ci_workflow_hygiene.py
just ci-lint-workflow
```

Expected result after implementation: all commands pass.

## Workflow Sanity Checks

```bash
rg -n "fmt-check:|needs:|include-managed-target-dir|deploy:|check-aarch64|source-fence|test-shards|taiki-e/install-action|fallback: none|cargo-zigbuild-x86_64-unknown-linux-gnu|needs\\.(detector|fmt-check|deny|clippy|check-aarch64|source-fence|test|build)\\.result" .github/workflows/ci.yml
rg -n "advisories:|taiki-e/install-action|fallback: none|cargo-deny" .github/workflows/advisory.yml
rg -n "cargo(\\s+\\+[A-Za-z0-9_.-]+)?\\s+.*\\binstall\\b.*(cargo-(deny|nextest|zigbuild)|cargo-(deny|nextest|zigbuild)@|--git|--path)|sudo\\s+.*cargo\\s+.*\\binstall\\b|env\\s+.*cargo\\s+.*\\binstall\\b" .github/workflows/ci.yml .github/workflows/advisory.yml
```

Expected evidence:

- `fmt-check` has no `needs: detector`.
- `build` still has `needs: detector` and `if: needs.detector.outputs.build_required == 'true'`.
- Jobs using `steps.setup.outputs.managed_target_dir` set `include-managed-target-dir: "true"`.
- `deploy.needs` includes all required safety lanes directly.
- `gate` checks all required lane results.
- `cargo-deny` and `cargo-nextest` use pinned `taiki-e/install-action` with `fallback: none`.
- `advisory.yml` uses the same pinned `cargo-deny` prebuilt install path with `fallback: none`.
- `cargo-zigbuild` installs from the pinned-version release archive and verifies the archive checksum before extraction.
- The raw source-build scan returns no workflow matches for direct, toolchain-prefixed, crate-version, `--git`, `--path`, sudo-prefixed, or env-prefixed `cargo install` forms.

## Verification Gate

```bash
just fmt-check
just ci-lint-workflow
git diff --check
```

Exact-head CI must pass before external review:

- `detector`
- `fmt-check`
- `deny`
- `clippy`
- `check-aarch64`
- `source-fence`
- `nextest shard 1 of 4`
- `nextest shard 2 of 4`
- `nextest shard 3 of 4`
- `nextest shard 4 of 4`
- `test`
- `build`
- `gate`
