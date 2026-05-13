# Data Model: Thin Bolt-v3 Live Canary Path

## BoltV3RuntimeConfig

TOML-backed loaded config used by the production entrypoint.

Fields:
- root config path and checksum
- `runtime`
- `venues`
- `strategies`
- `risk`
- `live_canary`
- `persistence`
- `nautilus`

Rules:
- loaded once before runtime build
- no environment fallback for runtime values
- no secret value storage

## ProviderBinding

Provider-owned registry entry.

Fields:
- provider key
- validation function
- supported market families
- SSM secret requirements
- secret resolver
- NT adapter mapper
- credential log filters

Rules:
- concrete provider keys live in provider binding modules
- core calls binding interface only
- adding provider must not alter core entrypoint or submit admission

## MarketFamilyBinding

Market-family-owned registry entry.

Fields:
- market family key
- validation function
- required instrument traits
- supported provider keys through cross-binding compatibility
- diagnostic function for unsupported family/provider combinations

Rules:
- concrete market-family keys live in market-family binding modules
- core calls binding interface only
- adding a market family must not alter core entrypoint, provider dispatch, or submit admission
- unsupported family/provider combinations fail closed with a family-owned diagnostic

## StrategyBinding

Strategy-owned registry entry.

Fields:
- strategy archetype key
- required reference-data roles
- parameter validator
- build function
- decision evidence requirement

Rules:
- concrete strategy internals live in strategy binding module
- construction fails closed if required evidence, reference roles, or parameters are absent
- strategy emits NT-native orders only through one admission boundary

## BoltV3LiveCanaryGateReport

Validated result from the bolt-v3 live canary gate.

Fields:
- approval id
- no-submit readiness report path
- max no-submit readiness report bytes: byte cap used before reading and parsing the readiness report
- max live order count: the canary-local order-count budget from `[live_canary]`
- max notional per order: the canary-local per-order cap from `[live_canary]`
- root max notional per order: the root risk ceiling from `[risk]`

Rules:
- produced before NT runner entry
- consumed by submit admission before every live order
- not a substitute for submit-time counters
- current source is `check_bolt_v3_live_canary_gate`; submit admission must not reparse `[live_canary]`
- readiness report read is bounded by `max_no_submit_readiness_report_bytes` before JSON parse
- canary-local notional must be less than or equal to the root risk ceiling
- prose field names map one-to-one to the existing `BoltV3LiveCanaryGateReport` values exposed by read-only accessors
- `max notional per order` is the canary-specific cap; `root max notional per order` is the global `[risk]` cap that bounds every canary cap

## SubmitAdmissionState

Runtime state for the tiny-capital canary submit gate.

Fields:
- gate report
- admitted order count
- internal mutex protecting gate report, armed flag, and count mutation
- per-order cap copied from `BoltV3LiveCanaryGateReport.max_notional_per_order`
- strategy/order/instrument labels for diagnostics
- strategy-supplied positive order notional

Rules:
- initialized only from a valid `BoltV3LiveCanaryGateReport`
- begins unarmed during build and arms only after live canary gate returns a valid report
- rejects admission while unarmed with a distinct `NotArmed` error
- rejects a second arm attempt so validated bounds cannot change under a running canary
- stale-arm means any arm attempt after one successful arm, including a different report; it rejects and does not mutate caps or count
- rejects when order count is exhausted
- rejects when proposed order notional exceeds cap; notional equal to cap admits
- rejects non-positive proposed notional
- decision evidence persistence must succeed before admission consumes order budget
- must execute before NT submit
- admitted order count means submit attempts accepted by admission, not venue accepts or fills
- the budget is global across all registered strategies, with no per-strategy partition
- entry submits, exit submits, and replace-submit paths consume the same count and notional budget
- plain cancel requests are not submits and do not consume admission budget
- admission budget is consumed before NT submit and is not refunded on NT submit error unless a later reviewed requirement changes this fail-closed rule
- decision evidence captures strategy intent before admission; an admission rejection can leave an intent record with no NT submit evidence
- a fresh process or fresh bolt-v3 live-node build creates a fresh unarmed state; restart reconciliation remains NT-owned and is not Phase 6 scope
- restart resets the in-memory admission budget; Phase 6 does not reconstruct prior admission count from NT cache or venue state

## NoSubmitReadinessReport

Redacted report from real SSM and real venue connect/disconnect, with zero orders.

Fields:
- config checksum
- SSM path identifiers only
- venue/client stage statuses
- NT connect/disconnect evidence
- timestamp and code manifest hash

Rules:
- no secrets
- no orders
- consumed by live canary gate
- stale or unsatisfied report rejects runner entry

## CanaryRunEvidence

Redacted artifact for the tiny-capital live canary.

Fields:
- approval id
- exact commit SHA
- config checksum
- SSM path identifiers only
- submitted order id and client order id
- NT order event facts
- venue accept/fill/reject facts
- strategy-driven cancel facts if order remains open
- restart reconciliation facts

Rules:
- one approved capped order maximum for MVP
- no credential values
- local mocks cannot populate live proof fields
