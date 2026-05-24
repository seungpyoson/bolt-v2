# Research: Decompose Disk-Governance Verifiers

## Decision: Start from merged #375 main and keep #454 isolated

**Rationale**: #375 closed as completed at 2026-05-23T14:52:18Z, and `origin/main` was fetched to `f354efbaa2afc78575e9cc40580cf2b682bd66e6`, the PR #460 merge commit. Branch `codex/454-decompose-disk-governance-verifiers` was created from that exact commit.

**Alternatives considered**: Reusing the #375 worktree or stale local `main` was rejected because repo rules make post-merge `main` authoritative and #454 requires fresh `origin/main`.

## Decision: Characterize current behavior before moving parser code

**Rationale**: Baseline commands passed on the fresh branch:

| Command | Result |
|---|---|
| `python3 scripts/test_rust_verification_cache_retention.py` | Pass: `OK: Rust verification cache retention self-tests passed.` |
| `python3 scripts/test_verify_ci_workflow_hygiene.py` | Pass: `OK: CI workflow hygiene verifier self-tests passed.` |

The issue risk is behavior drift between current runtime and static verifiers. Characterization tests preserve current behavior while extraction happens.

**Alternatives considered**: Moving code first and trusting existing suites was rejected because existing suites validate each surface separately and do not prove shared parity.

## Decision: Add one shared command-understanding module only for proven-equivalent helpers

**Rationale**: The candidate helper families are concrete and source-located, but same-name evidence is not enough to prove safe extraction:

| Helper family | Runtime verifier | Static workflow verifier | Current classification |
|---|---:|---:|---|
| `command_tokens` | `scripts/rust_verification.py:507` | `scripts/verify_ci_workflow_hygiene.py:1215` | Divergent: runtime uses `shlex.split`; static uses punctuation-aware `shlex.shlex` and token splitting. |
| `shell_command_substitution_payloads` | `scripts/rust_verification.py:622` | `scripts/verify_ci_workflow_hygiene.py:1311` | Divergent dependency boundary: runtime normalizes tokens before scanning; static scans caller tokens. |
| `shell_command_substitution_at` | `scripts/rust_verification.py:655` | `scripts/verify_ci_workflow_hygiene.py:2339` | Divergent: runtime requires normalized exact `$`; static accepts tokens ending in `$`. |
| Python AST command helpers | `scripts/rust_verification.py:740` | `scripts/verify_ci_workflow_hygiene.py:1388` | Equivalent candidate: `python_constant_string`, `python_command_string`, `python_call_name`, and `python_call_command_argument` support Python inline extraction. |
| `python_inline_command_payloads` | `scripts/rust_verification.py:792` | `scripts/verify_ci_workflow_hygiene.py:1440` | Equivalent candidate when moved with its Python AST helper dependencies. |
| `path_name_looks_like_renamed_cargo` | `scripts/rust_verification.py:1626` | `scripts/verify_ci_workflow_hygiene.py:2605` | Divergent: runtime treats `rustup` as a Rust tool; static raw-token helper does not. |
| `path_executable_looks_like_cargo` | `scripts/rust_verification.py:1632` | `scripts/verify_ci_workflow_hygiene.py:2609` | Divergent: runtime resolves filesystem symlinks; static inspects only the token path name. |
| `path_name_looks_like_renamed_rustc` | `scripts/rust_verification.py:1645` | `scripts/verify_ci_workflow_hygiene.py:2618` | Similar, but still requires pre-extraction behavior comparison before export. |
| `path_executable_looks_like_rustc` | `scripts/rust_verification.py:1649` | `scripts/verify_ci_workflow_hygiene.py:2622` | Divergent filesystem boundary: runtime resolves symlinks; static inspects only the token path name. |
| cargo subcommand scan | `scripts/rust_verification.py:2219` | `scripts/verify_ci_workflow_hygiene.py:1768` | Analogous, but static accepts a `start` offset and must be classified before extraction. |
| wrapper handling | Runtime recursive wrapper indices around `scripts/rust_verification.py:1513` | `scripts/verify_ci_workflow_hygiene.py:2069` | Not a same helper; characterize current behavior, do not extract without a separate equivalence proof. |
| target-routing override scan | `scripts/rust_verification.py:2357` | `scripts/verify_ci_workflow_hygiene.py:1862` | Analogous policy behavior, not a full duplicate; characterize per surface and defer shared extraction unless equivalence is proven. |

The initial shared module export set is therefore limited to helper families that the pre-extraction comparison proves equivalent. The Python inline-command helper family is the current candidate because both surfaces share the same AST support behavior and payload extraction shape. Divergent tokenization, shell substitution, path-resolution, wrapper, and target-routing helpers are characterization targets, not mechanical extraction targets.

**Alternatives considered**: Splitting the oversized test files first was rejected because it reduces line counts but does not address parser behavior drift. Forcing every same-named helper into one body was rejected because it would change at least one verifier surface.

## Decision: Limit the first extraction to parser/scanner behavior already accepted

**Rationale**: #454 explicitly forbids adding shell edge cases, regex cases, wrapper families, command-prediction behavior, or policy changes. The first shared path should copy/centralize only existing equivalent behavior and let existing tests plus new parity tests prove no behavior drift. Divergent helpers need current-behavior characterization and either deferral or explicit operator-approved semantic-change handling.

**Alternatives considered**: Redesigning command parsing into a richer shell model was rejected as out of scope and likely to recreate the edge-case chase #454 warns against.

## Decision: Keep no-mistakes out of this issue unless explicitly requested

**Rationale**: Issue #454 says "Do not use no-mistakes unless explicitly requested by the operator." The required gate is exact-head GitHub CI plus external exact-head review. The operator later explicitly requested no-mistakes for PR #461, so any resulting lint or CI cleanup is ancillary gate cleanup and must be recorded separately from the issue #454 verifier-decomposition slice.

**Alternatives considered**: Running no-mistakes by default was rejected because it would violate the issue process contract.
