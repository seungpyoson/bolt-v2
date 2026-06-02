# Decomposition Architecture Contract — bolt-v3 monoliths (#522)

**Purpose:** the rules every slice (Track A + Track B) follows so we cut along the
right lines and name things consistently. **Behavior-preserving moves only** — no logic
or economics changes in this phase.

## 1. Module boundary map — which layer does each piece belong to?

| Layer | Owns | Lives in |
|---|---|---|
| **Strategy** | *what* to trade — signals, intent, entry/exit decisions **only** | `strategies/…` |
| **Shared execution/admission** | *whether & how* to execute — **order construction, quantity-normalization**, rounding, fee-adjustment, sizing, admission-request construction + valuation, submit-gating, the **submit wrapper** | `bolt_v3_*` modules |
| **Market-family** | family-specific mechanics — identity, target validation, family pricing, the family registry | `bolt_v3_market_families/…` |
| **Core/shared util & glue** | family-agnostic numeric/util, shared types, the error enum, constants, JSON I/O | dedicated shared files (e.g. `bolt_v3_numeric.rs`); only *truly common* glue stays in a `mod.rs` (don't let `mod.rs` become a dumping ground) |

**How to read the slice tables (important):** a "Track A target" names a concern to
**extract OUT OF** the strategy monolith. The **destination layer is given by the target
module name** — `bolt_v3_*` = shared layer, `bolt_v3_market_families/*` = family layer.
So "extract `book_sizing` → `bolt_v3_book_sizing.rs`" means it **moves out of the strategy
into the shared layer** — it does NOT stay strategy-resident. (This resolves the apparent
"sizing/admission is both shared *and* a Track A target" contradiction: Track A lists
*what we remove from the strategy*; the layer is *where it lands*.)

Per-target destination + layer (Track A):

| Target → module | Destination layer |
|---|---|
| `numeric` → `bolt_v3_numeric.rs` | core/shared util |
| `taker_signal` → `bolt_v3_taker_signal.rs` | shared decision-math (family-agnostic) |
| `selection` → `…selection` | market-family / shared |
| `book_sizing` → `bolt_v3_book_sizing.rs` | shared execution |
| `taker_pricing` → `bolt_v3_taker_pricing.rs` | shared pricing-state |
| `exposure` → `…exposure` | shared **position/exposure accounting**. If the monolith mixes *signal-intent* exposure (a strategy concern) with *position accounting* (shared), **split them** — accounting goes shared, signal-intent stays strategy. |
| `source_proof` → `…source_proof` | shared evidence/replay |
| `config` → `bolt_v3_config.rs` | shared/core |
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
  do **not** catch a shared module doing `use crate::strategies::…`. **Before A3**, add a
  dependency-direction fence (`scripts/verify_bolt_v3_dependency_direction.py`) that fails
  CI on any `crate::strategies` import or strategy-type reference inside `bolt_v3_*` and
  `bolt_v3_market_families/*`. Until that fence exists, every slice PR must record an
  explicit manual grep-check of the moved module (see §4).

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
- **Real shrink, not gamed:** the original monolith's logic and public interface are
  preserved AND its line count strictly drops — line count alone is insufficient (no
  moving comments/whitespace to fake a drop).
- **Dependency check passes:** the §2 dependency-direction fence is green (or, until it
  exists, the PR records the manual grep-check).
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
