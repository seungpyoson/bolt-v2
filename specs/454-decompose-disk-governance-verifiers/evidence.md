# Evidence Map: Decompose Disk-Governance Verifiers

## Current State

| Evidence | Result |
|---|---|
| #375 issue state | Closed as completed at 2026-05-23T14:52:18Z after PR #460 merge. |
| #460 merge commit | `f354efbaa2afc78575e9cc40580cf2b682bd66e6` |
| #454 issue state | Open: "Decompose disk-governance verifier scripts after #436" |
| Branch | `codex/454-decompose-disk-governance-verifiers` |
| Branch base | `0cc03a07b4ef7da5c1bef71476d48d4745933772` |
| Current PR head | `9a7e8e4678359f7978c9d09cdcf432f729e28676` |

## Baseline Verification

| Command | Result |
|---|---|
| `python3 scripts/test_rust_verification_cache_retention.py` | Pass: `OK: Rust verification cache retention self-tests passed.` |
| `python3 scripts/test_verify_ci_workflow_hygiene.py` | Pass: `OK: CI workflow hygiene verifier self-tests passed.` |
| `python3 scripts/test_rust_verification_cache_retention.py` on 2026-05-24 | Pass: `OK: Rust verification cache retention self-tests passed.` |
| `python3 scripts/test_verify_ci_workflow_hygiene.py` on 2026-05-24 | Pass: `OK: CI workflow hygiene verifier self-tests passed.` |
| `python3 -m py_compile scripts/rust_verification.py scripts/verify_ci_workflow_hygiene.py scripts/test_rust_verification_cache_retention.py scripts/test_verify_ci_workflow_hygiene.py` on 2026-05-24 | Pass: no output. |

## Implementation Verification

| Command | Result |
|---|---|
| `python3 scripts/test_command_understanding.py` before `scripts/command_understanding.py` existed | RED: failed with `AssertionError: missing scripts/command_understanding.py`. |
| `python3 scripts/test_command_understanding.py` after adding `scripts/command_understanding.py` | GREEN: `OK: command understanding self-tests passed.` |
| `python3 scripts/test_command_understanding.py` before rewiring verifier clients | RED: failed because both verifier clients still used local Python AST helper definitions. |
| `python3 scripts/test_command_understanding.py` after rewiring verifier clients | GREEN: `OK: command understanding self-tests passed.` |
| `python3 -m py_compile scripts/command_understanding.py scripts/rust_verification.py scripts/verify_ci_workflow_hygiene.py scripts/test_command_understanding.py` | Pass: no output. |
| `python3 scripts/test_rust_verification_cache_retention.py` | Pass: `OK: Rust verification cache retention self-tests passed.` |
| `python3 scripts/test_verify_ci_workflow_hygiene.py` | Pass: `OK: CI workflow hygiene verifier self-tests passed.` |

## Planning Validation

| Command | Result |
|---|---|
| `marker_pattern='NEEDS'' CLARIFICATION|\\[FEA''TURE|\\[#''##|ACTION'' REQUIRED|TO''DO|fix'' later'; rg -n "$marker_pattern" specs/454-decompose-disk-governance-verifiers/spec.md specs/454-decompose-disk-governance-verifiers/plan.md specs/454-decompose-disk-governance-verifiers/research.md specs/454-decompose-disk-governance-verifiers/data-model.md specs/454-decompose-disk-governance-verifiers/evidence.md specs/454-decompose-disk-governance-verifiers/tasks.md specs/454-decompose-disk-governance-verifiers/contracts` | Pass: no unresolved marker matches. |
| `rg -n "specs/023-nt-order-intent-layer/plan.md" AGENTS.md .specify/feature.json` | Pass: global project pointers stay on the existing active plan; #454 artifacts remain PR-scoped under `specs/454-decompose-disk-governance-verifiers/`. |
| `git diff --check` | Pass: no whitespace errors. |

## Size Evidence

| File | Lines |
|---|---:|
| `scripts/rust_verification.py` before extraction | 2738 |
| `scripts/rust_verification.py` after extraction | 2663 |
| `scripts/verify_ci_workflow_hygiene.py` before extraction | 6175 |
| `scripts/verify_ci_workflow_hygiene.py` after extraction | 6099 |
| `scripts/command_understanding.py` | 88 |
| `scripts/test_command_understanding.py` | 350 |
| `scripts/test_rust_verification_cache_retention.py` | 3175 |
| `scripts/test_verify_ci_workflow_hygiene.py` | 5102 |

## Candidate Helper Evidence And Eligibility

