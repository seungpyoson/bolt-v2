# Production Trade Readiness - Closeout Runbook (T043B -> T044 -> T045 -> T046)

This is the operator runbook for the final sequence **after T043B succeeds**. It orders the
remaining Phase 8 gates, gives the exact command for each, and marks the steps that must not
run without fresh explicit operator approval.

**This runbook does not assert that readiness is complete.** It is the procedure to follow once
the T043B selected-trade-path proof lands. Readiness is the operator's call, made step by step
below against recorded evidence, not by this document.

## Scope of this lane

- **Owns:** the closeout sequence: final local verification, final CI, the T044 canary approval
  gate, T045 post-run hygiene, and the T046 readiness-ledger update.
- **Does not own:** T043B itself, the T043A multi-venue data-client matrix, or any hardcode
  cleanup. Those are tracked in [`tasks.md`](tasks.md) and worked in their own lanes.
- **Constraints (carried from [`plan.md`](plan.md) and project rules):** one readiness PR; SSM is
  the only secret source; no secret display; no hardcoded runtime values; no live / no-submit /
  trading operation runs until its prerequisites are proven at the exact head; operator approval
  does not bypass prerequisites.

## Current state (read before using this runbook)

- T043B selected-path packet/no-submit: **locally proven at `67523e6c`**, with
  `verify-final --verification-stage pre-run` passing and a no-submit report with all 7 stages
  satisfied. This runbook/docs commit moves `HEAD`, so Step 2 and Step 3 below still re-affirm the
  packet and no-submit proof at the eventual `FINAL_HEAD` before T044.
- T043A multi-venue matrix: **open**. The configured selected data/execution path is
  production-usable for the T044 canary path; remaining data-only venue rows gate the broad
  multi-venue claim, not the selected canary. See
  [`data-adapter-production-readiness.md`](data-adapter-production-readiness.md).
- **T043B: selected-path implementation closed locally; final-head re-affirm still required.**
  See [`tiny-canary.md`](tiny-canary.md). If any later commit changes code, config, or bound
  evidence, rerun the Step 2/Step 3 pre-run proofs instead of treating older artifacts as current.
- T044 / T045 / T046: **open**, gated as in [`tasks.md`](tasks.md) Phase 8.
- Last exact-head verification baseline (now superseded by T043B head movement): local and
  CI evidence were green at PR #480 head `8b95eca9c2f410ff462954cff90c4734d01593cb`
  ([`evidence.md`](evidence.md) T041).

---

## Entry gate - artifacts required before T044

