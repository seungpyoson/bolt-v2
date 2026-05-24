# Evidence Map: #466 Disk-Governance Verifier Decomposition

## Fresh State

| Fact | Evidence |
|---|---|
| Branch/worktree | `goal/466-command-tokenization-characterization` at `REPO_ROOT_PATH/.worktrees/466-command-tokenization-characterization`; PR #478 is the single active consolidation PR. |
| Base | `origin/main` at `3a444a57cfdcdc31d58cbfe8d22857eb86f8bad9` after PR #470 merge. |
| Issue #466 | GitHub state is `CLOSED` at 2026-05-24T11:21:38Z, but the operator explicitly continued the original /goal scope on PR #478; this file must still track #466 ledger status and final gates before any completion claim. |
| Issue #464 | Closed for PR #465 cargo-scanner slice only; close comment moved remaining work to #466 |
| PR #468 | Merged 2026-05-24T07:06:34Z; delivered only the item-8 static cargo option drift slice and reopened #466 for remaining scope |
| PR #465 | Merged 2026-05-24T03:33:55Z; delivered only shared cargo scanner helpers |
| PR #461 | Merged 2026-05-24T01:10:27Z; delivered Python command AST helper extraction |
| Issue #454 | Closed by PR #461; residual scope moved through #464/#466 |

## Issue And PR Source References

| Entity | Command | Current result |
|---|---|---|
| Issue #466 | `gh issue view 466 --json number,title,state,body,comments,url,closedAt` | Closed 2026-05-24T11:21:38Z. Earlier body/comment history lists the eight decomposition areas; operator continuation keeps this ledger active on PR #478 until final gates are satisfied. |
| Issue #464 | `gh issue view 464 --json number,title,state,closedAt,body,comments,url` | Closed 2026-05-24T03:46:28Z. Close comment states PR #465 completed only the cargo-scanner extraction slice and moved remaining verifier-decomposition work to #466. |
| PR #465 | `gh pr view 465 --json number,title,state,mergedAt,headRefName,baseRefName,commits,files,url,body` | Merged 2026-05-24T03:33:55Z. Files show `scripts/command_understanding.py`, runtime/static verifier clients, tests, and `specs/464-*`; body says PR does not close broader remaining scope. |
| PR #461 | `gh pr view 461 --json number,title,state,mergedAt,headRefName,baseRefName,commits,files,url,body` | Merged 2026-05-24T01:10:27Z. Delivered command-understanding helper extraction for #454 and recorded residual follow-up scope. |
| Issue #454 | `gh issue view 454 --json number,title,state,closedAt,body,comments,url` | Closed 2026-05-24T01:11:55Z. Completion comment says PR #461 delivered #454 and residual decomposition moved to #464. |
| PR #478 | `gh pr view 478 --json number,title,state,isDraft,headRefName,url,body` | Open draft consolidation PR on `goal/466-command-tokenization-characterization`; only open repository PR after operator-directed consolidation. Body states future work goes to #478 only and lists remaining #466 ledger items. |

## Current Baseline

