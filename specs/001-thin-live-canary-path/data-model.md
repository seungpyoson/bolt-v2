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

## LiveCanaryGateReport

Validated result from PR #305 gate.

Fields:
- approval id
- no-submit readiness report path
- max live order count
- max notional per order
- root max notional per order

Rules:
- produced before NT runner entry
- consumed by submit admission before every live order
- not a substitute for submit-time counters

## SubmitAdmissionState

Runtime state for the tiny-capital canary submit gate.

Fields:
- gate report
- admitted order count
- per-order cap
- strategy/evidence availability state

Rules:
- initialized only from a valid `LiveCanaryGateReport`
- rejects when order count is exhausted
- rejects when proposed order notional exceeds cap
- rejects when decision evidence persistence is unavailable
- must execute before NT submit

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
