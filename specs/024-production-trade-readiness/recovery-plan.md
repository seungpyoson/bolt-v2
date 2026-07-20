# PR #480 Production Trade-Readiness — Recovery Plan

> Working recovery plan authored at HEAD `5097e6bc36bcc0f67c450692db797b51bb8220d7`.
> Every claim below was personally verified against code/tests/git/CI at this HEAD
> (not taken from prior-agent docs, and not taken on faith from audit subagents).

## 0. Scope of this plan

PR #480's single declared scope = **production-grade live trade readiness for the
selected tiny-capital path** (issue #024 / #369), closing out tasks T036–T047. This
plan drives PR #480 to an **honest** readiness verdict. It is NOT just docs, not just
T044, not just #492, not just the data-adapter work.

## 1. Verified state at HEAD (source-of-truth recovery audit)

### 1.1 CI is RED (run 26673253247) — falsifies any current "ready" claim

| Lane | Root cause (personally reproduced) | Origin |
|---|---|---|
| clippy | `operator_artifacts.rs:7837,:7857` needless_borrow (`&selected`); `binary_oracle_edge_taker.rs:5972` useless_conversion (`u64::try_from` on a `u64`) | 7837=`bb4af925`, 7857=`603eae90`, 5972=`d60a4c3e` — all native 024 commits |
| fmt-check + source-fence | same root: `verify_bolt_v3_runtime_literals.py` allowlist out of sync — 5 stale rows + ~60 unclassified literals | 024 trade-chunk/tick (live_node/operator_artifacts/validate) + venue_account_state_source data-API literals |
| nextest ×3 | 3 failing tests (also fail locally — confirmed) | see 1.2 |
| gate, test | cascade gates from the above | — |

### 1.2 The 3 test failures

- **`client_registration::live_node_build_path_registers_polymarket_data_polymarket_exec_and_binance_data`** (`tests/bolt_v3_client_registration.rs:201`): scoped build path registers **1** client (`polymarket_main`), test asserts **2** (expects `binance_reference`). **Stale assertion.** See §1.4.
- **`adapter_mapping::live_node_build_path_propagates_adapter_mapping_failures`** (`tests/bolt_v3_adapter_mapping.rs:477`): forces the **binance_reference** secret to fail, but scoping drops binance *before* secret resolution → no `SecretResolution` surfaces. **Stale fixture target.** See §1.4.
- **`live_canary_gate::live_canary_gate_rejects_oversized_approval_consumption_before_reading_to_eof`** (`tests/bolt_v3_live_canary_gate.rs:1227`): oversize operator-evidence read returns a different error than `OperatorApprovalConsumptionRead{exceeds}`. Proof-policy fixture/sizing **desync** between `largest_pre_consumption_operator_evidence_file_len()` and the gate's per-file read accounting. See §1.5.

### 1.3 Correction to the takeover brief's framing (evidence-backed)

- **There is no cherry-picked "t043a/#490 stack."** `venue_account_state_source.rs` (`9f45be00`), `entry_decision_source_inputs.rs` (`603eae90`), `fee_behavior_source.rs` (`0a580872`), `fees.rs`, and the flatness gate (`34df9eaf`) carry **no `cherry picked from` trailer**, authored==committed, native on `goal/024`. They share SHAs with `t043a`/`490` branches by **common ancestry** (forked working branches), not cherry-pick. They are inside PR #480's declared scope (plan.md + PR body T015–T024I / T036G). The **only** real cherry-picks (`git log --grep="cherry picked from"` = 23 commits) are disk-governance / host-clock / single-runner / cleanup work, and the #466 disk-governance diff nets to **zero** at HEAD (backed out per `scope-resolution.md`). **Action:** declare the provider surface in the PR body; verify the #466 net-zero holds. No split-out.

### 1.4 Selected live-trade path — what it actually is (decisive)

- **Production selected path = `polymarket_main` data + `polymarket_main` execution.** The strategy's reference comes from a **source-owned decision-reference provider**, NOT an NT data client and NOT the msgbus publish-topic. In `config/strategies/binary_oracle.toml`, `[reference_data]` is empty and `gate_subscriptions.{resolution,decision_reference}` resolve to `resolution_kind="chainlink_data_streams"`, `provider_id="resolution_oracle_primary"`. At runtime, `binary_oracle_edge_taker.rs:583-592` maps `target.gate_subscriptions.decision_reference` onto the strategy's `reference_venue` field by assigning `reference_venue = decision_reference.provider_id` (`"resolution_oracle_primary"`) and `reference_instrument_id = decision_reference.resolution_identity` — a **source-owned decision-reference provider id**, not an NT data client. The reference is therefore neither a Binance NT data client nor the publish-topic.
- **Binance and the other venues are broad-T043A readiness *probe* clients**, not selected-path clients. Their registration is proven by the **separate** all-configured build path `build_bolt_v3_all_configured_client_mapping_live_node_with_summary` (test `live_node_registration_can_load_all_requested_data_clients_without_extra_execution_clients`, `tests/bolt_v3_client_registration.rs:250`).
- The scoping change `0a8e0f8d` ("Scope live transport to selected strategy clients") is **architecturally correct**: NT requires every registered client to connect before `Running`, so the trade runner must register only strategy-bound clients. The two failing scoped-path tests (§1.2) encode the **removed** all-clients model → they are **stale**, not evidence of a regression.
- **No dual-path defect (R3 dropped — see §3).** `trade_transport_client_keys` (`bolt_v3_live_node.rs:2107`) derives scope from `execution_client_id` + `[reference_data].*.data_client_id` + `proof_policy.execution_client_id`. It **correctly** scopes only NT data clients. `reference_venue`/`reference_instrument_id` are populated at runtime from the source-owned `decision_reference` provider id (`binary_oracle_edge_taker.rs:583-592`) — **not** from a `[clients]` entry. Adding `reference_venue` to `trade_transport_client_keys` would wrongly require `resolution_oracle_primary` to exist as a registrable NT `[clients]` data client, which it is not. There is **no** single-source-of-truth violation here; the earlier "latent class gap" framing was wrong.

### 1.5 Docs stale vs HEAD

- `readiness-ledger.md` pins remote `8b95eca9` / local `135c0d09` (commit #105 of 120) — ~100 commits stale. T046 open.
- `tasks.md`: T039–T043 / T043B evidence recorded at `6a28cc7f`/`8b95eca9` (~51 commits stale); **T047 marked complete is CONTRADICTED** by the live clippy + allowlist failures; T044/T045/T046 correctly open.
- `data-adapter-production-readiness.md` is honest/fail-closed, but the line-7 "trade surface confirmed working on all three venues" is about the **probe** surface and must not be read as selected-path/T044 readiness.

### 1.6 Audit answers (the 7 questions)

1. **Complete at HEAD:** the provider-snapshot/entry/fee materializer surface (T015–T024I/T036G) compiles and is in-scope; the all-configured data-client mapping path + its test pass; the scoping change itself is correct.
2. **Stale because HEAD moved:** T043/T043B no-submit + final-packet evidence; readiness-ledger heads; the two scoped-path registration/adapter tests; T047 "complete".
3. **Contradicted by code/tests:** T047 complete (clippy+allowlist red); any implied "CI green"; the brief's "cherry-picked stack" framing; the reconcile-agent's "binance drop breaks selected path".
4. **Blocks selected-path T044:** CI red gates **merge** but NOT the canary binary itself (no crate-wide `deny(warnings)`; the literal verifier runs only in CI). The real selected-path preconditions are the live-canary gate + fresh exact-head no-submit + operator approval. Fixing the 3 tests + clippy + allowlist is required for an **honest green**, and the canary must run at a frozen, green head. **Caveat (see §4):** the T044 canary runs under `live_canary.proof_policy.enabled`, which makes `build_live_node_with_clients` (`bolt_v3_live_node.rs:3238-3260`) **skip** strategy registration and run the canary proof executor instead. That proof executor submits **one** fee-capped order via its own `order_intent` config (`bolt_v3_canary_proof_executor.rs:108-184`) — it proves the **submit/execution** path, not the `binary_oracle_edge_taker` decision→entry path. A green proof-policy canary must NOT be reported as production-strategy decision readiness.
5. **Blocks broad T043A:** Binance spot is openly deferred (credential/IP-gated); broad production-usability remains an explicitly open claim. Not required for selected-path T044.
6. **Blocks final production-grade readiness:** all of CI red + stale docs + missing exact-head packet/no-submit + T044/T045/T046.
7. **Shortest honest path:** fix code-side CI (clippy, 3 tests) → reconcile docs → freeze head → regenerate packet/no-submit at exact head → re-sync allowlist last → run one gated selected-path canary (with explicit live-submit authorization) → T045 → T046 → final CI.

## 2. Recovery task list (ordered)

**R1 — clippy (class: never declare CI green without running `cargo clippy --locked -- -D warnings`):**
- `operator_artifacts.rs:7837,:7857`: `&selected` → `selected`.
- `binary_oracle_edge_taker.rs:5972`: drop `u64::try_from(config.cadence_seconds).ok()?` → `config.cadence_seconds.checked_mul(MILLIS_PER_SECOND_U64)?` (field already `u64`); leave the `usize` try_from at :5978.

**R2 — selected-path scoped tests (retarget to the correct contract; do NOT un-scope):**
- `client_registration::...binance_data`: retarget to assert the **scoped** path registers exactly the strategy-bound set (`polymarket_main` data+exec) and does **not** pull in unbound broad clients. Rename to reflect scoped intent. Broad registration stays proven by the all-configured test (`...can_load_all_requested_data_clients...`).
- `adapter_mapping::...propagates...failures`: force the secret failure on the **in-scope** client (`polymarket_main`) so `SecretResolution` surfaces through the scoped build path (preserves the test's "mapping step cannot be skipped" intent).

**R3 — DROPPED.** (Was: fold `reference_venue`/`reference_instrument_id` into `trade_transport_client_keys`.) **Reason:** `reference_venue` is mapped at runtime from `target.gate_subscriptions.decision_reference` onto the provider id `"resolution_oracle_primary"` (`binary_oracle_edge_taker.rs:583-592`) — a **source-owned decision-reference provider**, NOT an NT data client. `trade_transport_client_keys` (`bolt_v3_live_node.rs:2107`) correctly scopes only NT data clients (`[reference_data].*.data_client_id`); adding `reference_venue` would wrongly require `resolution_oracle_primary` to exist as a `[clients]` entry. There is **no dual-path defect** and no class of problem to fix. See §3.

**R4 — canary-gate oversize desync:** derive both `largest_pre_consumption_operator_evidence_file_len()` and the gate's per-file max-bytes read accounting from **one shared list** of operator-evidence paths so adding a proof artifact cannot desync them; re-run the test.

**R5 — runtime-literal allowlist + FULL source-fence (LAST, after R1–R4 freeze the literal set):** in `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`, remove the 5 stale rows; add the remaining unclassified literals with reused `classification` + `reason` (Polymarket data-API params → a provider-scoped data-api classification, NOT `provider_credential_log_module`). Any literal that is a genuine runtime *value* gets a named constant instead (rule #1). **Do not stop at the runtime-literal self-test:** once `verify_bolt_v3_runtime_literals.py:136` passes, re-run the **entire `just source-fence`** target (~13 verifier pairs, including `verify_bolt_v3_provider_leaks.py`). Treat every downstream verifier as potentially **newly-failing** the moment line 136 passes — a clean self-test is necessary but not sufficient for a green source-fence lane.

**R6 — doc reconciliation to HEAD:**
- `tasks.md`: reopen/annotate T047 (cleanup not complete until CI green at HEAD; clippy + allowlist were red); mark T043/T043B evidence stale pending exact-head rerun.
- `data-adapter-production-readiness.md`: add explicit "probe-surface ≠ selected-path/T044 readiness" statement.
- `readiness-ledger.md`: refresh heads to the frozen final HEAD as part of T046.
- PR body: declare the provider-snapshot/entry/fee surface as in-scope; confirm #466 net-zero.

**R7 — focused verification + FULL source-fence:** `cargo clippy --locked -- -D warnings`; `cargo nextest run` for the 3 fixed tests + the all-configured test + no-submit suite; `python3 scripts/test_verify_bolt_v3_runtime_literals.py`; then the **full** `just source-fence` (all ~13 verifier pairs incl `verify_bolt_v3_provider_leaks.py`) and `just fmt-check` — not only the runtime-literal self-test.

**R8 — freeze head (Head-discipline, adopted verbatim from the closeout runbook):** commit R1–R6, push, record the exact final HEAD as `FINAL_HEAD`. Prove the packet / no-submit / canary **once at `FINAL_HEAD`**. T044/T045/T046 **evidence** commits land *after* the proofs; run final CI **once** at the very end. Do not re-prove at each evidence commit, and do not re-fire the canary after each fix.

**R9 — regenerate exact-head final packet + no-submit evidence** at `FINAL_HEAD` (`final-packet.md`, `final-no-submit.md`), replacing stale-head artifacts. **Precondition:** the no-submit requires AWS SSM + network + the EIP-allowlisted egress. The EIP **exists on the EC2 host**, so a fully-representative no-submit/canary must run **from EC2**, not from this dev worktree (`SP-MB-Pro.local`). Off-host, run the no-submit to the boundary it can honestly reach and record the off-host limitation; it does not substitute for the on-host proof.

**R10 — T044 selected-path proof canary (GATED, see §4):** exactly one run, only if all gates pass AND the operator explicitly authorizes a live submit. **Precondition:** must execute **from EC2 / the allowlisted EIP** (network + SSM + allowlisted egress). If run off-host, the operator declines, or no qualifying market exists, this becomes the **limited-green** path (§5). Record submit, venue order state, reconciliation, optional cancel. **Note:** the production strategy recurrently produces `no_side_selected` (both sides negative-EV), so a canary attempt may fail closed before submit with no fill — and **each approved canary attempt BURNS the one-time operator approval regardless of whether a fill occurred**. Under proof-policy this run proves the submit/execution path only (§4), not the decision→entry path.

**R11 — T045 post-run hygiene** (`post-run-hygiene.md`): flatten/cancel verification, artifact retention, no residual exposure.

**R12 — T046:** update `readiness-ledger.md`, issues #369/#385/#409/#360, and PR #480 body with `FINAL_HEAD`, hashes, T044/T045 outcomes, final CI status, and remaining scope. Bidirectional links.

**R13 — final CI, once** at the end. Record the exact `FINAL_HEAD` result in `evidence.md`.

## 3. R3 design decision — RESOLVED: no change (R3 dropped)

The earlier draft proposed folding `reference_venue`/`reference_instrument_id` into the same
source of truth as `[reference_data]` inside `trade_transport_client_keys`. **This is wrong and
R3 is dropped.** `reference_venue` is not a `[clients]` data-client key: it is populated at
runtime from `target.gate_subscriptions.decision_reference` onto the **source-owned** provider id
`"resolution_oracle_primary"` (`binary_oracle_edge_taker.rs:583-592`). `trade_transport_client_keys`
(`bolt_v3_live_node.rs:2107`) already has a single, complete source of truth over the things it
scopes — registrable NT data clients (`[reference_data].*.data_client_id`). Adding `reference_venue`
would require `resolution_oracle_primary` to exist as a registrable `[clients]` entry, which it is
not, and would **introduce** a defect rather than fix one. There is no dual-path / rule-#2 violation
here. No code change, no regression test, nothing tracked as follow-up for this item.

## 4. Live-trading safety (T044) — non-negotiable

- T044 fires a **real order on Polymarket with real funds**. Per global rule #10 and #9, a
  live submit requires **explicit operator confirmation**; "proceed" in one context does not
  carry over. The no-submit proof can run freely; the **live submit cannot** without a hard yes.
- **What the proof-policy canary actually proves (Codex HIGH).** With
  `live_canary.proof_policy.enabled`, `build_live_node_with_clients`
  (`bolt_v3_live_node.rs:3238-3260`) **skips** strategy registration
  (`BoltV3StrategyRegistrationSummary { registered: Vec::new() }`) and instead registers the
  **canary proof executor** (`register_canary_proof_executor_on_node`). That executor submits
  **one** capped limit order built from its own `order_intent` config, with the fee-inclusive cap
  computed in `bolt_v3_canary_proof_executor.rs:108-184`
  (`fee_inclusive_admission_notional` over the max-entry-fee bound). It does **NOT** run the
  `binary_oracle_edge_taker` decision→entry path. **Therefore a proof-policy T044 canary proves the
  SUBMIT / EXECUTION path with a single capped order — not the production-strategy decision→entry
  readiness.** A "green" canary must NOT be reported as proof that the production strategy's
  decision path is ready; that boundary is proven separately (decision evidence, no-submit, the
  gate session), not by a proof-executor fill.
- **Approval burns regardless of fill (recurring reality).** The production strategy recurrently
  returns `no_side_selected` (both sides negative-EV), so a canary attempt can fail closed before
  submit with **zero admitted orders and zero fills** — yet **each approved attempt still consumes
  the one-time operator approval**. Prior approved attempts (`9fa15005`, `7efad2cb`, `78a03da5`)
  burned approvals without a fill. Plan for this: one approval = one attempt, fill or not.
- Run **exactly one** canary per approval. Do not re-fire after each fix. Freeze head first; canary
  last. Run **from EC2 / the allowlisted EIP** (§9 precondition).
- Gates that must be green at the exact canary head before any submit: live-canary gate
  validation, fresh exact-head no-submit (all 7 stages), operator-approval consumption proof,
  submit-admission cap (single live order, fee-inclusive cap from `97da6236`).

## 5. Final verdict path

- **green:** ALL of — CI green at `FINAL_HEAD` (clippy + 3 fixed tests + full `just source-fence` incl `verify_bolt_v3_provider_leaks.py` + fmt) **AND** exact-head final packet + exact-head no-submit at `FINAL_HEAD` **AND** a T044 **submit-path** proof executed **on EC2 / the allowlisted EIP** (proof-policy canary proving the submit/execution path per §4 — not claimed as decision-path readiness) **AND** T045 post-run hygiene **AND** T046 ledger/issue/PR updates. Reminder (§4): the proof-policy fill proves the submit path, not the production decision→entry path.
- **limited-green (likely outcome from this worktree):** everything above green and complete EXCEPT the **live submit** — because the canary cannot run on EC2 from this dev worktree (`SP-MB-Pro.local` lacks the allowlisted EIP, §9), the operator declines the live submit, or no qualifying market exists (e.g. `no_side_selected`). The selected path is proven to the submit boundary via the exact-head no-submit and the live-canary gate; the live fill is deferred with the explicit reason recorded. CI + packet/no-submit + T045 + T046 must still all be green/complete for limited-green.
- **blocked:** any **code-side** gate cannot be made green honestly (clippy, the 3 tests, the canary-gate desync, the full source-fence), or a real selected-path regression is found. A red CI lane at `FINAL_HEAD` is not green and not limited-green.