| Command | Result |
|---|---|
| `python3 scripts/test_command_understanding.py` | Pass: `OK: command understanding self-tests passed.` |
| `python3 scripts/test_verify_ci_workflow_hygiene.py` | Pass: `OK: CI workflow hygiene verifier self-tests passed.` |
| `python3 -m scripts.test_command_understanding` | Pass: `OK: command understanding self-tests passed.` |
| `python3 -m py_compile scripts/command_understanding.py scripts/test_command_understanding.py scripts/rust_verification.py scripts/verify_ci_workflow_hygiene.py` | Pass |
| `python3 scripts/test_rust_verification_cache_retention.py` | Pass: `OK: Rust verification cache retention self-tests passed.` |
| `git diff --check` | Pass |
| Strict #466 placeholder scan | Pass: no matches for Spec Kit placeholders or unresolved clarification markers in #466 spec/plan/tasks/evidence/research/data-model/quickstart/contract files. |
| PR #468 source-fence on superseded head `112c6e6386937ef4b02b8dccbcb5284f92063665` | Fail: source-fence caught the incorrect active Spec Kit pointer update. This head is superseded by `9771a13d3db6ae64451b04426f0d8439911f4258`. |
| `just source-fence` after restoring active Spec Kit pointers | Pass. Confirms `AGENTS.md` and `.specify/feature.json` remain pinned to `specs/023-nt-order-intent-layer/plan.md`; #466 docs are addressed by explicit path. |
| PR #468 GitHub Actions on pre-Kimi-fix head `9771a13d3db6ae64451b04426f0d8439911f4258` | Pass: `source-fence`, `gate`, `test`, `fmt-check`, `clippy`, `deny`, `check-aarch64`, `nextest archive`, nextest shards 1-4, CodeQL, actionlint, and analyze jobs passed. Workflow run `26351928227`; source-fence job `77571603841`; gate job `77571960933`. |
| Historical PR #468 GitHub Actions on superseded pre-implementation review head `43b9460f077cdff4e8769174f87be887960a1b42` | Pass by `gh pr view 468 --json headRefOid,statusCheckRollup`: CI run `26352284544`; `source-fence` job `77572551567`; `gate` job `77572894337`; `test` job `77572890708`; fmt-check, clippy, deny, check-aarch64, nextest archive, nextest shards 1-4, CodeQL, actionlint, detector, and analyze jobs all succeeded. This is historical pre-implementation evidence, not current merge-gate evidence. |
| Historical PR #468 GitHub Actions on superseded post-doc-cleanup head `9b020b7a363f959afa01a4eb8fd0074eb6614540` | Pass by GitHub workflow runs: actionlint run `26353275834`; CI run `26353275828`; detector, fmt-check, source-fence, deny, check-aarch64, nextest archive, clippy, nextest shards 1-4, test, and gate succeeded. This head was superseded by evidence cleanup after Gemini/Grok flagged stale committed evidence, so it is not current merge-gate evidence. |

## Ledger Final-State Rules

- `open`: item remains active in #466 and cannot support a completion claim.
- `resolved`: item has current-main evidence, chosen resolution, touched-file or intentionally-not-touched list, required tests, review evidence, and final verification proving no unresolved scope remains.
- `blocked`: item has a concrete blocker with command/reviewer/issue evidence and cannot progress without operator input or external state change.
- `operator-moved`: operator explicitly approved moving the scope out of #466, and #466 was updated so the moved item is no longer required for closure.

## Scope Ledger

Completion invariant: before #466 completion, every row must end as `resolved`, `blocked`, or `operator-moved`; final completion still also requires exact-head CI, required external review, resolved comments, operator approval, and approved issue closure.

