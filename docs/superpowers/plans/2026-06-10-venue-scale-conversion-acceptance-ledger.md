# Venue Scale Conversion Acceptance Ledger Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a venue/source-universe acceptance ledger that reports current converted, source-only, and blocked conversion status for Binance, Bybit, and PMXT without claiming unconverted data is converted.

**Architecture:** Add one Rust module in `crates/backtesting-vertical-slice` that reads a TOML spec, validates referenced conversion/source artifacts, and writes an idempotent JSON ledger. The ledger summarizes existing completion ledgers for converted BNBUSDC batches, the Bybit all-instrument source-only object manifest, and the PMXT selected-source NT catalog conversion.

**Tech Stack:** Rust, serde, toml, serde_json, sha2, existing BTE reference artifact layout.

---

### Task 1: Venue Acceptance Ledger Contract

**Files:**
- Create: `crates/backtesting-vertical-slice/tests/backtesting_vertical_slice_venue_scale_conversion_acceptance.rs`
- Create: `crates/backtesting-vertical-slice/src/venue_scale_conversion_acceptance.rs`
- Modify: `crates/backtesting-vertical-slice/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Test that a three-venue spec writes a ledger with:
- Binance converted BNBUSDC completion totals.
- Bybit converted BNBUSDC completion totals plus the all-instrument source manifest count.
- PMXT selected-source conversion totals plus explicit blocked status for full PMXT.

- [ ] **Step 2: Run the focused test to verify it fails**

Run:
`RUST_VERIFICATION_ROOT_BASE=/private/tmp/bolt-rv-bte python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_venue_scale_conversion_acceptance -- --nocapture`

- [ ] **Step 3: Implement minimal module and expose it**

Implement spec parsing, artifact hashing, JSON/TOML summary extraction, status aggregation, and idempotent artifact writing.

- [ ] **Step 4: Run focused test to verify it passes**

Run the same focused test.

### Task 2: Reference Spec And CLI

**Files:**
- Create: `crates/backtesting-vertical-slice/src/bin/venue_scale_conversion_acceptance_ledger.rs`
- Create: `specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/binance-bybit-pmxt-current/venue-scale-conversion-acceptance-ledger.toml`

- [ ] **Step 1: Add CLI coverage through the same library function**

The CLI accepts `--spec` and prints ledger path, status, venue count, converted universes, source-only universes, and blocked universes.

- [ ] **Step 2: Materialize the committed reference ledger**

Run the CLI against the reference spec and commit both TOML and JSON artifacts.

### Task 3: Verification

**Files:**
- Existing BTE crate and reference artifacts.

- [ ] **Step 1: Format**

Run:
`cargo fmt --check`

- [ ] **Step 2: Focused test**

Run:
`RUST_VERIFICATION_ROOT_BASE=/private/tmp/bolt-rv-bte python3 scripts/rust_verification.py cargo --repo crates/backtesting-vertical-slice -- test --locked --test backtesting_vertical_slice_venue_scale_conversion_acceptance -- --nocapture`

- [ ] **Step 3: BTE checks**

Run:
`just bte-clippy`
`just bte-test`
