# Internal Adversarial Review

## Verdict

Approve as a production-profit gate package. Reject as capital authorization.

## Review Findings

### Finding 1: The package could overstate profit from source proof

**Risk**: Source proof says a market is eligible, not profitable.

**Disposition**: Addressed. The state machine separates `capture_eligible`, `promotion_ready`, `no_submit_ready`, and `tiny_canary_ready`. Source proof alone cannot promote capital.

### Finding 2: Provider names could leak into strategy logic

**Risk**: Handling OpticOdds, SportsGameOdds, Pinnacle, Polymarket, and Kalshi by name would create dual paths.

**Disposition**: Addressed. Provider behavior is represented by capability records, source labels, and TOML-owned roles. Strategy logic remains provider-neutral.

### Finding 3: World Cup rules might be encoded prematurely

**Risk**: Hardcoding tournament details would become stale and violate repo rules.

**Disposition**: Addressed. The package requires official source artifacts and hashes, with no Rust constants for tournament or market-resolution rules.

### Finding 4: Direct Pinnacle assumptions are unsafe

**Risk**: Aggregator odds could be treated as direct bookmaker data, overstating latency and rights.

**Disposition**: Addressed. Direct classification requires direct proof; aggregator-sourced odds remain labeled.

### Finding 5: Polymarket availability could be ignored

**Risk**: If venue geography or account status blocks trading, the strategy cannot run production capital there.

**Disposition**: Addressed. Geography/account/product availability is a hard live enablement gate.

### Finding 6: Historical profit could be low fidelity

**Risk**: REST snapshots or stale odds can produce false profit.

**Disposition**: Addressed. Fidelity class is explicit. Only accepted L2/order-book replay through the NT-backed path can support execution-quality claims.

### Finding 7: Promotion might be mistaken for live enablement

**Risk**: A disabled config package could be treated as approval.

**Disposition**: Addressed. Promotion creates disabled TOML only. No-submit, canary, operator approval, and legal/geography gates remain separate.

### Finding 8: Spec Kit pointer drift could break source-fence

**Risk**: Updating `.specify/feature.json` or `AGENTS.md` would conflict with current source-fence assumptions.

**Disposition**: Addressed. Both pointers must remain pinned to the guarded 023 plan.

## Residual Risks

- Provider commercial terms and plan entitlements must be refreshed before purchase or production use.
- Legal/geographic venue availability requires operator/legal confirmation, not model inference.
- Profit thresholds and capital caps must be defined in TOML during implementation and reviewed before canary.
- World Cup official regulation artifacts must be captured from authoritative sources at run time.

## Approval Boundary

Approved for implementation planning and future non-live gate implementation. Not approved for live trading, production capital, provider purchase, or bypassing no-submit/canary gates.