| # | Ledger item | Current-main runtime implementation evidence | Current-main static verifier implementation evidence | Current-main test/doc evidence | Equivalence verdict | Chosen resolution | Exact files touched or intentionally not touched | Tests required | Review evidence | Final state |
|---:|---|---|---|---|---|---|---|---|---|---|
| 1 | Command tokenization and line-boundary tokenization | `scripts/rust_verification.py:524` uses plain `shlex.split` with `command.split()` fallback; no runtime `command_tokens_with_line_boundaries` peer exists, and `scripts/test_command_understanding.py:535` rejects introducing one without fresh review. | `scripts/verify_ci_workflow_hygiene.py:1235` uses punctuation-aware `shlex.shlex`; `scripts/verify_ci_workflow_hygiene.py:1244` adds logical-line boundary tokenization. | `scripts/test_command_understanding.py:531` through `:578` pins runtime/static tokenization divergence for `&&`, `;`, newline-separated commands, and boundary-continued commands; `scripts/test_command_understanding.py:443` keeps `command_tokens_with_line_boundaries` out of shared exports; #464 evidence records this as residual scope. | Divergent but fully characterized for this slice. | Resolved as keep-local; no shared primitive extracted because runtime process parsing and static workflow scanning intentionally preserve different token and logical-line boundaries. | Touched: `scripts/test_command_understanding.py`, this ledger, and `specs/466-decompose-disk-governance-verifiers/tasks.md`. Intentionally not touched: runtime/static verifier code. | `python3 scripts/test_command_understanding.py`; `python3 -m py_compile scripts/test_command_understanding.py`; `git diff --check`; broader verifier suites before PR gate if requested by Phase 5. | #466 pre-implementation plan review approved or operator-waived; RED/GREEN evidence recorded below. Post-implementation review still required before merge. | Resolved. |
| 2 | Shell command substitution parsing | `scripts/rust_verification.py:659` normalizes tokens before payload scanning; `scripts/rust_verification.py:692` normalizes tokens and requires exact `$` before `(` for `shell_command_substitution_at`. | `scripts/verify_ci_workflow_hygiene.py:1321` scans caller tokens directly; `scripts/verify_ci_workflow_hygiene.py:2192` accepts `$` or tokens ending in `$` without runtime token normalization. | `scripts/test_command_understanding.py:563` through `:612` pins raw `$(`, separated `$ (`, prefix-dollar, process-substitution, inline, backtick, and nested-payload behavior plus `shell_command_substitution_at` divergence; `scripts/test_command_understanding.py:444` through `:445` keeps the helpers out of shared exports. | Divergent but fully characterized for this slice. | Resolved as keep-local; no shared extraction because runtime normalizes process tokens while static scans raw workflow tokens, and `shell_command_substitution_at` intentionally diverges for raw `$(` and prefix-dollar tokens. | Touched: `scripts/test_command_understanding.py`, this ledger, and `specs/466-decompose-disk-governance-verifiers/tasks.md`. Intentionally not touched: runtime/static verifier code. | `python3 scripts/test_command_understanding.py`; `python3 -m scripts.test_command_understanding`; `python3 -m py_compile scripts/test_command_understanding.py`; `git diff --check`; broader verifier suites before PR gate if requested by Phase 5. | #466 pre-implementation plan review approved or operator-waived; RED/GREEN evidence recorded below. Post-implementation review still required before merge. | Resolved. |
| 3 | Renamed `cargo` / `rustc` detection | `scripts/rust_verification.py:1581` through `:1614` treats runtime process paths and symlink resolution as evidence; runtime classifies `rustup` as cargo-like and resolves host symlinks for process paths. Runtime recursive command coverage in `scripts/test_rust_verification_cache_retention.py:2478` through `:2502` and `:2597` through `:2656` covers renamed cargo/rustc launch forms, wrappers, containers, Python payloads, and symlinked renamed cargo. | `scripts/verify_ci_workflow_hygiene.py:2458` through `:2481` inspects raw path tokens and intentionally does not resolve host filesystem symlinks. Static workflow coverage in `scripts/test_verify_ci_workflow_hygiene.py:1989` through `:2022`, `:2650` through `:2667`, and `:2881` through `:2910` covers renamed rustc/cargo raw-output and raw-target findings plus the static no-host-symlink boundary. | `scripts/test_command_understanding.py:532` through `:605` pins runtime/static path-name, slash-path, no-slash, underscore, script-extension, `rustup`, `r`, `mycargo`, `myrustc`, and symlink-resolution behavior; `scripts/test_command_understanding.py:446` through `:449` keeps the classifiers out of shared exports. | Divergent but fully characterized for this slice. | Resolved as keep-local; no shared extraction because runtime resolves real process paths and symlinks while static workflow scanning must remain token-only and host-filesystem independent. | Touched: `scripts/test_command_understanding.py`, this ledger, and `specs/466-decompose-disk-governance-verifiers/tasks.md`. Intentionally not touched: runtime/static verifier code; existing owner-suite coverage was verified rather than rewritten. | `python3 scripts/test_command_understanding.py`; `python3 -m scripts.test_command_understanding`; `python3 scripts/test_rust_verification_cache_retention.py`; `python3 scripts/test_verify_ci_workflow_hygiene.py`; `python3 -m py_compile scripts/test_command_understanding.py`; `git diff --check`; `just ci-lint-workflow` before PR gate. | #466 pre-implementation plan review approved or operator-waived; RED/GREEN evidence recorded below. Post-implementation review still required before merge. | Resolved. |
| 4 | Wrapper handling | `scripts/rust_verification.py:1462` through `:1500` exposes `process_wrapper_tokens` for runtime process recursion and returns one wrapper layer's inner tokens or `None`. Runtime owner coverage in `scripts/test_rust_verification_cache_retention.py:919` through `:1005` and `:2538` through `:2595` proves active-process protection and wrapper option parsing across supported wrappers. | `scripts/verify_ci_workflow_hygiene.py:1922` through `:2000` exposes `wrapper_inner_tokens` in static workflow scanning with static option helper policy and the same one-layer inner-token contract. Static owner coverage in `scripts/test_verify_ci_workflow_hygiene.py:2769` through `:2787`, `:2884` through `:2894`, and `:3123` through `:3128` proves wrapper raw-cargo detection and wrapped cargo-install refusals. | `scripts/test_command_understanding.py:603` through `:635` pins representative runtime/static wrapper outcomes for command, env, sudo, nice, flock, docker/podman-style container, no-mistakes, xargs, and unsupported wrappers; the `env -S "timeout 30 cargo build"` case documents the one-layer, non-recursive helper contract. `scripts/test_command_understanding.py:452` through `:453` keeps wrapper helpers out of shared exports. | Divergent caller policy but characterized for the helper contract. | Resolved as keep-local; no shared extraction because the helpers share a one-layer shape but sit behind different runtime/static wrapper option tables, recursion callers, and fail-open/fail-closed policies. | Touched: `scripts/test_command_understanding.py`, this ledger, and `specs/466-decompose-disk-governance-verifiers/tasks.md`. Intentionally not touched: runtime/static verifier code; existing owner-suite coverage was verified rather than rewritten. | `python3 scripts/test_command_understanding.py`; `python3 -m scripts.test_command_understanding`; `python3 scripts/test_rust_verification_cache_retention.py`; `python3 scripts/test_verify_ci_workflow_hygiene.py`; `python3 -m py_compile scripts/test_command_understanding.py`; `git diff --check`; `just ci-lint-workflow` before PR gate. | #466 pre-implementation plan review approved or operator-waived; RED/GREEN evidence recorded below. Post-implementation review still required before merge. | Resolved. |
| 5 | Target-routing override policy beyond the pure cargo scan helper from PR #465 | Pure scan helper is shared at `scripts/command_understanding.py:170`; runtime policy `scripts/rust_verification.py:2647` returns the offending option/config override string for managed cargo refusal payloads, including value options, `--config` storage overrides, and post-`--` cargo test/bench/run filtering. | Static scan uses the shared pure helper after static command routing at `scripts/verify_ci_workflow_hygiene.py:1695` through `:1712`; policy `scripts/verify_ci_workflow_hygiene.py:1715` through `:1743` returns bool and treats environment prefixes plus managed `scripts/rust_verification.py cargo --repo ... --` launches as target-routing overrides while leaving managed `test-binary` pass-through local. | `scripts/test_command_understanding.py:629` through `:718` pins runtime/static target-routing policy cases for split and equals options, post-separator test/bench/run/build handling, `--config` file/target-dir/rustflags storage overrides, static env-prefix overrides, managed cargo command routing, and `test-binary` pass-through. Runtime target-routing tests exist near `scripts/test_rust_verification_cache_retention.py:4782`, `:4837`, and `:4883`; static raw storage and workflow tests exist near `scripts/test_verify_ci_workflow_hygiene.py:1715`, `:2799`, `:3025`, and `:4534`. | Divergent policy/return shape, but fully characterized for this slice; pure scan helper remains the only shared target-routing primitive. | Resolved as keep-local; no shared extraction because runtime needs refusal payload detail and managed-command semantics, while static needs bool workflow detection, env-prefix scanning, alias/source text checks, and pass-through exceptions. | Touched: `scripts/test_command_understanding.py`, this ledger, and `specs/466-decompose-disk-governance-verifiers/tasks.md`. Intentionally not touched: runtime/static verifier code and existing owner-suite target-routing tests. | `python3 scripts/test_command_understanding.py`; `python3 -m scripts.test_command_understanding`; `python3 scripts/test_rust_verification_cache_retention.py`; `python3 scripts/test_verify_ci_workflow_hygiene.py`; `python3 -m py_compile scripts/test_command_understanding.py`; `git diff --check`; `just ci-lint-workflow` before PR gate. | #466 pre-implementation plan review approved or operator-waived; RED/GREEN evidence recorded below. Post-implementation review still required before merge. | Resolved. |
| 6 | Mechanical splitting of oversized verifier and verifier-test files by concern where behavior-preserving and reviewable | `scripts/rust_verification.py` is 3053 lines and exposes one CLI parser/main path at `scripts/rust_verification.py:2955` through `:3044` over interdependent policy loading, cache safety, process parsing, wrapper handling, cleanup, target-routing, and managed-command execution. The candidate split areas share policy types, refusal payloads, parser state, and command-entry behavior. | `scripts/verify_ci_workflow_hygiene.py` is 6028 lines and exposes one repository verifier entrypoint at `scripts/verify_ci_workflow_hygiene.py:5891` through `:6001` over workflow parsing, source-build checks, raw-storage policy, target-routing, dynamic-env scanning, no-mistakes checks, and gate validation. Candidate helper regions cross-call through shared tokenization, shell expansion, path-role, and workflow text scanners. | Test surfaces remain intentionally monolithic: `scripts/test_rust_verification_cache_retention.py` is 5216 lines with a single `main` that runs cache, process, cleanup, managed cargo, target-routing, and cleanup policy checks; `scripts/test_verify_ci_workflow_hygiene.py` is 5111 lines with one `main` that runs workflow, source-build, raw-storage, target-routing, no-mistakes, and gate checks. `scripts/test_command_understanding.py` is 794 lines and already owns the focused cross-verifier characterization guards added by this #466 work. | Not a runtime/static semantic equivalence item; audit found no split candidate that would reduce review risk without import/entrypoint churn and broader behavioral blast radius in the consolidated PR. | Resolved as no-split for #466. The current safest behavior-preserving resolution is to keep oversized verifier/test files intact and add focused characterization guards instead of moving cross-cutting code during the consolidated branch. | Touched: this ledger and `specs/466-decompose-disk-governance-verifiers/tasks.md`. Intentionally not touched: `scripts/rust_verification.py`, `scripts/verify_ci_workflow_hygiene.py`, `scripts/test_rust_verification_cache_retention.py`, and `scripts/test_verify_ci_workflow_hygiene.py`. | `wc -l` and top-level structure audit; `python3 scripts/test_command_understanding.py`; `python3 -m scripts.test_command_understanding`; `python3 scripts/test_rust_verification_cache_retention.py`; `python3 scripts/test_verify_ci_workflow_hygiene.py`; `python3 -m py_compile` for touched Python files; `git diff --check`; `just ci-lint-workflow` before PR gate. | #466 pre-implementation plan review approved or operator-waived; no-split audit evidence recorded below. Post-implementation review still required before merge. | Resolved. |
| 7 | Test-only import setup cleanup without weakening direct-script vs module import coverage | Runtime verifier import guard exists in `scripts/rust_verification.py:22`; no runtime behavior change intended. | Static verifier import guard exists in `scripts/verify_ci_workflow_hygiene.py` near its shared helper imports; no static runtime behavior change intended. | `scripts/test_command_understanding.py:23` through `:29` encapsulates test-only `sys.path` setup in `ensure_test_imports_available`; `scripts/test_command_understanding.py:51` through `:89` tracks direct and aliased `sys.path` references and rejects top-level import setup regressions; `scripts/test_command_understanding.py:92` through `:151` pins bare and aliased top-level mutation regressions; `scripts/test_command_understanding.py:154` through `:181` preserves repo-root import and `python3 -m scripts.rust_verification --help` coverage. | Not applicable to verifier semantics; hygiene-only. | Encapsulate the test import setup in a named helper while keeping direct-script and module-mode coverage explicit. | Touched: `scripts/test_command_understanding.py`, this ledger, and `specs/466-decompose-disk-governance-verifiers/tasks.md`. Intentionally not touched: runtime/static verifier code. | `python3 scripts/test_command_understanding.py`; `python3 -m scripts.test_command_understanding`; relevant py_compile; `git diff --check`. | #466 pre-implementation plan review approved or operator-waived; RED/GREEN evidence recorded below. Post-implementation review still required before merge. | Resolved. |
| 8 | Static `consume_cargo_global_options` option handling drift risk, including `CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT` | Shared `scripts/command_understanding.py:9` defines `CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT` for cargo scanner helpers. | Static `scripts/verify_ci_workflow_hygiene.py` now imports shared `CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT`; `consume_cargo_global_options` still uses the same name for static-only option consumption. `CARGO_GLOBAL_OPTIONS_WITHOUT_ARGUMENT` remains local because it intentionally differs from the shared scanner superset. | RED: `python3 scripts/test_command_understanding.py` failed with `AssertionError: static CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT must use the shared cargo scanner constant` after changing the guard from equality to identity. GREEN: same command passed after importing the shared constant; `python3 scripts/test_verify_ci_workflow_hygiene.py` also passed. | Proven identical by object identity for `WITH_ARGUMENT`; `WITHOUT_ARGUMENT` intentionally differs for static-only consumption. | Extract shared `CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT` use into the static verifier and keep static-only `WITHOUT_ARGUMENT` local. | Touched: `scripts/test_command_understanding.py`, `scripts/verify_ci_workflow_hygiene.py`, this ledger. Intentionally not touched: runtime verifier code. | `python3 scripts/test_command_understanding.py`; `python3 scripts/test_verify_ci_workflow_hygiene.py`; py_compile; `git diff --check`; `just ci-lint-workflow`. | #466 pre-implementation plan review approved or operator-waived; RED/GREEN evidence recorded here. Post-implementation review still required before merge. | Resolved. |

