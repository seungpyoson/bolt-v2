# Contract: Thin Boundary

## Bolt-v3 Owns

- TOML schema and loaded config shape
- SSM-only secret resolution and redaction
- provider, market-family, and strategy registration
- strategy decision policy and reference-role validation
- pre-submit admission gates
- compact audit evidence for Bolt-derived decisions

## NautilusTrader Owns

- runtime event loop
- venue adapters and wire translation
- market data protocols
- order lifecycle
- execution engine
- cache semantics
- portfolio/account/order/fill/balance/exposure state
- reconciliation

## Forbidden In This Feature

- Bolt-side order lifecycle state machine
- Bolt-side reconciliation engine
- mock venue world as live-readiness proof
- adapter behavior fork in Bolt
- NT cache semantics fork in Bolt
- hardcoded provider, market, strategy, ID, notional, or timeout in core
- alternate submit or runner path

## Review Rule

Any diff adding lifecycle, reconciliation, adapter behavior, cache semantics, or concrete provider strategy leakage to core is a finding, even if tests pass.
