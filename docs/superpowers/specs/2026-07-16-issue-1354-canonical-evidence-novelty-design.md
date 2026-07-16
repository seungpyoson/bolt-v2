# Issue 1354 Canonical Evidence Novelty Design

## Scope

Repair the finite-novelty tracer bullet for entry-skip and blocked-RV strategy-input evidence. Submit-linked strategy-input snapshots, persistence, retirement, recovery readers, and other producers remain unchanged and tracked by #1354/#1385.

## Design

The TOML registry assigns permanent canonical IDs inside the frozen market-family ranges. The sixteen supported entry-skip reasons own IDs `240..255`. The twelve combinations of RV gate result and watermark presence own IDs `144..155`. Other market IDs remain reserved for their frozen families. Generated Rust exposes typed canonical IDs; runtime callers cannot submit arbitrary state values.

Novelty is stored as a per-`EvidenceEpisodeId` fixed bitset with no eviction or input-driven reset. Switching episode A to B and back to A therefore preserves A's prior claims. Capsule-backed retirement and durable empty/reuse barriers remain future scope.

`EvidenceEpisodeId` is constructed from the complete stable market binding: logical strategy, target, venue, Gamma market, condition, question, negative-risk mode, and exactly two ordered outcomes containing index, normalized label, and exact CLOB token ID. Prices, timestamps, slugs, diagnostics, configuration identity, and transient availability remain excluded.

Entry blockers, source diagnostics, availability, and other observation details remain payload. They do not create dynamic canonical IDs. Unknown or unassigned IDs fail before payload construction; duplicates skip payload construction and append. Writer failures remain claimed to prevent retry storms.

## Verification

- Registry tests prove the frozen ranges, permanent numeric mappings, generated-byte determinism, and rejection of unknown or duplicate IDs.
- Rust regressions prove episode A-to-B-to-A suppression, no-eviction behavior beyond the retired ceiling, duplicate suppression before payload construction, and capacity independence from arrival order.
- Identity tests prove every required stable component changes the episode ID and forbidden volatile components cannot enter its constructor.
- Existing producer tests prove entry-skip and blocked-RV mappings, fail-closed unknown handling, and the untouched direct submit-linked snapshot path.
- Allowed local static, formatting, workflow, and source-fence checks run before exact-head remote Rust verification.

## Non-Goals

No schema migration, Capsule persistence, episode retirement, archive changes, recovery changes, trading behavior, deployment, or live operation.