## Pre-Implementation Review Gate Status

Pre-implementation external review for #466 is satisfied for implementation start: five reviewers approved exact head `43b9460f077cdff4e8769174f87be887960a1b42`, and the operator explicitly waived the Claude slot after the recorded failure cause was usage limit. This does not satisfy the later post-implementation review or merge gate.

| Reviewer | Route / source state | Result |
|---|---|---|
| Claude | Subscription review attempt `f46b959d-380c-49ea-bc71-9774d0315a1d`, session `244bb5a4-656b-4e01-85bf-94377391ce85`; source sent; failed review slot. | Waived by operator on 2026-05-24 only because failure was usage limit: "skip Claude if the usage limit is the issue." |
| Gemini | Subscription review job `dbb61c80-67bd-4ba7-b1db-1155380af730`, session `92ed3882-248b-4ff4-ac9f-dea7dd566b01`; source sent. | APPROVE; no blocking findings. |
| Grok | Subscription-backed review job `job_6af99e67-1b2a-4329-9654-c71fc677a09b`; source sent. | APPROVE; no blocking findings. |
| GLM | Direct API review job `job_5e9701f0-0043-484f-92a8-ee77eb825f00`, session `20260524130057c7e2267806964cef`; source sent; approval state approved; selected source packet was 9 files, 47,394 bytes, 553 lines. | APPROVE; no blocking findings. |
| DeepSeek | Direct API review job `job_794c7f97-5634-414d-8639-2d963f2790d2`, session `b9a68597-94ae-4fd3-bc81-f1a15837cf43`; source sent; approval state approved; selected source packet was 9 files, 47,394 bytes, 553 lines. | APPROVE; no blocking findings. |
| Kimi | Subscription review job `06bd807c-5731-43c8-ab39-88975447b62d`, session `9b3b92af-420f-4b67-80cb-99b9d422a034`; source sent. | APPROVE; no blocking findings. |