| Helper family | Runtime verifier | Static workflow verifier | Current eligibility |
|---|---:|---:|---|
| `command_tokens` | `scripts/rust_verification.py:507` | `scripts/verify_ci_workflow_hygiene.py:1215` | Divergent: runtime uses simple `shlex.split`; static uses punctuation-aware lexing and token splitting. Characterize only unless a semantic change is approved. |
| `shell_command_substitution_payloads` | `scripts/rust_verification.py:622` | `scripts/verify_ci_workflow_hygiene.py:1311` | Divergent dependency boundary: runtime normalizes tokens before scanning; static scans caller tokens. |
| `shell_command_substitution_at` | `scripts/rust_verification.py:655` | `scripts/verify_ci_workflow_hygiene.py:2339` | Divergent: runtime requires normalized exact `$`; static accepts tokens ending in `$`. |
| Python AST command helpers | `scripts/rust_verification.py:21` imports shared helpers | `scripts/verify_ci_workflow_hygiene.py:13` imports shared helpers | Extracted to `scripts/command_understanding.py:9`, `:28`, `:43`, and `:52`; characterization tests prove current behavior parity. |
| `python_inline_command_payloads` | `scripts/rust_verification.py:21` imports shared helper | `scripts/verify_ci_workflow_hygiene.py:13` imports shared helper | Extracted to `scripts/command_understanding.py:61`; characterization tests cover scalar, list, keyword-argument, dynamic, and syntax-error cases. |
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

## Implementation Result

- Extracted only the proven-equivalent Python AST helper family to `scripts/command_understanding.py`.
- Rewired `scripts/rust_verification.py` and `scripts/verify_ci_workflow_hygiene.py` to import the shared helper path.
- Left divergent or unproven helper families local: tokenization, shell command substitutions, renamed executable detection, wrapper handling, cargo subcommand scanning, and target-routing override scanning.
- Added `scripts/test_command_understanding.py` characterization tests for both extracted helpers and representative divergent/deferred helper behavior.
- No mechanical test split was made; remaining oversized verifier test files are intentionally deferred and documented as residual review risk.

## Review Notes

- Planning artifacts received initial adversarial review on head `c966ce2f4509cfe5f577a79f7847cfcecb3f6717`.
- Claude `0ac369aa-b507-44cb-8d5d-a5c689c1d0b1`: APPROVE with non-blocking notes about contract/evidence mismatch.
- Kimi `2790fa79-8a23-4647-9709-ead1af30c9f0`: REQUEST_CHANGES because several claimed duplicate helpers are semantically divergent.
- GLM `job_af3c8ed8-768b-4608-9c46-73629a2e628b`: REQUEST_CHANGES because `python_command_string` was missing from the contract and unproven helpers were predeclared.
- DeepSeek `job_909a409d-5486-4f30-83f6-6a35cf69a941`: APPROVE with non-blocking notes about `python_command_string` and classification contingencies.
- This planning revision narrows extraction eligibility to pre-extraction-proven equivalent helpers and treats divergent candidates as characterization-only unless operator-approved semantic-change evidence is added.
- Exact-head re-review on `39e5a22b93de193fd019bf7334e676eeed9aad7b` returned APPROVE from Claude `4f0661e9-485e-46ad-862f-540a9a2be3ba`, Kimi `6b2c181a-9165-426a-8b06-2ce71633c1ef`, GLM `job_c42abe34-c146-40bb-a833-b2f16189540c`, and DeepSeek `job_c0e20a68-7c2d-44f4-9a1b-f0f69255a853`.
- On 2026-05-24 the required planning gate was strengthened to require unanimous current-head approvals from Claude, Gemini, Kimi, Grok, GLM, and DeepSeek before implementation starts.
- Planning gate record for exact head `c0d7332bf4f30e4ddef314c69f3a51c13cfc31d2` was posted to issue #454: https://github.com/seungpyoson/bolt-v2/issues/454#issuecomment-4525926019. Clean APPROVE slots: Claude `cc2312d6-429e-4e6c-bc6e-9b92e669a30a`, Gemini `00e31366-c3bc-43d6-bd10-35c2a76037fa`, Grok `job_6d446dd9-d391-493c-b709-7871f6867324`, GLM `job_5315e9db-2110-4eea-8d9a-a438669d037d`, DeepSeek `job_2bb9cfed-fb15-45c8-a10a-b92def6b6f9c`, and Kimi `99c0e045-b37f-4fca-9e3b-52c3fd5d34b9`.
- Kimi's clean approval used a core planning custom-review packet after two branch-diff attempts failed; final PR-head external review must still include Kimi on the full PR head before merge readiness.
- no-mistakes was run after the operator explicitly requested it; the live PR head includes its follow-up lint/CI commits and still requires exact-head CI plus exact-head external review before merge readiness.

## Reviewer Availability Evidence

| Reviewer | Command | Result |
|---|---|---|
| Claude | `node .../claude-companion.mjs doctor --auth-mode subscription --cwd ...` | Ready: subscription OAuth, model `claude-opus-4-7`. |
| Gemini | `node .../gemini-companion.mjs doctor --cwd ...` | Ready: subscription OAuth, model `gemini-3.1-pro-preview`. |
| Kimi | `node .../kimi-companion.mjs doctor --cwd ...` | Ready: subscription OAuth, model `kimi-code/kimi-for-coding`. |
| Grok | `node .../grok-companion.mjs doctor` | Ready after operator login: subscription CLI, logged in, model `grok-build`. |
| DeepSeek | `api-reviewer doctor --provider deepseek` | Ready: direct API, model `deepseek-v4-pro`, source-free probe HTTP 200. |
| GLM | `api-reviewer doctor --provider glm` | Ready: direct API, model `glm-5.1`, source-free probe HTTP 200. |
