# Provider Snapshot Hard-Stop Inventory

Purpose: identify production-readiness and live-operation hard stops that can be affected by one external provider observation. The invariant is market, venue, and account agnostic: a single provider snapshot may not falsely block readiness without configured confirmation, and persistent blocking state must still fail closed.

## Immediate Provider-Snapshot Readiness Gates

| Gate | Source | Hard-stop field | Current disposition |
| --- | --- | --- | --- |
| Venue account open orders | Configured execution venue open-orders API | `conflicting_open_orders_absent` | Confirmed with shared external snapshot policy before hard-stop; persistent open orders and confirmation fetch failures still block. |
| Venue account active positions | Configured venue/account position source | `preexisting_position_absent` | Confirmed with shared external snapshot policy before hard-stop; NT dust threshold remains the active-position boundary. |
| CLOB collateral balance/allowance | Configured execution venue balance/allowance API | `collateral_accounting_verified` | Initial fetch uses configured retries; low balance/allowance is confirmed before hard-stop; confirmation fetch is single-attempt per configured retry. |
| CLOB funding margin balance/allowance | Configured execution venue balance/allowance API | `funding_margin_covers_max_notional_plus_fees` | Same confirmed balance/allowance path as collateral accounting; persistent insufficiency still blocks. |

## Related Gates Classified Separately

| Gate | Source | Why not changed in this patch |
| --- | --- | --- |
| Entry-decision instrument and market book source proofs | Operator-approved bounded source inputs and configured source provenance | Already tracked by T036A/T036B source-input materialization; missing or mismatched source proof must fail closed rather than be auto-confirmed. |
| Price-to-beat feed configuration | Root TOML strategy/source configuration | T036D intentionally fails closed until the operator supplies an approved real feed id. |
| Egress identity | Operator-approved observed egress file and approved hash | T036F binds an approved observed file/hash; substituting a live laptop/public-IP probe would be incorrect. |
| Host clock / provider time source | Public time source materializer | Readiness evidence source, not an account/venue flatness gate; failures should stay explicit evidence failures. |
| Final packet verification | Local artifact and root-TOML verifier | T038 verifies exact artifact/root binding; it should not retry or soften invalid local provenance. |
| Final no-submit / tiny canary run evidence | Approved operational run outputs | T043/T044 are live/no-submit side-effect gates and remain explicit operator-approved execution steps. |

## Review And Test Evidence

- Claude adversarial review job `2b5835b2-84a9-4171-9b3e-138ab623a762` returned `APPROVE` with no blocking findings. Its non-blocking concerns were persistent-block coverage, confirmation-fetch-failure coverage, nested retry call counts, delay naming, and diff-only scope.
- The implementation addresses those concerns by adding explicit fail-closed tests and by using a non-nested single balance/allowance fetch for each confirmation retry.
- Focused verification: `cargo test --test bolt_v3_cli keeps_ -- --nocapture` passed: 4 passed, 0 failed.

No AWS, SSM secret read, no-submit run, venue submit/cancel, transfer, root TOML mutation, or live trade was run for this inventory/fix.