## Post-Implementation Review Gate Status

Post-implementation review must target the current pushed PR head after exact-head GitHub CI is green. Because committing a review-results update changes the PR head, committed rows in this file are historical snapshots unless they describe the current git head. Final merge evidence belongs in the PR status checks and PR body/comment for the final head; superseded snapshots here must not be used as merge approval.

| Reviewer | Route / source state | Result |
|---|---|---|
| PR #474 gate snapshot | Initial reapply head `fbfb82c0e9360c0c7c0bd1abaa4d1f8c81949c73` had exact-head CI green before the evidence-only gate marker commit. Current-head post-implementation external review evidence must be recorded in the PR body/comment after final CI because committing a row here changes the head. | OPEN GATE: do not treat this snapshot or the historical rows below as merge approval for PR #474. Current-head Claude, Gemini, Grok, GLM, DeepSeek, and Kimi reviews or explicit operator waivers are still required before merge readiness. |
| Gemini | Subscription review job `5d0e79f8-0f41-4521-8baa-0676fcad12e3`, session `32fa4ad1-9875-427e-9965-3326a2a1b679`; source sent for head `9b020b7a363f959afa01a4eb8fd0074eb6614540`. | REQUEST_CHANGES: stale committed CI/review evidence referenced superseded heads. |
| Grok | Subscription-backed review job `job_d1775521-0105-4cc3-beef-deed2e93759e`; source sent for head `9b020b7a363f959afa01a4eb8fd0074eb6614540`. | REQUEST_CHANGES: stale committed CI/review evidence referenced superseded heads. |
| Kimi | Subscription review job `f77fbb82-9f33-467c-af13-30cc38278ed5`, session `b2f90e0a-e5dc-441d-937a-37a6849c871e`; source sent for head `9b020b7a363f959afa01a4eb8fd0074eb6614540`. | Failed slot: `step_limit_exceeded`; no usable verdict. |

