# Evidence Map: Decompose Disk-Governance Verifiers

## Current State

| Evidence | Result |
|---|---|
| #375 issue state | Closed as completed at 2026-05-23T14:52:18Z after PR #460 merge. |
| #460 merge commit | `f354efbaa2afc78575e9cc40580cf2b682bd66e6` |
| #454 issue state | Open: "Decompose disk-governance verifier scripts after #436" |
| Branch | `codex/454-decompose-disk-governance-verifiers` |
| Branch base | `f354efbaa2afc78575e9cc40580cf2b682bd66e6` |

## Baseline Verification

| Command | Result |
|---|---|
| `python3 scripts/test_rust_verification_cache_retention.py` | Pass: `OK: Rust verification cache retention self-tests passed.` |
| `python3 scripts/test_verify_ci_workflow_hygiene.py` | Pass: `OK: CI workflow hygiene verifier self-tests passed.` |

## Planning Validation

| Command | Result |
|---|---|
| `rg -n "NEEDS CLARIFICATION|\\[FEATURE|\\[###|ACTION REQUIRED|TODO|fix later" specs/454-decompose-disk-governance-verifiers/spec.md specs/454-decompose-disk-governance-verifiers/plan.md specs/454-decompose-disk-governance-verifiers/research.md specs/454-decompose-disk-governance-verifiers/data-model.md specs/454-decompose-disk-governance-verifiers/evidence.md specs/454-decompose-disk-governance-verifiers/tasks.md specs/454-decompose-disk-governance-verifiers/contracts` | Pass: no unresolved marker matches. |
| `rg -n "specs/454-decompose-disk-governance-verifiers/plan.md" AGENTS.md .specify/feature.json` | Pass: `AGENTS.md` points at the #454 plan. |
| `git diff --check` | Pass: no whitespace errors. |

## Size Evidence

| File | Lines |
|---|---:|
| `scripts/rust_verification.py` | 2738 |
| `scripts/verify_ci_workflow_hygiene.py` | 6175 |
| `scripts/test_rust_verification_cache_retention.py` | 3175 |
| `scripts/test_verify_ci_workflow_hygiene.py` | 5102 |

## Duplicate Helper Evidence

| Helper family | Runtime verifier | Static workflow verifier |
|---|---:|---:|
| `command_tokens` | `scripts/rust_verification.py:507` | `scripts/verify_ci_workflow_hygiene.py:1215` |
| `shell_command_substitution_at` | `scripts/rust_verification.py:655` | `scripts/verify_ci_workflow_hygiene.py:2339` |
| `python_command_string` | `scripts/rust_verification.py:759` | `scripts/verify_ci_workflow_hygiene.py:1407` |
| `python_inline_command_payloads` | `scripts/rust_verification.py:792` | `scripts/verify_ci_workflow_hygiene.py:1440` |
| `path_name_looks_like_renamed_cargo` | `scripts/rust_verification.py:1626` | `scripts/verify_ci_workflow_hygiene.py:2605` |
| `path_executable_looks_like_cargo` | `scripts/rust_verification.py:1632` | `scripts/verify_ci_workflow_hygiene.py:2609` |
| `path_name_looks_like_renamed_rustc` | `scripts/rust_verification.py:1645` | `scripts/verify_ci_workflow_hygiene.py:2618` |
| `path_executable_looks_like_rustc` | `scripts/rust_verification.py:1649` | `scripts/verify_ci_workflow_hygiene.py:2622` |
| target-routing override scan | Not a full duplicate | `scripts/verify_ci_workflow_hygiene.py:1862` |

## Latent Risk

- Runtime process cleanup and static workflow hygiene currently encode overlapping command-understanding rules independently.
- A future fix to one parser surface can miss the other surface, recreating stale-cache or raw-cargo mismatch risk.
- Test files are also large, so a cosmetic split without characterization can hide behavior changes.

## Future Enablement Requirement

- Add focused characterization/parity tests before moving parser code.
- Introduce `scripts/command_understanding.py` as the shared path for extracted helpers.
- Rewire both verifier clients only after the shared module tests are red/green.
- Keep any remaining oversized-surface split mechanical and separately evidenced.

## Review Notes

- Planning artifacts must receive adversarial review before implementation starts.
- no-mistakes is intentionally excluded unless the operator explicitly requests it.
