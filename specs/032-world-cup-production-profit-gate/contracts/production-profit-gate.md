# Contract: Production Profit Gate State Machine

## States

- `source_proof_pending`
- `source_proof_rejected`
- `capture_eligible`
- `shadow_evidence_collecting`
- `evidence_rejected`
- `promotion_ready`
- `disabled_config_generated`
- `no_submit_ready`
- `tiny_canary_ready`
- `live_enabled`

## Transitions

### `source_proof_pending` -> `capture_eligible`

Requires accepted event-market source proof and provider capability proof.

### `source_proof_pending` -> `source_proof_rejected`

Occurs when required source, venue, provider, or jurisdiction proof is missing, stale, conflicting, or insufficient.

### `capture_eligible` -> `shadow_evidence_collecting`

Requires NT-backed data capture/replay path and accepted reference quorum.

### `shadow_evidence_collecting` -> `promotion_ready`

Requires accepted profit evidence session with candidate/no-trade/fill/markout/settlement evidence and configured thresholds.

### `shadow_evidence_collecting` -> `evidence_rejected`

Occurs when evidence class, fillability, markout, settlement, latency, fee, or quorum proof fails.

### `promotion_ready` -> `disabled_config_generated`

Requires disabled generated TOML bound to source proof, provider proof, profit evidence, commit SHA, and config checksum.

### `disabled_config_generated` -> `no_submit_ready`

Requires exact-head no-submit readiness and matching source/promotion hashes.

### `no_submit_ready` -> `tiny_canary_ready`

Requires explicit operator approval, current geography/account/product availability proof, source-fence pass, exact-head CI, and tiny-canary proof.

### `tiny_canary_ready` -> `live_enabled`

Requires a separate operator decision for the exact venue/account/product/market-family/config hash. This package does not grant that decision.

## Invariants

- No transition places, cancels, replaces, or transfers by itself.
- Promotion output starts disabled.
- Source proof and provider capability are data records, not provider-specific strategy branches.
- Runtime secrets are SSM-only.
- Live enablement is scoped to exact head, exact config, exact account, exact venue, and exact market family.
- Loss of provider quorum blocks new order intent.
- Strategy code remains intent-only.

## Failure Output

Each rejection emits:

- state
- reason code
- source artifact hash when available
- expected proof class
- observed proof class
- exact commit SHA
- config checksum
- redacted provider/account identifiers