## Slice Verification: Ledger Item 1

| Command | Result |
|---|---|
| `python3 scripts/test_command_understanding.py` after adding newline line-boundary characterization with an intentionally wrong expected value | Expected fail: `AssertionError: static command_tokens_with_line_boundaries('cargo build\ncargo test') changed: ['cargo', 'build', ';', 'cargo', 'test']`. |
| `python3 scripts/test_command_understanding.py` after correcting the expected static line-boundary tokens | Pass: `OK: command understanding self-tests passed.` |
| `python3 -m scripts.test_command_understanding` | Pass: `OK: command understanding self-tests passed.` |
| `python3 -m py_compile scripts/test_command_understanding.py` | Pass. |
| `git diff --check` | Pass. |
| `just ci-lint-workflow` | Pass: CI workflow hygiene, same-SHA evidence, path-filter, Rust verification owner, command understanding, Rust verification decoupling, Rust verification cache retention, and raw cargo workflow command checks passed. |

## Slice Verification: Ledger Item 2

| Command | Result |
|---|---|
| `python3 scripts/test_command_understanding.py` after adding dedicated shell-substitution characterization with an intentionally wrong static raw-`$(` expectation | Expected fail: `AssertionError: static shell_command_substitution_payloads(['echo', '$(', 'cargo', ')']) changed: []`. |
| `python3 scripts/test_command_understanding.py` after correcting the static raw-`$(` expectation and moving item-2 checks out of the generic non-export block | Pass: `OK: command understanding self-tests passed.` |
| `python3 -m scripts.test_command_understanding` | Pass: `OK: command understanding self-tests passed.` |
| `python3 -m py_compile scripts/test_command_understanding.py` | Pass. |
| `git diff --check` | Pass. |
| `just ci-lint-workflow` | Pass: CI workflow hygiene, same-SHA evidence, path-filter, Rust verification owner, command understanding, Rust verification decoupling, Rust verification cache retention, and raw cargo workflow command checks passed. |

