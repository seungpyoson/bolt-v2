# ETH Anchor Semantics Experiment

## Objective

Test whether the mechanical delivery process blocks ambiguous ETH anchor semantics before implementation.

## Seam Under Test

- strategy field: `active.interval_open`
- market anchor candidate: `market.price_to_beat`
- fallback sources observed in code: `best_healthy_oracle_price(snapshot)`, `snapshot.fair_value`
- staleness input: `last_reference_ts_ms` guarded by `forced_flat_stale_chainlink_ms`

## Hypothesis

If the process works, this seam will block at the seam/proof stage before any new implementation is admitted, because the current local code still permits mixed anchor semantics.

## Current Outcome

Blocked.

The experiment already found an open `semantic_ambiguity` blocker:

- `interval_open` is allowed to derive from Polymarket anchor, healthy oracle price, or fused reference fair value
- the local tests encode that fallback as acceptable
- the public Polymarket market page exposes an exact `priceToBeat`
- the public Gamma event payload sampled here does not expose a matching anchor field

That is exactly the class of ambiguity the process is supposed to stop early.
