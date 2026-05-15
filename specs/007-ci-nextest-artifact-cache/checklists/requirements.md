# Requirements Checklist: CI Nextest Artifact Cache

**Purpose**: Validate that #195 requirements from the issue body, #333 epic boundary, and #332 sharded base are fully represented.
**Created**: 2026-05-15
**Feature**: [spec.md](../spec.md)

## Completeness

- [x] CHK001 Does the spec preserve the #195 core outcome: warm reruns avoid unnecessary `Compiling bolt-v2` test-profile rebuilds? [FR-002, SC-003]
- [x] CHK002 Does the spec adapt to #332's four nextest shards because #332 landed first in this stack? [FR-003, Assumptions]
- [x] CHK003 Does the spec require exact cold/warm run IDs and log excerpts before closure? [FR-013, FR-014, SC-004]
- [x] CHK004 Does the spec require cache archive size and pruning/eviction evidence? [FR-016, SC-005]
- [x] CHK005 Does the spec keep missing/stale cache behavior as a correct cold test run? [FR-008, SC-006]

## Boundary

- [x] CHK006 Does the spec avoid taking over #332 lane topology beyond adapting to its current stack? [FR-017]
- [x] CHK007 Does the spec keep #205 smoke-tag dedup out of scope? [FR-017]
- [x] CHK008 Does the spec keep #344 docs/pass-stub/evidence work out of scope? [FR-017]
- [x] CHK009 Does the spec keep #340 config relocation out of scope? [FR-017]
- [x] CHK010 Does the spec preserve #203 verifier use only for #195-specific cache invariants? [FR-011, FR-012]

## Cache Correctness

- [x] CHK011 Does the spec require managed target-dir preservation through the setup action? [FR-001]
- [x] CHK012 Does the spec require workspace artifact preservation rather than opaque target-dir caching only? [FR-002]
- [x] CHK013 Does the spec require real Rust input invalidation through rust-environment hashing? [FR-004, FR-006]
- [x] CHK014 Does the spec reject unbounded per-commit cache keys? [FR-005]
- [x] CHK015 Does the spec preserve one cache backend unless a strong reason is recorded? [FR-007]

## Gate Safety

- [x] CHK016 Does the spec keep `test` dependent on `source-fence`? [FR-010]
- [x] CHK017 Does the spec keep aggregate `gate` requiring successful `test`? [FR-009]
- [x] CHK018 Does the spec make cache status observational, not a substitute for test execution? [US4]

## Evidence Discipline

- [x] CHK019 Does the spec explicitly mark exact CI evidence blocked when stacked PR CI is unavailable? [US5]
- [x] CHK020 Does the quickstart list the exact commands/data needed to collect completion evidence? [SC-003..SC-005]