## Slice Verification: Ledger Item 3

| Command | Result |
|---|---|
| `python3 scripts/test_command_understanding.py` after adding dedicated renamed-tool characterization with an intentionally wrong static `rustup` cargo-name expectation | Expected fail: `AssertionError: static cargo path-name classifier changed for 'rustup': False`. |
| `python3 scripts/test_command_understanding.py` after correcting the static `rustup` expectation | Pass: `OK: command understanding self-tests passed.` |
| `python3 -m scripts.test_command_understanding` | Pass: `OK: command understanding self-tests passed.` |
| `python3 -m py_compile scripts/test_command_understanding.py` | Pass. |
| `python3 scripts/test_rust_verification_cache_retention.py` | Pass: `OK: Rust verification cache retention self-tests passed.` |
| `python3 scripts/test_verify_ci_workflow_hygiene.py` | Pass: `OK: CI workflow hygiene verifier self-tests passed.` |
| `git diff --check` | Pass. |
| `just ci-lint-workflow` | Pass: CI workflow hygiene, same-SHA evidence, path-filter, Rust verification owner, command understanding, Rust verification decoupling, Rust verification cache retention, and raw cargo workflow command checks passed. |

## Slice Verification: Ledger Item 4

| Command | Result |
|---|---|
| `python3 scripts/test_command_understanding.py` after adding dedicated wrapper characterization with an intentionally over-recursive static `env -S` expectation | Expected fail: `AssertionError: static wrapper_inner_tokens(['env', '-S', 'timeout 30 cargo build']) changed: ['timeout', '30', 'cargo', 'build']`. |
| `python3 scripts/test_command_understanding.py` after correcting the static one-layer `env -S` expectation | Pass: `OK: command understanding self-tests passed.` |
| `python3 -m scripts.test_command_understanding` | Pass: `OK: command understanding self-tests passed.` |
| `python3 -m py_compile scripts/test_command_understanding.py` | Pass. |
| `python3 scripts/test_rust_verification_cache_retention.py` | Pass: `OK: Rust verification cache retention self-tests passed.` |
| `python3 scripts/test_verify_ci_workflow_hygiene.py` | Pass: `OK: CI workflow hygiene verifier self-tests passed.` |
| `git diff --check` | Pass. |
| `just ci-lint-workflow` | Pass: CI workflow hygiene, same-SHA evidence, path-filter, Rust verification owner, command understanding, Rust verification decoupling, Rust verification cache retention, and raw cargo workflow command checks passed. |

## Slice Verification: Ledger Item 5

| Command | Result |
|---|---|
| `python3 scripts/test_command_understanding.py` after adding dedicated target-routing policy characterization with an intentionally wrong runtime post-`--` `cargo test` expectation | Expected fail: `AssertionError: runtime cargo_target_routing_override(['test', '--', '--target-dir', '/tmp/raw']) changed: None`. |
| `python3 scripts/test_command_understanding.py` after correcting the runtime post-`--` expectation | Pass: `OK: command understanding self-tests passed.` |

## Slice Verification: Ledger Item 6

