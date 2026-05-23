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
| `marker_pattern='NEEDS'' CLARIFICATION|\\[FEA''TURE|\\[#''##|ACTION'' REQUIRED|TO''DO|fix'' later'; rg -n "$marker_pattern" specs/454-decompose-disk-governance-verifiers/spec.md specs/454-decompose-disk-governance-verifiers/plan.md specs/454-decompose-disk-governance-verifiers/research.md specs/454-decompose-disk-governance-verifiers/data-model.md specs/454-decompose-disk-governance-verifiers/evidence.md specs/454-decompose-disk-governance-verifiers/tasks.md specs/454-decompose-disk-governance-verifiers/contracts` | Pass: no unresolved marker matches. |
| `rg -n "specs/454-decompose-disk-governance-verifiers/plan.md" AGENTS.md .specify/feature.json` | Pass: `AGENTS.md` points at the #454 plan. |
| `git diff --check` | Pass: no whitespace errors. |

## Size Evidence

| File | Lines |
|---|---:|
| `scripts/rust_verification.py` | 2738 |
| `scripts/verify_ci_workflow_hygiene.py` | 6175 |
| `scripts/test_rust_verification_cache_retention.py` | 3175 |
| `scripts/test_verify_ci_workflow_hygiene.py` | 5102 |

## Candidate Helper Evidence And Eligibility

| Helper family | Runtime verifier | Static workflow verifier | Current eligibility |
|---|---:|---:|---|
| `command_tokens` | `scripts/rust_verification.py:507` | `scripts/verify_ci_workflow_hygiene.py:1215` | Divergent: runtime uses simple `shlex.split`; static uses punctuation-aware lexing and token splitting. Characterize only unless a semantic change is approved. |
| `shell_command_substitution_payloads` | `scripts/rust_verification.py:622` | `scripts/verify_ci_workflow_hygiene.py:1311` | Divergent dependency boundary: runtime normalizes tokens before scanning; static scans caller tokens. |
| `shell_command_substitution_at` | `scripts/rust_verification.py:655` | `scripts/verify_ci_workflow_hygiene.py:2339` | Divergent: runtime requires normalized exact `$`; static accepts tokens ending in `$`. |
| Python AST command helpers | `scripts/rust_verification.py:740` | `scripts/verify_ci_workflow_hygiene.py:1388` | Equivalent candidate: includes `python_constant_string`, `python_command_string`, `python_call_name`, and `python_call_command_argument`. |
| `python_inline_command_payloads` | `scripts/rust_verification.py:792` | `scripts/verify_ci_workflow_hygiene.py:1440` | Equivalent candidate when moved with its Python AST helper dependencies. |
| `path_name_looks_like_renamed_cargo` | `scripts/rust_verification.py:1626` | `scripts/verify_ci_workflow_hygiene.py:2605` | Divergent: runtime includes `rustup`; static raw-token helper does not. |
| `path_executable_looks_like_cargo` | `scripts/rust_verification.py:1632` | `scripts/verify_ci_workflow_hygiene.py:2609` | Divergent: runtime resolves filesystem symlinks; static inspects only token path name. |
| `path_name_looks_like_renamed_rustc` | `scripts/rust_verification.py:1645` | `scripts/verify_ci_workflow_hygiene.py:2618` | Similar, but still requires pre-extraction behavior comparison before export. |
| `path_executable_looks_like_rustc` | `scripts/rust_verification.py:1649` | `scripts/verify_ci_workflow_hygiene.py:2622` | Divergent filesystem boundary: runtime resolves symlinks; static inspects only token path name. |
| cargo subcommand scan | `scripts/rust_verification.py:2219` | `scripts/verify_ci_workflow_hygiene.py:1768` | Analogous but not proven identical; static accepts a `start` offset. Characterize before extraction. |
| wrapper handling | Runtime recursive wrapper indices around `scripts/rust_verification.py:1513` | `scripts/verify_ci_workflow_hygiene.py:2069` | Not the same helper contract. Characterize current behavior, do not extract mechanically. |
| target-routing override scan | `scripts/rust_verification.py:2357` | `scripts/verify_ci_workflow_hygiene.py:1862` | Analogous policy behavior, not a full duplicate helper. Characterize per surface and defer shared extraction unless equivalence is proven. |

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

- Planning artifacts received initial adversarial review on head `c966ce2f4509cfe5f577a79f7847cfcecb3f6717`.
- Claude `0ac369aa-b507-44cb-8d5d-a5c689c1d0b1`: APPROVE with non-blocking notes about contract/evidence mismatch.
- Kimi `2790fa79-8a23-4647-9709-ead1af30c9f0`: REQUEST_CHANGES because several claimed duplicate helpers are semantically divergent.
- GLM `job_af3c8ed8-768b-4608-9c46-73629a2e628b`: REQUEST_CHANGES because `python_command_string` was missing from the contract and unproven helpers were predeclared.
- DeepSeek `job_909a409d-5486-4f30-83f6-6a35cf69a941`: APPROVE with non-blocking notes about `python_command_string` and classification contingencies.
- This planning revision narrows extraction eligibility to pre-extraction-proven equivalent helpers and treats divergent candidates as characterization-only unless operator-approved semantic-change evidence is added.
- Current-head adversarial re-review is required before implementation starts.
- no-mistakes is intentionally excluded unless the operator explicitly requests it.
