# Decomposition Architecture Contract — bolt-v3 monoliths (#522)

**Purpose:** the rules every slice (Track A + Track B) follows so we cut along the
right lines and name things consistently. **Behavior-preserving moves only** — no logic
or economics changes in this phase.

## 1. Module boundary map — which layer does each piece belong to?

One test per piece of code:

| Layer | Owns | Goes in |
|---|---|---|
| **Strategy** | *what* to trade — signals, intent, entry/exit decisions | `strategies/…` |
| **Shared execution/admission** | *whether & how* to execute — admission, sizing, rounding, fee-adjustment, submit-gating | `bolt_v3_*` modules |
| **Market-family** | family-specific mechanics — identity, target validation, family pricing | `bolt_v3_market_families/…` |
| **Core glue** | shared types, the error enum, constants, JSON I/O | each file's `mod.rs` (**stays put**) |

Concrete targets:

- **Track A** → `taker_signal`, `numeric`, `selection`, `book_sizing`, `taker_pricing`,
  `exposure`, `source_proof`, `config`, `submit_admission`.
- **Track B** → `operator_artifacts/` concern-modules (gate-evidence,
  data-client-readiness, ssm-manifest, financial-envelope, pre-run/abort proofs,
  evidence-packet, provenance).

## 2. Dependency direction — one way only

- Strategy **may** use shared + family modules.
- Shared / family modules **must never import the strategy** — no back-references.
- **No cycles.** If A needs B and B needs A, the boundary is wrong — pull the shared
  piece out.
- Litmus test: *"if I deleted the strategy, would the shared modules still compile?"*
  They must.

## 3. Naming convention

- **Core/shared names are family-agnostic** — no concrete family word (`Updown`,
  `Polymarket`) in shared/core code. *(Enforced by
  `scripts/verify_bolt_v3_provider_leaks.py`. Example already hit: the side type is
  `OutcomeSide`, not `UpdownOutcomeSide`.)*
- **Family words are allowed in exactly one place** — the family module
  (`market_families/updown.rs`, which is fence-exempt).
- **Evidence/field names use `market_*`, never `polymarket_*`** — existing `polymarket_*`
  fields in shared evidence are debt to rename (tracked W2-2 / findings-doc #12).
- **Don't collide with NautilusTrader-owned names** — source of truth
  `docs/bolt-v3/research/naming/nt-owned-name-audit.yaml`, enforced by
  `scripts/verify_bolt_v3_naming.py`.
- New module/type names are proposed per slice but must pass all three CI fences above
  (naming, provider-leak, core-boundary).

## 4. Acceptance — every slice must hold

- Move + imports only; **no logic/branch/number change**. Public API preserved via
  re-export *only* where an external caller/test needs it.
- Monolith line count strictly drops. Tests carried to the new home (RED→GREEN). CI
  green at exact head; all three fences + the runtime-literal allowlist pass.

## 5. Out of scope (this phase)

No logic/economics changes, no new features, no venue/secret changes. The deeper "route
by instrument identity instead of an Up/Down enum in core" is a **future** item
(findings-doc #13), not a decomposition slice.