| Command | Result |
|---|---|
| `wc -l scripts/rust_verification.py scripts/verify_ci_workflow_hygiene.py scripts/test_rust_verification_cache_retention.py scripts/test_verify_ci_workflow_hygiene.py scripts/test_command_understanding.py` | Audit: candidate files are large (`3053`, `6028`, `5216`, `5111`, and `794` lines respectively), but size alone does not prove a behavior-preserving split boundary. |
| `rg -n "^(def|class) " scripts/rust_verification.py scripts/verify_ci_workflow_hygiene.py` | Audit: production candidates expose cross-cutting command, shell, policy, workflow, cache, cleanup, target-routing, and CLI entrypoint helpers rather than one isolated concern boundary suitable for a low-risk mechanical move in this PR. |
| `rg -n "^(def|class) |if __name__|sys.exit\\(|argparse|load_module|import" scripts/test_rust_verification_cache_retention.py scripts/test_verify_ci_workflow_hygiene.py scripts/test_command_understanding.py` | Audit: test candidates are self-contained direct-script suites with single `main` entrypoints and cross-suite policy aggregators; splitting them in this consolidated branch would add import/entrypoint churn without reducing current review risk. |

## Slice Verification: Ledger Item 8

| Command | Result |
|---|---|
| `python3 scripts/test_command_understanding.py` after RED identity guard only | Expected fail: `AssertionError: static CARGO_GLOBAL_OPTIONS_WITH_ARGUMENT must use the shared cargo scanner constant`. |
| `python3 scripts/test_command_understanding.py` after static import | Pass: `OK: command understanding self-tests passed.` |
| `python3 scripts/test_verify_ci_workflow_hygiene.py` | Pass: `OK: CI workflow hygiene verifier self-tests passed.` |
| `python3 -m scripts.test_command_understanding` | Pass: `OK: command understanding self-tests passed.` |
| `python3 -m py_compile scripts/command_understanding.py scripts/test_command_understanding.py scripts/verify_ci_workflow_hygiene.py` | Pass. |
| `git diff --check` | Pass. |
| `just ci-lint-workflow` | Pass: CI workflow hygiene, same-SHA evidence, path-filter, Rust verification owner, command understanding, Rust verification decoupling, Rust verification cache retention, and raw cargo workflow command checks passed. |

## Slice Verification: Ledger Item 7

| Command | Result |
|---|---|
| `python3 scripts/test_command_understanding.py` after RED import-setup guard only | Expected fail: `AssertionError: test import sys.path setup must be encapsulated in a helper`. |
| `python3 scripts/test_command_understanding.py` after encapsulating setup | Pass: `OK: command understanding self-tests passed.` |
| PR #470 Greptile and Gemini review comments on `825ea636` | Finding accepted: the first guard only rejected top-level `if` statements containing `sys.path` and missed bare top-level calls such as `sys.path.insert(...)`. |
| `python3 scripts/test_command_understanding.py` after adding bare top-level mutation regression only | Expected fail: `AssertionError: bare top-level sys.path setup must be rejected`. |
| `python3 scripts/test_command_understanding.py` after widening the guard | Pass: `OK: command understanding self-tests passed.` |
| PR #470 external review on `84acc529` after widening the guard | Finding accepted: the string-based guard could miss aliased top-level setup such as `from sys import path` followed by `path.insert(...)`; Kimi retries failed with `step_limit_exceeded` and produced no usable verdict. |
| `python3 scripts/test_command_understanding.py` after adding aliased top-level mutation regression only | Expected fail: `AssertionError: aliased top-level sys.path setup must be rejected`. |
| `python3 scripts/test_command_understanding.py` after replacing the substring guard with AST alias detection | Pass: `OK: command understanding self-tests passed.` |
| `python3 -m scripts.test_command_understanding` | Pass: `OK: command understanding self-tests passed.` |
| `python3 -m py_compile scripts/test_command_understanding.py` | Pass. |
| `python3 scripts/test_verify_ci_workflow_hygiene.py` | Pass: `OK: CI workflow hygiene verifier self-tests passed.` |
| `python3 scripts/test_rust_verification_cache_retention.py` | Pass: `OK: Rust verification cache retention self-tests passed.` |
| `git diff --check` | Pass. |
| `just ci-lint-workflow` | Pass: CI workflow hygiene, same-SHA evidence, path-filter, Rust verification owner, command understanding, Rust verification decoupling, Rust verification cache retention, and raw cargo workflow command checks passed. |