Before T044, the main readiness lane (worktree `.worktrees/024-production-trade-readiness`,
branch `goal/024-production-trade-readiness`, PR #480) must have committed and recorded all of the
following at one **committed** head. Record that head and treat it as `FINAL_HEAD` for every step
below:

- **`FINAL_HEAD`**: the committed code head the canary binary is built from
  (`git rev-parse HEAD`). All later commits in this lane move past it; see *Head discipline*.
- **Root TOML + sha256**: the approved ignored `config/live.local.toml` and its sha256.
- **Fresh source-bound decision chain** at `FINAL_HEAD`: `entry-decision-source.json`,
  `instrument-source.json`, `fee-rate-source.json`, decision-evidence JSONL,
  `market-selection-source.json`, `strategy-input.json`, `entry-readiness-gate-session.json`
  (+ sha256s).
- **Regenerated final packet** at `FINAL_HEAD` (paths + sha256, recorded in
  [`final-packet.md`](final-packet.md)): `ssm-manifest.json`, `financial-envelope.json`,
  `approval-nonce.json`, `pre-run-state.json`, `abort-plan.json`, operator-evidence JSON,
  `static-artifacts-manifest.json`, `approval-envelope.json`, and the operator-evidence packet.
- **`verify-final --verification-stage pre-run` PASS** at `FINAL_HEAD` against that root TOML
  and packet (command + result recorded).
- **Fresh no-submit report** at `FINAL_HEAD` (path + sha256, 7 stages satisfied) recorded in
  [`final-no-submit.md`](final-no-submit.md).
- **T043A selected row = production-usable** for the configured canary path,
  recorded in [`data-adapter-production-readiness.md`](data-adapter-production-readiness.md).

If any item is missing, stop. T044 cannot be approved from stale or partial evidence.

---

## Head discipline (applies to every step)

The operator packet binds `head_sha` to the **code build head**, and the no-submit report binds
`config_bundle_checksum` + `executable_identity` to that head. **Any commit, including a
docs-only commit in this lane, changes `git HEAD`.** Therefore:

1. Land all code + pre-canary evidence commits **first**, including this runbook and any Step 1-3
   evidence. Pick the resulting head as `FINAL_HEAD`.
2. Regenerate + `verify-final` the packet, run no-submit, run CI, and run the canary **at
   `FINAL_HEAD`**.
3. T045 / T046 evidence is produced **after** the canary; committing it is the final tree change.
   Run the **single** final CI check after those commits (see Step 4 timing). Do **not** re-run
   `verify-final` / no-submit / canary to chase a docs-only head bump; the canary was already
   proven at `FINAL_HEAD`. This is the convention recorded in [`final-packet.md`](final-packet.md)
   and the [`tasks.md`](tasks.md) MVP note ("Final GitHub CI should be checked once at the end ...
   not after every docs-only update").

---

## Step 1 - Final source-fence / local verification (re-affirm T039 + T040 at `FINAL_HEAD`)

T040 was green at the old head; T043B moved the head, so re-run at `FINAL_HEAD`. Record the
output summary in [`evidence.md`](evidence.md).

```bash
git rev-parse HEAD                                   # must equal FINAL_HEAD
git status --short --branch                          # tree must be clean
cargo fmt --check
git diff --check
python3 scripts/test_verify_bolt_v3_runtime_literals.py   # verifier self-test
python3 scripts/verify_bolt_v3_runtime_literals.py        # runtime-literal audit -> "OK: ..."
just clippy
just source-fence        # runtime-literal/provider-leak/core/naming/status/schema/pure-Rust/
                         # default/strategy-policy/source-capture fences + controlled-connect +
                         # production-entrypoint tests; also runs cargo fetch --locked
```

Focused readiness suites (T039; the live-relevant tests):

```bash
cargo test --locked \
  --test bolt_v3_operator_artifacts \
  --test bolt_v3_tiny_canary_preconditions \
  --test bolt_v3_tiny_canary_operator \
  --test bolt_v3_live_canary_gate \
  --test bolt_v3_cli -- --nocapture
```

**Pass criteria:** `fmt --check` clean; `git diff --check` clean; verifier prints
`OK: Bolt-v3 runtime literal audit passed.`; `just clippy` and `just source-fence` pass; all
focused suites pass (0 failed; the one live operator-harness entrypoint test stays `ignored`).

---

## Step 2 - Confirm the final packet verifies at `FINAL_HEAD`

This is the gate the canary consumes. Confirm it here against the exact root TOML and head.
Assembly recipe (generate-base-static -> generate-operator-evidence-json ->
update-operator-evidence-toml -> write-manifest-from-operator-evidence -> assemble-final) is
recorded step-by-step in [`tiny-canary.md`](tiny-canary.md) "Latest Non-Live Preflight". The
verification command is:

```bash
cargo run --locked --bin bolt-v2 -- operator-artifacts verify-final \
  --config config/live.local.toml \
  --operator-packet <FINAL_PACKET>.json \
  --verification-stage pre-run
```

**Pass criteria:** PASS, verifying approval-envelope, operator-evidence-packet, and
static-artifacts-manifest hashes. A stale packet fails closed (e.g. `head_sha does not match
build head_sha` or `config_bundle_checksum does not match loaded config`); that means the packet
predates `FINAL_HEAD` and must be regenerated by the T043B lane, not worked around. Record paths +
hashes in [`final-packet.md`](final-packet.md).

---

## Step 3 - Final-packet no-submit at `FINAL_HEAD` (re-affirm T043)

No-submit must be proven at the exact final head. It connects the configured data and execution
clients, reconciles account state, expects zero orders/fills/positions, stops the
runner, writes the report. **No order is submitted, cancelled, transferred, or settled.**

```bash
cargo run --locked --bin bolt-v2 -- no-submit-readiness --config config/live.local.toml
```

**Pass criteria:** report at `var/bolt-v3-live/reports/no-submit-readiness.json`, schema
`bolt-v3.no-submit-readiness.v2`, **all 7 stages satisfied**: `operator_approval`,
`secret_resolution`, `live_node_build`, `controlled_connect`, `reference_readiness`,
`controlled_disconnect`, `report_write`. Record report path + sha256 + the no-side-effects
statement in [`final-no-submit.md`](final-no-submit.md). Confirm `config_bundle_checksum` and
`executable_identity` correspond to `FINAL_HEAD`.

---

## Step 4 - Final GitHub CI (once, at `FINAL_HEAD`) - re-affirm T041

**Timing:** check CI **once**, at the end, after the last code/evidence commit that sets
`FINAL_HEAD`, not after every docs update. Do **not** run CI before the final local pre-canary
evidence head is selected.

```bash
git push                                  # push FINAL_HEAD to origin/goal/024-production-trade-readiness
gh pr view 480 --json number,headRefName,headRefOid,baseRefName,baseRefOid,statusCheckRollup,url
```

**Pass criteria:** `headRefOid` equals `FINAL_HEAD`; all required checks succeed:
`detector`, `Analyze (actions)`, `actionlint`, `Analyze (rust)`, `fmt-check`, `deny`, `clippy`,
`check-aarch64`, `source-fence`, `nextest archive`, `build`, `nextest shard {1,2,3,4} of 4`,
`test`, `gate`, `CodeQL`. `same-sha-main-evidence` and `deploy` are expected **skipped**. Record
the run/PR view in [`evidence.md`](evidence.md).

---

## Step 5 - T044 tiny-capital canary - REQUIRES FRESH EXPLICIT OPERATOR APPROVAL

> **HARD STOP. This is the only live-capital step.** It may submit at most **one** live order.
> Do not run it on a standing/previous approval. The preflight approval window is **not reusable**
> ([`tiny-canary.md`](tiny-canary.md)). Proceed only after the operator gives fresh explicit
> approval for this specific head.

Configured bounds are config-owned and must be present and unchanged in the approved root TOML:

- `max_live_order_count = 1`
- `max_notional_per_order = "1.00"`

On fresh approval, the operator (not this runbook) performs:

1. Refresh the approval window in the **real ignored** `config/live.local.toml`
   (`approval_not_before_unix_seconds` / `approval_not_after_unix_seconds` / approval nonce) for
   `FINAL_HEAD`, then regenerate operator-evidence JSON + packet to bind the fresh approval.
2. Re-run **Step 2** `verify-final --verification-stage pre-run`; it must PASS at `FINAL_HEAD`.
3. Launch the live canary:

```bash
cargo run --locked --bin bolt-v2 -- run --config config/live.local.toml
```

**Outcomes:**
- **Blocked-before-submit** (for example, `entry_gate_blocked` from `IntervalOpenMissing` /
  `WarmupIncomplete` / `FeesNotReady` / `ActiveBookNotPriced`, or stale-strategy-input rejection):
  produce blocked-before-submit evidence with **no live order refs**; T044 stays open. This has
  happened before; see [`tiny-canary.md`](tiny-canary.md) "Failed Closed Before Submit".
- **Successful canary:** produce the live proof refs required by
  `Phase8CanaryEvidence::live_canary_proof`: `live-run/canary-evidence.json`,
  `nt-submit-event.json`, `venue-order-state.json`, `restart-reconciliation.json`,
  optional cancel proof, and `post-run-hygiene.json`. Then run `verify-final` post-run stage over
  the bound post-run artifacts.

Record the attempt (head, artifact root, outcome, exact side-effects statement) in
[`tiny-canary.md`](tiny-canary.md). **Never** mark T044 done from a blocked attempt.

---

## Step 6 - T045 post-run hygiene + evidence retention (requires T044 complete)

Run an artifact/log secret scan over the T044 output directory and configured post-run report
paths, write the hygiene proof, and record the retention/purge decision in
[`post-run-hygiene.md`](post-run-hygiene.md).

The hygiene proof must satisfy the `phase8_assert_post_run_hygiene_proof` contract:

- `record_kind = post_run_hygiene`; `run_id` matches the T044 run id.
- `strategy_instance_id_hash`, `client_order_id_hash`, `venue_order_id_hash` match the approved
  canary hashes.
- `raw_secret_residue_absent = true`.
- `scanned_artifact_hashes`: non-empty list of sha256s for scanned artifacts/logs.
- `retention_purge_path_hash`: sha256 of the retained/purged artifact path record.

**Scan must prove:** no API keys, private keys, passphrases, approval ids, non-redacted balances,
or raw secret material in retained artifacts/logs; retained set is the minimum needed for
final-packet verification and issue/PR evidence; any purge decision recorded **by hash/path-hash**,
never by printing secret-bearing paths or contents.

**Evidence retention policy:** keep only the minimum artifact set required to verify the packet and
back the issue/PR evidence; everything else is purged and recorded by hash. Live-run artifacts live
under the temp/canary artifact root and `var/bolt-v3-live/`; do not commit secret-bearing or
non-redacted artifacts into the repo; commit only redacted evidence files (paths + sha256s + status).

---

## Step 7 - T046 readiness ledger (requires T044 + T045)

Update the targets below with the exact final readiness status, then record the links in
[`readiness-ledger.md`](readiness-ledger.md). **Do not** post final-readiness claims before T044
and T045 are complete.

| Target | Update with |
| --- | --- |
| #369 | Final readiness disposition, `FINAL_HEAD`, final packet hashes, T044 result, T045 result, remaining/no-remaining scope. |
| #385 | Exact no-submit evidence (Step 3 / [`final-no-submit.md`](final-no-submit.md)) and the tiny-canary relationship from T044. |
| #409 | PortfolioSnapshot acceptance evidence from [`issue-409-portfolio-snapshot.md`](issue-409-portfolio-snapshot.md) + final CI/readiness status. |
| #360 | Final cross-reference to T043/T044/T045 evidence (already closed). |
| PR #480 | Completed task list, final hashes, final CI status/URL, and issue links. |

Bidirectional links required: each issue update references PR #480 and the relevant evidence file,
and PR #480 references each issue. T043A remains open for the broad multi-venue data-client claim
unless every requested data-only row is production-usable or explicitly dispositioned; state that
plainly; do not let it silently close.

---

## What must NOT run without explicit operator approval

- **`bolt-v2 run` against the live config (the T044 canary).** Live capital. Fresh, head-specific
  approval each time; one order, within the configured notional cap. A previous/preflight approval
  is never reusable.
- **Any order submit, cancel, transfer, CLOB allowance/cache mutation, or on-chain state mutation.**
  None of these happen in Steps 1-4; the canary is the only path that can, and only within bounds.
- **Marking T044 complete from a blocked attempt**, or marking T045/T046 done before T044 succeeds.
- **Printing or committing secrets**: API keys, private keys, passphrases, approval ids,
  non-redacted balances. Evidence is recorded by hash/path-hash only.
- **Re-running `verify-final` / no-submit / canary to chase a docs-only head bump.** Prove once at
  `FINAL_HEAD`; do not re-open a proven gate for a documentation commit.

Steps 1-4 are non-live verification and may run without trade approval, but Step 4 should still be
held until the final local pre-canary evidence head is selected. Step 5 is the approval boundary.

---

## At-a-glance checklist

- [ ] Entry gate: required pre-canary artifacts present at one committed head -> set `FINAL_HEAD`.
- [ ] Step 1: `cargo fmt --check`, `git diff --check`, runtime-literal verifier + self-test,
      `just clippy`, `just source-fence`, focused readiness suites all pass at `FINAL_HEAD`.
- [ ] Step 2: `operator-artifacts verify-final --verification-stage pre-run` PASS at `FINAL_HEAD`.
- [ ] Step 3: `no-submit-readiness` re-run at `FINAL_HEAD`; 7 stages satisfied, report sha256 recorded.
- [ ] Step 4: single final CI; `gh pr view 480 ... statusCheckRollup` green; `headRefOid == FINAL_HEAD`.
- [ ] Step 5: **fresh operator approval** -> refresh approval window -> re-verify -> `bolt-v2 run` canary; record outcome.
- [ ] Step 6: post-run hygiene scan + proof + retention/purge decision recorded.
- [ ] Step 7: #369 / #385 / #409 / #360 / PR #480 updated; links recorded; T043A residual scope stated.
