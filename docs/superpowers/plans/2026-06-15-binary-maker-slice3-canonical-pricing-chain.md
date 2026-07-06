# Binary-Oracle Maker — Slice 3: Canonical Pricing Chain (`GmReservationBand`)

> **For agentic workers:** REQUIRED SUB-SKILL: this is one tightly-coupled type
> refactor (a newtype threaded through a producer, a consumer, and a dispatcher).
> It is NOT parallelizable across subagents — implement it as a single coherent
> change with TDD structure. Local cargo is REFUSED; the "run test to see
> fail/pass" steps are verified on CI (Rust Probe `check-lib`, then full CI), not
> locally. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Close the §4 CANONICAL-CHAIN GAP (§16#8): make the maker's reservation
band *unconstructable* except as the output of `gm_binary_quote(p, μ)`, so the
fair probability used to lay out the quote and the GM posteriors used as the band
edges can never diverge. Today `gm_binary_quote` is dead in production and
`compose_binary_legs` takes three independent `f64` scalars (`p_up`,
`reservation_bid`, `reservation_ask`) that nothing forces to be mutually
consistent.

**Architecture:** Introduce a `GmReservationBand` newtype in
`src/bolt_v3_maker_model.rs` with **private** fields `(p_up, bid, ask)` and a sole
constructor — `gm_binary_quote`. It **replaces** `BinaryGmQuote` (no dual band
type). `compose_binary_legs` and `FamilyQuoteInputs` consume the band instead of
the three loose scalars. The scalar primitive `resolve_band` is unchanged and
keeps its degenerate-input tests (the band newtype gates the *compose* boundary;
the primitive stays defensively tested at the scalar layer).

**Tech Stack:** Rust, no NT types in this math, bounds/divisors from
`crate::bolt_v3_numeric`. No TOML changes (no new runtime values; μ and fair are
already-sourced inputs). **No gated source roots are touched** —
`bolt_v3_maker_model.rs`, `bolt_v3_quoting.rs`, `bolt_v3_market_families/{mod,updown}.rs`
are NOT in `GATED_SOURCE_ROOTS` (verified at `source_canonicalization.rs:561-589`),
so **no GOLDEN digest re-record is required**.

**§16 decision closed:** `§16#8` (canonical pricing chain).

**Depends:** Slice 1 (committed), Slice 2 (committed, CI-green @ `a2d8985fc`).

---

## Design decisions (locked)

1. **Band carries `p_up`.** `GmReservationBand { p_up, bid, ask }` (all private).
   `compose_binary_legs` reads all three from the band — it no longer takes a
   separate `p_up`/`reservation_bid`/`reservation_ask`. This is the single-source
   guarantee: the value used for the straddle/layout is *definitionally* the value
   the posteriors were computed from.
2. **Replace, don't add.** `BinaryGmQuote` is deleted; `gm_binary_quote` returns
   `Option<GmReservationBand>`. `BinaryGmQuote` has no consumers outside this
   module + its own tests + `gm_half_spread` (verified), so deletion is clean and
   avoids a dual band type (NO DUAL PATHS).
3. **Accessors, not public fields.** `p_up()`, `bid()`, `ask()`, `half_spread()`
   read-only accessors. No public field, no `pub(crate)` test constructor — a
   test backdoor would defeat the invariant the slice exists to create.
4. **`resolve_band` stays scalar + keeps its degenerate tests.** The band
   guarantees `bid ≤ p_up ≤ ask` by construction (gm posteriors satisfy
   `bid ≤ p ≤ ask`, equality at μ=0), so `compose_binary_legs`' crossed-band
   rejection is unreachable through the canonical path. That rejection remains
   covered where it is still reachable — `resolve_band`'s own scalar tests. The
   `compose_binary_legs` tests are rewritten to feed **real** `gm_binary_quote`
   bands.
5. **`FamilyQuoteInputs` collapses** `fair` + `reservation_bid` + `reservation_ask`
   → one `band: GmReservationBand` field. Net field count 10 → 8. Stays `Copy`
   (band is 3×`f64`). The binding pointer type
   `fn(FamilyQuoteInputs) -> Option<QuoteTargets>` is unchanged.

---

## File structure

- **Modify** `src/bolt_v3_maker_model.rs` — delete `BinaryGmQuote`; add
  `GmReservationBand` (private fields + accessors); `gm_binary_quote` returns it;
  `gm_half_spread` reads `band.half_spread()`; update module doc + tests.
- **Modify** `src/bolt_v3_quoting.rs` — `FamilyQuoteInputs.{fair,reservation_bid,
  reservation_ask}` → `band: GmReservationBand`; `compose_binary_legs(band, …)`
  (8 params); rewrite the 5 in-file test call sites + `compose_symmetric` helper
  to mint bands via `gm_binary_quote`; drop the crossed-band assertion (now lives
  at `resolve_band`).
- **Modify** `src/bolt_v3_market_families/updown.rs` — `maker_quote_targets`
  passes `inputs.band` (drops the `sanitize_probability(inputs.fair)` re-sanitize;
  the band's `p_up` is already sanitized by `gm_binary_quote`).
- **Modify** `src/bolt_v3_market_families/mod.rs` — `fixture_quote_inputs` builds
  `band: gm_binary_quote(0.60, μ)` instead of the `fair/reservation_bid/
  reservation_ask` literals.

---

### Task 1: `GmReservationBand` newtype replaces `BinaryGmQuote`

**Files:** Modify `src/bolt_v3_maker_model.rs`.

- [ ] **Step 1 — Write the failing tests.** Replace the `BinaryGmQuote`-typed
  assertions with `GmReservationBand` accessor assertions, and add the
  sole-producer/consistency tests:

```rust
#[test]
fn band_exposes_consistent_fair_and_straddling_edges() {
    // p_up is the exact value the posteriors were computed from, and the edges
    // straddle it: bid <= p_up <= ask (the resolve_band straddle invariant, now
    // guaranteed by construction rather than checked downstream).
    let band = gm_binary_quote(0.6, 0.2).expect("interior fair, valid mu");
    assert!((band.p_up() - 0.6).abs() < EPSILON);
    assert!(band.bid() <= band.p_up() && band.p_up() <= band.ask());
    // half_spread accessor agrees with (ask - bid)/2.
    assert!((band.half_spread() - (band.ask() - band.bid()) / 2.0).abs() < EPSILON);
}
```

  Update every existing test that read `quote.bid` / `quote.ask` to
  `band.bid()` / `band.ask()`, and `BinaryGmQuote` type names to
  `GmReservationBand`. Keep the closed-form posterior values identical.

- [ ] **Step 2 — Verify it fails (CI/probe).** It won't compile until Step 3
  (type `GmReservationBand`, methods `p_up()/bid()/ask()/half_spread()` don't
  exist). On the Rust Probe this is a compile error on the test module — the
  expected red.

- [ ] **Step 3 — Minimal implementation.** Replace the `BinaryGmQuote` struct and
  the `gm_binary_quote` tail:

```rust
/// The Glosten-Milgrom reservation band for a binary outcome token: the fair
/// probability `p_up` the band was computed from, together with the break-even
/// `bid = E[V|sell]` and `ask = E[V|buy]` posteriors that straddle it. The fields
/// are private and the only constructor is [`gm_binary_quote`] — a band cannot be
/// assembled from a bare struct literal, so the value used to lay out the quote
/// (`p_up`) is definitionally the value its edges were derived from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GmReservationBand {
    p_up: f64,
    bid: f64,
    ask: f64,
}

impl GmReservationBand {
    /// The fair probability the YES outcome resolves true.
    pub fn p_up(&self) -> f64 {
        self.p_up
    }
    /// `E[V | sell]` — the price at which the maker is willing to buy YES.
    pub fn bid(&self) -> f64 {
        self.bid
    }
    /// `E[V | buy]` — the price at which the maker is willing to sell YES.
    pub fn ask(&self) -> f64 {
        self.ask
    }
    /// The adverse-selection half-spread `(ask − bid)/2`, in probability units.
    pub fn half_spread(&self) -> f64 {
        (self.ask - self.bid) / TWO_F64
    }
}
```

  `gm_binary_quote` returns `Option<GmReservationBand>` with the existing
  posterior math, now stamping `p_up: p`:

```rust
    Some(GmReservationBand {
        p_up: p,
        bid: sell_up / sell_denom,
        ask: buy_up / buy_denom,
    })
```

  `gm_half_spread` delegates to the band:

```rust
pub fn gm_half_spread(fair_p_up: f64, informed_fraction: f64) -> Option<f64> {
    Some(gm_binary_quote(fair_p_up, informed_fraction)?.half_spread())
}
```

  Update the module doc header to name `GmReservationBand` as the produced type.

- [ ] **Step 4 — Verify it passes (CI/probe).** Covered by the full-CI nextest run
  after push.

- [ ] **Step 5 — Commit** (after Tasks 2-4 compile together — this is one coherent
  refactor; a single commit for the slice).

---

### Task 2: `compose_binary_legs` and `FamilyQuoteInputs` consume the band

**Files:** Modify `src/bolt_v3_quoting.rs`.

- [ ] **Step 1 — Write/rewrite the failing tests.** `compose_symmetric` becomes a
  band-minting helper, and the crossed-band assertion is dropped (it is covered at
  `resolve_band_rejects_degenerate_bands`):

```rust
// Build a real GM band and lay it out — the only way to get a band is through
// gm_binary_quote, so the tests exercise the canonical chain end to end.
fn compose_from(p: f64, mu: f64, skew: f64) -> Option<BinaryLegPrices> {
    let band = gm_binary_quote(p, mu)?;
    compose_binary_legs(band, ZERO_F64, UNIT_F64, REF_TAU, REF_TAU, WIDEN_CAP, skew, TEST_EPS)
}
```

  Rewrite the widening test to use `compose_binary_legs(band, …, REF_TAU/4.0, …)`
  with a band from `gm_binary_quote`, and the eps/crossed-horizon failure tests to
  pass a valid band with a bad `tau`/`eps`. Remove the
  `compose_binary_legs(0.50, 0.70, 0.30, …)` crossed-band assertion.

- [ ] **Step 2 — Verify it fails (CI/probe):** compile error — `compose_binary_legs`
  still has the old 10-scalar signature; `gm_binary_quote` import missing.

- [ ] **Step 3 — Minimal implementation.**
  Import the band: `use crate::bolt_v3_maker_model::GmReservationBand;`
  Collapse the struct:

```rust
pub struct FamilyQuoteInputs {
    pub band: GmReservationBand,
    pub inventory_skew: f64,
    pub half_spread_floor: f64,
    pub max_half_spread: f64,
    pub eps: f64,
    pub tau: f64,
    pub reference_tau: f64,
    pub time_widen_cap: f64,
}
```

  New signature (band first, 8 params), reading scalars from the band and keeping
  `resolve_band`/widening/skew/eps/`yes+no<1`/straddle logic byte-for-byte
  otherwise:

```rust
#[allow(clippy::too_many_arguments)]
pub fn compose_binary_legs(
    band: GmReservationBand,
    half_spread_floor: f64,
    max_half_spread: f64,
    tau: f64,
    reference_tau: f64,
    time_widen_cap: f64,
    inventory_skew: f64,
    eps: f64,
) -> Option<BinaryLegPrices> {
    let p_up = band.p_up();
    let (resolved_bid, resolved_ask) = resolve_band(
        p_up,
        band.bid(),
        band.ask(),
        half_spread_floor,
        max_half_spread,
    )?;
    // … unchanged from here (mid, widening, skew, eps, yes+no<1, straddle check) …
}
```

  `resolve_band` and `time_widening_factor` keep their current scalar signatures
  and tests unchanged.

- [ ] **Step 4 — Verify it passes (CI).**
- [ ] **Step 5 — Commit** (with the slice).

---

### Task 3: `updown::maker_quote_targets` + dispatcher fixture

**Files:** Modify `src/bolt_v3_market_families/updown.rs`,
`src/bolt_v3_market_families/mod.rs`.

- [ ] **Step 1 — Update the production consumer** (`updown.rs`): pass the band,
  drop the redundant `sanitize_probability(inputs.fair)` (band `p_up` is already
  sanitized):

```rust
pub fn maker_quote_targets(inputs: FamilyQuoteInputs) -> Option<QuoteTargets> {
    let legs = compose_binary_legs(
        inputs.band,
        inputs.half_spread_floor,
        inputs.max_half_spread,
        inputs.tau,
        inputs.reference_tau,
        inputs.time_widen_cap,
        inputs.inventory_skew,
        inputs.eps,
    )?;
    Some(QuoteTargets {
        leg_a: QuoteTargetLeg { side: QuoteSide::Buy, price: legs.yes_price },
        leg_b: QuoteTargetLeg { side: QuoteSide::Buy, price: legs.no_price },
    })
}
```

  If `sanitize_probability` becomes unused in `updown.rs`, drop its import.

- [ ] **Step 2 — Update the dispatcher fixture** (`mod.rs:1246`) to mint a band:

```rust
fn fixture_quote_inputs() -> FamilyQuoteInputs {
    FamilyQuoteInputs {
        band: crate::bolt_v3_maker_model::gm_binary_quote(0.60, 0.10)
            .expect("interior fair, valid mu"),
        inventory_skew: 0.0,
        half_spread_floor: 0.0,
        max_half_spread: 1.0,
        eps: 0.000_001,
        tau: 3_600.0,
        reference_tau: 3_600.0,
        time_widen_cap: 10.0,
    }
}
```

  (μ = 0.10 gives an interior band around 0.60; the dispatcher test asserts
  routing, not exact prices.)

- [ ] **Step 3 — Verify (CI).**
- [ ] **Step 4 — Commit** (with the slice).

---

### Task 4: Verify + review

- [ ] `cargo fmt` (the only permitted local cargo) — format all four files.
- [ ] Fast local gate: `just source-fence` (runtime-literals + naming) — no new
  literals are introduced (all numbers come from `bolt_v3_numeric` or are existing
  test consts), so this should pass without an allowlist row.
- [ ] Push; run Rust Probe (`check-lib`) for fast compile/test feedback on the
  slice; then mark the PR ready for full pull-request CI or use the merge queue
  gate for proof (no digest job — no gated file changed).
- [ ] Slice-3 adversarial review (Codex + internal, multi-lens) per the program
  cadence; every finding FIXED or DISPROVEN before Slice 4.

---

## Self-review (writing-plans)

- **Spec coverage:** §16#8 canonical chain — closed by Tasks 1-3 (sole-producer
  newtype + consumer wiring). ✓
- **Placeholder scan:** the only `…` is the explicitly-"unchanged" tail of
  `compose_binary_legs` (byte-for-byte preserved); every new type/signature is
  shown in full. ✓
- **Type consistency:** `GmReservationBand` accessors (`p_up/bid/ask/half_spread`)
  used identically in Tasks 1-3; `compose_binary_legs(band, …)` 8-arg signature
  matches every rewritten call site; `FamilyQuoteInputs.band` field name matches
  the fixture and `updown` reads. ✓
- **Scope:** one newtype, one §16 decision, no gated files, no TOML, no strategy
  edits → no digest rotation. ✓
