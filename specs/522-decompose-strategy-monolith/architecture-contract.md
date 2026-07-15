# Decomposition Architecture Contract — bolt-v3 monoliths (#522)

**Purpose:** the rules every slice (Track A + Track B) follows so we cut along the
right lines and name things consistently. **Behavior-preserving moves only** — no logic
or economics changes in this phase.

## 1. Module boundary map — which layer does each piece belong to?

| Layer | Owns | Lives in |
|---|---|---|
| **Strategy** | *what* to trade — signals, intent, entry/exit decisions, strategy-local config/schema, and orchestration glue. Phase-1 may use strategy-local concern modules as an intermediate move, but submit mechanics must still leave the strategy. | `strategies/…` |
| **Shared execution/admission** | *whether & how* to execute — **order construction, quantity-normalization**, rounding, fee-adjustment, sizing, admission-request construction + valuation, submit-gating, the **submit wrapper** | `bolt_v3_*` modules |
| **Market-family** | family-specific mechanics — identity, target validation, family pricing, the family registry | `bolt_v3_market_families/…` |
| **Core/shared util & glue** | family-agnostic numeric/util, shared types, the error enum, constants, JSON I/O | dedicated shared files (e.g. `bolt_v3_numeric.rs`); only *truly common* glue stays in a `mod.rs` (don't let `mod.rs` become a dumping ground) |

**How to read the slice tables (important):** a "Track A target" names a concern to
**extract OUT OF** the single strategy file. The destination layer is given by the target
module name and by the active slice plan:

- `strategies/binary_oracle_edge_taker/*.rs` = strategy-local concern module, used for
  behavior-preserving monolith shrinkage while keeping the existing strategy surface.
- `bolt_v3_*` = shared/core/execution layer.
- `bolt_v3_market_families/*` = market-family layer.

So "extract `book_sizing` -> `bolt_v3_book_sizing.rs`" means it moves out of the
strategy into the shared execution layer. "Extract `selection` -> `selection.rs`" means
it moves out of the single strategy file into a strategy-local submodule first; future
family/shared generalization must be a separate named slice, not hidden inside A3.

Per-target destination + layer (Track A):

| Target → module | Destination layer |
|---|---|
| `numeric` → `bolt_v3_numeric.rs` | core/shared util |
| `sizing` → `bolt_v3_sizing.rs` | shared dollar-intent sizing math |
| `taker_updown_signal` → `bolt_v3_taker_updown_signal.rs` | taker/updown EV, side-selection, and uncertainty math |
| `selection` -> `strategies/binary_oracle_edge_taker/selection.rs` | strategy-local concern module: candidate/selection snapshot construction and venue-routing predicates move out of the single file. This completes the planned A3 monolith shrink, not a shared/family generalization. Family-owned identity/target validation remains in `bolt_v3_market_families/*`; route-by-instrument-identity and any shared/family split of `CandidateMarket` are separate future work, not hidden in A3. |
| `book_sizing` → `bolt_v3_book_sizing.rs` | shared execution (book state, VWAP/slippage sizing, **rounding + fee-adjustment**) |
| `taker_pricing` → `bolt_v3_taker_pricing.rs` | shared pricing-state |
| `exposure` → `strategies/binary_oracle_edge_taker/exposure.rs` | strategy-local concern module: exposure/recovery state moved out of the single file in A6. Any later shared position-accounting split must be a separate named slice because the accepted A6 scope was a behavior-preserving monolith shrink, not a shared-layer generalization. |
| `source_proof` → `strategies/binary_oracle_edge_taker/source_proof.rs` | strategy-local concern module: source-proof / replay / evidence derivation moves out of the single file in A7. The replay path instantiates `BinaryOracleEdgeTaker` and uses strategy-local state, so moving it into shared code would require signature/boundary changes outside a pure move. Any shared evidence/replay generalization is separate future scope. |
| `config` -> `strategies/binary_oracle_edge_taker/config.rs` (or an explicitly approved archetype module) | strategy-local config schema + parse/validate. Root/global config machinery stays in `bolt_v3_config.rs`; do not move strategy-specific TOML schema into shared core just to shrink the monolith. |
| `submit_admission` → `bolt_v3_submit_admission.rs` | shared execution/admission — **owns order construction, quantity-normalization, admission-request construction + valuation, and the submit wrapper. None of these may remain strategy-resident.** |

Track B targets land in `bolt_v3_operator_artifacts/` (see §6).

## 2. Dependency direction — one way only (and enforced)

- Strategy **may** use shared + family modules.
- Shared / family modules **must never import the strategy** — no back-references, no
  strategy types.
- **No cycles.** If A needs B and B needs A, the boundary is wrong — pull the shared
  piece out.
- Litmus test: *"if I deleted the strategy, would the shared modules still compile?"*
  They must.
- **Enforcement (required — this rule is NOT honor-system):** the three existing fences
  do **not** catch a shared module doing `use crate::strategies::…`. Land the
  dependency-direction fence (`scripts/verify_bolt_v3_dependency_direction.py`, PR #546)
  before merging the first code slice that relies on this contract. It must be wired into
  the source-fence CI lane and fail on any `crate::strategies` import or strategy-type
  reference inside `bolt_v3_*` and `bolt_v3_market_families/*`.
  The strategy-owned archetypes now live under `src/strategies/*/archetype.rs`, and
  live-node registration resolves through `src/strategy_bindings.rs`; production
  `src/bolt_v3_*` modules therefore have no strategy-layer back-references. The fence's
  `FINDING_ALLOWANCES` tuple is empty and must stay empty. The allowlist may **only
  shrink**; **no new entry may ever be added**.
  Normal source-fence execution mechanically compares the in-tree allowlist with the
  protected `origin/main` baseline using `--check-shrink-only-vs-main` and requires the
  current entries to be a subset. The fence — not a manual grep — is the gate.

## 3. Naming convention

- **Type, function, and field NAMES in core/shared code are family-agnostic** — no
  concrete family word (`Updown`, `Polymarket`) in those identifiers. *(Enforced by
  `scripts/verify_bolt_v3_provider_leaks.py`; note "provider" ≈ "family" in the script's
  name — rename tracked. Example already applied: the side type is `OutcomeSide`, not
  `UpdownOutcomeSide`.)*
- **Module declarations and the family registry are EXEMPT.** The concrete family word
  legitimately appears throughout the `bolt_v3_market_families/` directory — `mod.rs`
  must contain `pub mod updown;` plus the registry/validation binding table, and
  `updown.rs` is the family's own module. The rule restricts *agnostic-layer identifiers*,
  not module paths or registration keys.
- **Evidence/field names use `market_*`, never `polymarket_*`** — existing `polymarket_*`
  fields in shared evidence are debt to rename (tracked W2-2 / findings-doc #12).
- **Don't collide with NautilusTrader-owned names** — source of truth
  `docs/bolt-v3/research/naming/nt-owned-name-audit.yaml`, enforced by
  `scripts/verify_bolt_v3_naming.py`.

## 4. Acceptance — every slice must hold

- **Behavior-preserving:** move + imports only; no logic/branch/number change. **Minimal
  signature-only changes are permitted *solely* to break a dependency cycle**, provided no
  branch semantics or numeric values change.
- **Visibility minimality:** widen visibility only as far as the new boundary requires —
  prefer `pub(crate)`; add `pub`/re-export only where an external caller or test needs it.
  Gratuitous `pub`-widening is a finding.
- **Boundary actually moves:** the declared symbol cluster leaves the original
  monolith, exists exactly once in its declared owner module, and preserves the
  original callable surface. Size deltas may be reported as telemetry, but are not
  acceptance proof.
- **Dependency check passes:** the §2 dependency-direction fence is green. It lands before
  A3/A8 merge (with its frozen pre-existing allowlist); no later slice merges without it.
- **Tests:** carried to the new home (RED→GREEN); CI green at exact head; all fences +
  the runtime-literal allowlist pass.
- **Bugs found during a move are ticketed and fixed in a separate follow-up PR** — never
  slipped into a "behavior-preserving" move commit.

## 5. Out of scope (this phase)

No logic/economics changes, no new features, no venue/secret changes. The deeper "route
by instrument identity instead of an Up/Down enum in core" is a **future** item
(findings-doc #13), not a decomposition slice.

## 6. Track B concern modules (operator_artifacts)

Decompose `bolt_v3_operator_artifacts.rs` into one module per concern under
`bolt_v3_operator_artifacts/`. **Concern-local JSON I/O and constants move WITH the
concern**; only truly shared error/re-export/writer glue stays in `mod.rs`.
**Secret-handling and evidence surfaces must NOT be misfiled as "core glue" and left
behind.** Concerns (from the plan; non-exhaustive — confirm against the file at slice
time):

gate-evidence · data-client-readiness · ssm-manifest/redaction ·
financial-envelope/approval-nonce · market-selection-source · abort-plan-proof ·
strategy-input-evidence · chainlink-streams · entry-decision-source ·
live-canary-terminal/secret-scan · static-artifacts-manifest · operator-evidence-packet.
