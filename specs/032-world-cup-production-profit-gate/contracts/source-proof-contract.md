# Contract: Source-Proof Admission

## Purpose

Validate that a World Cup event market is eligible for evidence capture before any strategy evaluates it.

## Inputs

### Event market proof

- official event source URL
- official event source sha256
- official event retrieval timestamp
- official event expiry timestamp
- venue market terms URL
- venue market terms sha256
- venue market retrieval timestamp
- resolution rule fields
- void/postponement/abandonment/settlement rule fields
- jurisdiction/account/product availability proof
- config checksum
- commit SHA

### Provider capability proof

- provider id
- transport class
- update semantics
- plan entitlement proof
- supported sports/leagues/books/markets
- historical tick support
- order-book-depth support
- latency/freshness proof
- rate-limit policy
- license scope

## Output

### Accepted

```json
{
  "status": "accepted",
  "state": "capture_eligible",
  "market_proof_hash": "sha256:<hash>",
  "provider_capability_hashes": ["sha256:<hash>"],
  "claim_class": "source_proven_capture_candidate"
}
```

### Rejected

```json
{
  "status": "rejected",
  "state": "source_proof_rejected",
  "reasons": [
    {
      "code": "venue_terms_missing",
      "field": "venue_market_terms_sha256"
    }
  ]
}
```

## Required Rejection Reasons

- `official_event_source_missing`
- `official_event_source_stale`
- `venue_terms_missing`
- `venue_terms_stale`
- `resolution_rule_missing`
- `resolution_rule_conflict`
- `provider_plan_missing`
- `provider_capability_stale`
- `provider_capability_insufficient`
- `direct_source_unproven`
- `aggregator_source_unlabeled`
- `jurisdiction_unavailable`
- `config_checksum_mismatch`
- `commit_sha_missing`

## Pinnacle Handling

- Direct Pinnacle classification requires direct API/license/rate-limit proof.
- Aggregator-sourced Pinnacle remains aggregator-sourced even if the bookmaker id is Pinnacle.
- Missing direct proof cannot be waived by strategy code.

## Non-Goals

- No orders.
- No cancels.
- No fund movement.
- No runtime secret resolution outside AWS SSM.
- No TOML mutation.
- No inference from market names.
