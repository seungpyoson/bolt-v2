# Binary-Maker Slice 0 — Drift Fences & Shared Feed-Health Seam — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the two structural enablers the binary-maker build needs before any maker code: (1) the FR-080 CI fence forbidding venue-name string-literal branches outside provider modules, and (2) the stale-feed forced-flat predicates hoisted from the taker into a shared `bolt_v3_feed_health` module so the maker can reuse them (Rule #6, no dual-state) — plus the §16#9 stale `spec.md` assumption fix.

**Architecture:** Two independent units, no maker code. Unit A is a pure Python CI fence mirroring the existing `verify_bolt_v3_no_exit_market_command.py` sibling, registered in the `justfile` `source-fence-static` recipe. Unit B is a behavior-preserving Rust refactor: move three `pub(super)` items into a new `src/bolt_v3_feed_health.rs`, sever the fence-forbidden `SelectionPhase` coupling by replacing a `phase: SelectionPhase` field with a `frozen: bool`, and rewire the taker's two callers + three direct-call test files.

**Tech Stack:** Python 3 (stdlib `re`, `unittest`, `dataclasses`) for the fence; Rust + NautilusTrader for the hoist; `just` recipes for CI wiring.

**Source spec:** `docs/superpowers/specs/2026-06-14-multi-asset-mm-platform-design.md` @ `790da9a2e` (§9, §15, §16#9, §16#12). Program: `docs/superpowers/plans/2026-06-15-binary-maker-program.md`. Issue **#488**.

**Branch:** `feat/488-slice0-fences-feed-health` off `main`. **Verification:** Python fences run locally; Rust runs **remote-first per `AGENTS.md`** (local cargo is refused by `ci/rust-verification.toml [local_compile_policy]` — do NOT run local `cargo build`/`cargo test`; use `just verify-remote` / push and let CI run). The dependency-direction and venue-name fences are plain Python and run locally.

---

## File Structure

**Unit A — FR-080 venue-name fence (§16#12):**
- Create `scripts/verify_bolt_v3_no_venue_name_branch.py` — the fence (scans `src/**/*.rs`, exempts `src/bolt_v3_providers/`).
- Create `scripts/test_verify_bolt_v3_no_venue_name_branch.py` — its unit test (positive/negative fixtures + fail-closed + current-tree-clean).
- Modify `justfile` — add a `verify-bolt-v3-no-venue-name-branch` recipe and register the pair in `source-fence-static`.

**Unit B — feed-health hoist (§15):**
- Create `src/bolt_v3_feed_health.rs` — `ForcedFlatReason`, `ForcedFlatInputs` (with `frozen: bool`), `evaluate_forced_flat_predicates`, all `pub`, + inline `#[cfg(test)] mod tests`.
- Modify `src/lib.rs` — add `pub mod bolt_v3_feed_health;`.
- Modify `src/strategies/binary_oracle_edge_taker/exposure.rs` — delete the three hoisted items + drop now-unused `SelectionPhase` import if unreferenced.
- Modify `src/strategies/binary_oracle_edge_taker/mod.rs` — re-import the three symbols from `crate::bolt_v3_feed_health`; change `phase: self.active.phase` → `frozen: self.active.phase == SelectionPhase::Freeze` in both callers.
- Modify `src/strategies/binary_oracle_edge_taker/tests/{orders_admission,core_glue,book_sizing}.rs` — update import path + `phase:`→`frozen:` in the three direct-call sites.

**Unit C — docs hygiene (§16#9):**
- Modify `specs/488-binary-oracle-maker/spec.md` — correct the stale `100/second` order-budget assumption to `40/min`.

---

## Task 1: FR-080 fence — failing test fixtures (TDD red)

**Files:**
- Create: `scripts/test_verify_bolt_v3_no_venue_name_branch.py`

- [ ] **Step 1: Write the test file**

```python
#!/usr/bin/env python3
"""Unit tests for the FR-080 venue-name string-literal branch fence."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

SCRIPT_PATH = Path(__file__).resolve().parent / "verify_bolt_v3_no_venue_name_branch.py"
_spec = importlib.util.spec_from_file_location("verify_bolt_v3_no_venue_name_branch", SCRIPT_PATH)
VERIFIER = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(VERIFIER)


class FenceTests(unittest.TestCase):
    def _one(self, snippet: str) -> list:
        return VERIFIER.find_violations_in_text("src/probe.rs", snippet)

    def test_name_eq_literal(self):
        self.assertEqual(len(self._one('if venue_id == "polymarket" {')), 1)

    def test_literal_eq_name(self):
        self.assertEqual(len(self._one('if "BINANCE" == venue.venue_id() {')), 1)

    def test_contains(self):
        self.assertEqual(len(self._one('if venue_name.contains("bybit") {')), 1)

    def test_starts_with_dotted(self):
        self.assertEqual(len(self._one('if self.venue_id.starts_with("okx") {')), 1)

    def test_eq_ignore_ascii_case(self):
        self.assertEqual(len(self._one('if venue.eq_ignore_ascii_case("hyperliquid") {')), 1)

    def test_matches_arm(self):
        self.assertEqual(len(self._one('if matches!(venue_id, "deribit") {')), 1)

    def test_uppercase_literal_is_caught(self):
        self.assertEqual(len(self._one('if venue_id == "POLYMARKET" {')), 1)

    def test_comment_is_not_a_violation(self):
        self.assertEqual(self._one('// venue_id == "polymarket" historical note'), [])

    def test_identifier_substring_is_not_a_violation(self):
        self.assertEqual(self._one('let venue_id_polymarket = 1;'), [])

    def test_arg_position_literal_is_not_a_violation(self):
        self.assertEqual(self._one('fast_spot("bybit", cfg);'), [])

    def test_empty_source_set_fails_closed(self):
        with self.assertRaises(RuntimeError):
            VERIFIER.collect_violations_from_files([])

    def test_current_bolt_src_is_clean(self):
        # Preventive fence: there are zero venue-name compares in src today.
        self.assertEqual(VERIFIER.collect_violations(), [])


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    unittest.main()
```

- [ ] **Step 2: Run it to confirm it fails (module missing)**

Run: `python3 scripts/test_verify_bolt_v3_no_venue_name_branch.py`
Expected: FAIL — `FileNotFoundError` / `ModuleNotFoundError` on the not-yet-written verifier.

---

## Task 2: FR-080 fence — implement the verifier (TDD green)

**Files:**
- Create: `scripts/verify_bolt_v3_no_venue_name_branch.py`

- [ ] **Step 1: Write the fence**

Mirror the structure of `scripts/verify_bolt_v3_no_exit_market_command.py` (dataclasses, `bolt_src_files`, `main`, footer). **Critical deltas vs that sibling:** (a) it imports `strip_rust_comments_and_literals` which BLANKS string literals — we must NOT use it (it would erase the `"polymarket"` literal we need to catch); instead strip comments while **preserving** string literals; (b) exempt the whole `src/bolt_v3_providers/` subtree (where `pub const KEY: &str = "BINANCE";` legitimately lives).

```python
#!/usr/bin/env python3
"""FR-080: forbid venue-name string-literal branches outside provider modules.

The capability contract (D8/FR-080) requires the controller to branch on venue
*capabilities* read from `VenueContract`, never on a hardcoded venue name. The
existing `verify_bolt_v3_core_boundary.py` catches only `match venue.kind` /
`VenueKind` enum dispatch over a fixed file set; it does NOT catch string-literal
comparisons like `venue_id == "polymarket"`. This fence closes that gap. Provider
modules under `src/bolt_v3_providers/` are exempt — that is where venue-name KEY
literals legitimately live.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

from bolt_v3_source_roots import REPO_ROOT
from verify_bolt_v3_pure_rust_runtime import production_text

PROVIDERS_PREFIX = "src/bolt_v3_providers/"

_VENUE = r"(?:polymarket|binance|bybit|okx|hyperliquid|deribit|chainlink|gamma)"
_NAME = r"(?:[A-Za-z_][A-Za-z0-9_]*\.)*(?:venue_id|venue_name|venue)\b"
_LIT = rf'"{_VENUE}"'


@dataclass(frozen=True)
class Rule:
    label: str
    pattern: re.Pattern[str]


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    label: str
    excerpt: str


FORBIDDEN_RULES = (
    Rule("venue-name equality (name == lit)", re.compile(rf"{_NAME}\s*==\s*{_LIT}", re.IGNORECASE)),
    Rule("venue-name equality (lit == name)", re.compile(rf"{_LIT}\s*==\s*{_NAME}", re.IGNORECASE)),
    Rule(
        "venue-name membership/method",
        re.compile(
            rf"{_NAME}\s*\.\s*(?:contains|starts_with|ends_with|eq|eq_ignore_ascii_case)\s*\(\s*{_LIT}",
            re.IGNORECASE,
        ),
    ),
    Rule("venue-name matches! arm", re.compile(rf"matches!\s*\(\s*{_NAME}\s*,[^)]*{_LIT}", re.IGNORECASE)),
)

_COMMENT_OR_STRING = re.compile(r'"(?:\\.|[^"\\])*"|//[^\n]*|/\*.*?\*/', re.DOTALL)


def strip_comments_keep_strings(text: str) -> str:
    """Blank // and /* */ comments but PRESERVE string literals and newlines.

    String literals are matched first in the alternation, so a `//` or `/*`
    inside a string is consumed as part of the (preserved) literal. Comments are
    replaced char-for-char with spaces so byte offsets and line numbers are
    unchanged.
    """

    def repl(match: re.Match[str]) -> str:
        token = match.group(0)
        if token.startswith('"'):
            return token
        return re.sub(r"[^\n]", " ", token)

    return _COMMENT_OR_STRING.sub(repl, text)


def line_number(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def find_violations_in_text(path: str, text: str) -> list[Violation]:
    scan_text = strip_comments_keep_strings(text)
    violations: list[Violation] = []
    for rule in FORBIDDEN_RULES:
        for match in rule.pattern.finditer(scan_text):
            line_start = scan_text.rfind("\n", 0, match.start()) + 1
            line_end = scan_text.find("\n", match.end())
            if line_end == -1:
                line_end = len(text)
            violations.append(
                Violation(
                    path=path,
                    line=line_number(scan_text, match.start()),
                    label=rule.label,
                    excerpt=text[line_start:line_end].strip(),
                )
            )
    return violations


def bolt_src_files() -> list[Path]:
    src_root = REPO_ROOT / "src"
    files: list[Path] = []
    for path in src_root.rglob("*.rs"):
        if path.is_symlink():
            raise ValueError(f"src contains a symlink: {path}")
        if not path.is_file():
            continue
        rel = path.relative_to(REPO_ROOT).as_posix()
        if rel.startswith(PROVIDERS_PREFIX):
            continue
        files.append(path)
    files.sort(key=lambda path: path.relative_to(REPO_ROOT).as_posix().encode("utf-8"))
    return files


def collect_violations_from_files(files: list[Path]) -> list[Violation]:
    if not files:
        raise RuntimeError("no Rust source files found under src")
    violations: list[Violation] = []
    for path in files:
        rel = str(path.relative_to(REPO_ROOT))
        violations.extend(find_violations_in_text(rel, production_text(path)))
    return violations


def collect_violations() -> list[Violation]:
    return collect_violations_from_files(bolt_src_files())


def main() -> int:
    violations = collect_violations()
    if violations:
        for violation in violations:
            print(
                "FAIL: Bolt-v3 FR-080 venue-name branch fence "
                f"{violation.label} at {violation.path}:{violation.line}: {violation.excerpt}",
                file=sys.stderr,
            )
        return 1
    print("OK: Bolt-v3 FR-080 venue-name branch fence passed.")
    return 0


if __name__ == "__main__":
    import lane_governor

    lane_governor.acquire()
    raise SystemExit(main())
```

- [ ] **Step 2: Run the unit test to verify it passes**

Run: `python3 scripts/test_verify_bolt_v3_no_venue_name_branch.py`
Expected: `OK` — all positive cases yield exactly one violation, all negative cases yield `[]`, `test_empty_source_set_fails_closed` raises `RuntimeError`, `test_current_bolt_src_is_clean` passes.

---

## Task 3: FR-080 fence — prove clean on the live tree + wire into CI

**Files:**
- Modify: `justfile` (recipe block ~`84-86`; `source-fence-static` ~`173-200`)

- [ ] **Step 1: Run the verifier against the real tree**

Run: `python3 scripts/verify_bolt_v3_no_venue_name_branch.py`
Expected: `OK: Bolt-v3 FR-080 venue-name branch fence passed.` (exit 0). This is a **preventive** fence — there are zero venue-name compares in `src` today. If it prints any `FAIL`, the regex has a false positive — fix the regex, do NOT exempt the file.

- [ ] **Step 2: Add the standalone recipe**

In `justfile`, after the `verify-bolt-v3-no-exit-market-command` recipe (lines 84-86), add:

```makefile
verify-bolt-v3-no-venue-name-branch: check-workspace
    python3 scripts/test_verify_bolt_v3_no_venue_name_branch.py
    python3 scripts/verify_bolt_v3_no_venue_name_branch.py
```

- [ ] **Step 3: Register the pair in `source-fence-static`**

In the `source-fence-static` recipe, immediately after the existing no-exit-market pair (lines 194-195), add:

```makefile
    python3 scripts/test_verify_bolt_v3_no_venue_name_branch.py
    python3 scripts/verify_bolt_v3_no_venue_name_branch.py
```

(No `ci/*.toml` edit — `ci/rust-verification.toml` governs cargo policy only; the source-fence roster lives entirely in this recipe, which the CI `source-fence` job invokes.)

- [ ] **Step 4: Run the aggregate to confirm it still passes**

Run: `just verify-bolt-v3-no-venue-name-branch`
Expected: both lines print `OK`.

- [ ] **Step 5: Commit**

```bash
git add scripts/verify_bolt_v3_no_venue_name_branch.py scripts/test_verify_bolt_v3_no_venue_name_branch.py justfile
python3 ~/.claude/lib/safe_git.py commit -m "feat(ci): FR-080 fence forbidding venue-name string-literal branches outside providers"
```

---

## Task 4: Feed-health hoist — characterization test pins current behavior

Pin the exact current predicate behavior so the move is provably behavior-preserving. These assertions mirror the ones already in the taker test suite (`tests/orders_admission.rs:2693-2711`, `tests/core_glue.rs:513-524`, `tests/book_sizing.rs:261-272`) but will live as the new module's inline tests in Task 5; write them here first against the **current** taker symbols to confirm they pass at HEAD.

**Files:**
- Modify (temporary scratch test, or run the existing suite): `src/strategies/binary_oracle_edge_taker/tests/orders_admission.rs`

- [ ] **Step 1: Confirm the existing forced-flat assertions pass at HEAD**

The behavior to preserve (read from `exposure.rs:385-416`):
1. Reasons are pushed in order `Freeze, StaleReference, ThinBook, MetadataMismatch, FastVenueIncoherent`.
2. `last_reference_ts_ms == None` ⇒ `StaleReference` (the `is_none_or` "never observed = maximally stale" branch, `exposure.rs:392`).
3. `liquidity_available == None` OR non-finite OR `< min_liquidity_required` ⇒ `ThinBook` (`exposure.rs:404`).
4. `fast_venue_incoherent == true` but reference **fresh** ⇒ **no** `FastVenueIncoherent` (the `&& reference_stale` coupling, `exposure.rs:411`).

Run (remote-first per `AGENTS.md`; do not run local full cargo): the targeted taker forced-flat tests via `just verify-remote` (or the repo's prescribed targeted-test path).
Expected: PASS at HEAD — establishes the green baseline the hoist must keep.

---

## Task 5: Feed-health hoist — create the shared module

**Files:**
- Create: `src/bolt_v3_feed_health.rs`

- [ ] **Step 1: Write the new module (verbatim move + `frozen: bool` redesign + inline tests)**

The only behavioral change vs `exposure.rs:364-416` is severing the `SelectionPhase` coupling: replace the `phase: SelectionPhase` field + `inputs.phase == SelectionPhase::Freeze` check with a `frozen: bool` + `inputs.frozen`. Everything else is byte-identical (preserve the A14 `is_none_or` comment verbatim — it is load-bearing).

```rust
//! Shared feed-health forced-flat predicates.
//!
//! Hoisted from `binary_oracle_edge_taker::exposure` so the taker AND the maker
//! admission gate evaluate ONE shared predicate set (Rule #6, no dual-state).
//! The `SelectionPhase` coupling is severed: callers pass a `frozen: bool`
//! instead of the taker-private `SelectionPhase`, so this module holds no
//! `crate::strategies` reference and the dependency-direction fence stays green.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForcedFlatReason {
    Freeze,
    StaleReference,
    ThinBook,
    MetadataMismatch,
    FastVenueIncoherent,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForcedFlatInputs {
    pub frozen: bool,
    pub metadata_matches_selection: bool,
    pub last_reference_ts_ms: Option<u64>,
    pub now_ms: u64,
    pub stale_reference_after_ms: u64,
    pub liquidity_available: Option<f64>,
    pub min_liquidity_required: f64,
    pub fast_venue_incoherent: bool,
}

pub fn evaluate_forced_flat_predicates(inputs: &ForcedFlatInputs) -> Vec<ForcedFlatReason> {
    let mut reasons = Vec::new();
    // Defense-in-depth (A14): a MISSING reference timestamp is the maximally
    // stale condition — the strategy has never observed a reference quote — so
    // it must classify as stale, not fresh. `is_none_or` returns `true` for the
    // `None` case (no reference ever) AND for an observed-but-aged reference,
    // and `false` only for a reference observed within the freshness bound.
    let reference_stale = inputs.last_reference_ts_ms.is_none_or(|last_ts_ms| {
        inputs.now_ms.saturating_sub(last_ts_ms) > inputs.stale_reference_after_ms
    });

    if inputs.frozen {
        reasons.push(ForcedFlatReason::Freeze);
    }
    if reference_stale {
        reasons.push(ForcedFlatReason::StaleReference);
    }
    if inputs
        .liquidity_available
        .is_none_or(|liquidity| !liquidity.is_finite() || liquidity < inputs.min_liquidity_required)
    {
        reasons.push(ForcedFlatReason::ThinBook);
    }
    if !inputs.metadata_matches_selection {
        reasons.push(ForcedFlatReason::MetadataMismatch);
    }
    if inputs.fast_venue_incoherent && reference_stale {
        reasons.push(ForcedFlatReason::FastVenueIncoherent);
    }

    reasons
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_ok() -> ForcedFlatInputs {
        ForcedFlatInputs {
            frozen: false,
            metadata_matches_selection: true,
            last_reference_ts_ms: Some(1_000),
            now_ms: 1_010,
            stale_reference_after_ms: 100,
            liquidity_available: Some(50.0),
            min_liquidity_required: 10.0,
            fast_venue_incoherent: false,
        }
    }

    #[test]
    fn clean_inputs_yield_no_reasons() {
        assert_eq!(evaluate_forced_flat_predicates(&fresh_ok()), vec![]);
    }

    #[test]
    fn all_reasons_in_canonical_order() {
        let inputs = ForcedFlatInputs {
            frozen: true,
            metadata_matches_selection: false,
            last_reference_ts_ms: Some(0),
            now_ms: 10_000,
            stale_reference_after_ms: 100,
            liquidity_available: Some(1.0),
            min_liquidity_required: 10.0,
            fast_venue_incoherent: true,
        };
        assert_eq!(
            evaluate_forced_flat_predicates(&inputs),
            vec![
                ForcedFlatReason::Freeze,
                ForcedFlatReason::StaleReference,
                ForcedFlatReason::ThinBook,
                ForcedFlatReason::MetadataMismatch,
                ForcedFlatReason::FastVenueIncoherent,
            ]
        );
    }

    #[test]
    fn none_reference_ts_is_maximally_stale() {
        let mut inputs = fresh_ok();
        inputs.last_reference_ts_ms = None;
        assert!(evaluate_forced_flat_predicates(&inputs).contains(&ForcedFlatReason::StaleReference));
    }

    #[test]
    fn non_finite_liquidity_is_thin_book() {
        let mut inputs = fresh_ok();
        inputs.liquidity_available = Some(f64::NAN);
        assert!(evaluate_forced_flat_predicates(&inputs).contains(&ForcedFlatReason::ThinBook));
    }

    #[test]
    fn fast_venue_incoherent_requires_staleness() {
        let mut inputs = fresh_ok();
        inputs.fast_venue_incoherent = true; // but reference is fresh
        assert!(!evaluate_forced_flat_predicates(&inputs)
            .contains(&ForcedFlatReason::FastVenueIncoherent));
    }
}
```

---

## Task 6: Feed-health hoist — register the module and rewire the taker

**Files:**
- Modify: `src/lib.rs` (the alphabetical `bolt_v3_*` block)
- Modify: `src/strategies/binary_oracle_edge_taker/exposure.rs:17,364-416`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs` (use-block ~`117-125`; callers `1872-1885`, `1887-1904`)
- Modify: `src/strategies/binary_oracle_edge_taker/tests/{orders_admission,core_glue,book_sizing}.rs`

- [ ] **Step 1: Register the module in `src/lib.rs`**

Add `pub mod bolt_v3_feed_health;` in the alphabetical `bolt_v3_*` block (it sorts between `bolt_v3_executable_cost` and `bolt_v3_instrument_filters`). Required so the dependency-direction fence's `src/bolt_v3_` scan covers it and the taker can `use crate::bolt_v3_feed_health::...`.

- [ ] **Step 2: Delete the three hoisted items from `exposure.rs`**

Remove `ForcedFlatReason` (`exposure.rs:364-371`), `ForcedFlatInputs` (`373-383`), and `evaluate_forced_flat_predicates` (`385-416`). If `SelectionPhase` is no longer referenced anywhere else in `exposure.rs`, drop it from the import at `exposure.rs:17` (`use super::{OutcomeFeeState, SelectionPhase};` → `use super::OutcomeFeeState;`). **Do NOT move `SelectionPhase`** — it stays `pub(super)` in `selection.rs`; it is still used by the selection state machine in `mod.rs`.

- [ ] **Step 3: Re-import + rewire the two callers in `mod.rs`**

In the `use self::exposure::{...}` block (~`117-125`), remove `ForcedFlatInputs`, `ForcedFlatReason`, `evaluate_forced_flat_predicates` and add:

```rust
use crate::bolt_v3_feed_health::{ForcedFlatInputs, ForcedFlatReason, evaluate_forced_flat_predicates};
```

In **both** callers — `active_forced_flat_reasons_at` (~`1872-1885`) and `position_forced_flat_reasons_at` (~`1887-1904`) — change the input construction field `phase: self.active.phase` to:

```rust
frozen: self.active.phase == SelectionPhase::Freeze,
```

(`EntryBlockReason::ForcedFlat(ForcedFlatReason)` at `mod.rs:5805` and the variant uses keep compiling via the re-imported type.)

- [ ] **Step 4: Update the three direct-call test files**

In `tests/orders_admission.rs:2693-2711`, `tests/core_glue.rs:513-524`, `tests/book_sizing.rs:261-272`: change the import of the three symbols to `use crate::bolt_v3_feed_health::{...}` and swap the `phase: <SelectionPhase>` field for `frozen: <bool>` (e.g. `phase: SelectionPhase::Freeze` → `frozen: true`; any non-`Freeze` phase → `frozen: false`). Test files that use **only** the `ForcedFlatReason` variants need just the import-path change.

---

## Task 7: Feed-health hoist — verify behavior-preserving + fence-clean, then commit

- [ ] **Step 1: Run the taker test suite + new inline tests (remote-first)**

Per `AGENTS.md` + `ci/rust-verification.toml [local_compile_policy]` (local cargo refused), run via `just verify-remote` (or the prescribed targeted path) the taker `tests/` families (`exposure`, `orders_admission`, `core_glue`, `book_sizing`, `selection`) plus the new `bolt_v3_feed_health` inline tests.
Expected: PASS with zero behavior change — the forced-flat assertions from Task 4 still hold.

- [ ] **Step 2: Run the dependency-direction fence (scan mode)**

Run: `python3 scripts/verify_bolt_v3_dependency_direction.py`
Expected: `OK` / zero findings — confirm `bolt_v3_feed_health.rs` contains **no** `crate::strategies::...` reference (the `frozen: bool` redesign removed the only `SelectionPhase` link).

- [ ] **Step 3: Run the shrink-only fence**

Run: `python3 scripts/verify_bolt_v3_dependency_direction.py --check-shrink-only-vs-main`
Expected: PASS with `FINDING_ALLOWANCES` unchanged. **Do NOT add an allowance** — the shrink-only check fails any new entry; if this step fails, the `SelectionPhase` coupling was not fully severed (fix the code, not the allowlist).

- [ ] **Step 4: Run the FR-080 fence + venue-name recipe (no regression)**

Run: `python3 scripts/verify_bolt_v3_no_venue_name_branch.py`
Expected: `OK` (the new `bolt_v3_feed_health.rs` introduces no venue-name branch).

- [ ] **Step 5: Commit**

```bash
git add src/bolt_v3_feed_health.rs src/lib.rs \
  src/strategies/binary_oracle_edge_taker/exposure.rs \
  src/strategies/binary_oracle_edge_taker/mod.rs \
  src/strategies/binary_oracle_edge_taker/tests/orders_admission.rs \
  src/strategies/binary_oracle_edge_taker/tests/core_glue.rs \
  src/strategies/binary_oracle_edge_taker/tests/book_sizing.rs
python3 ~/.claude/lib/safe_git.py commit -m "refactor(488): hoist forced-flat feed-health predicates to shared bolt_v3_feed_health (behavior-preserving)"
```

---

## Task 8: Docs hygiene — correct the stale order-budget assumption (§16#9)

**Files:**
- Modify: `specs/488-binary-oracle-maker/spec.md:177`

- [ ] **Step 1: Fix the stale `100/second` assumption**

In `specs/488-binary-oracle-maker/spec.md`, change the order-budget assumption from `100/second` to `40/min (config/root.toml:413; NT max_order_submit_rate)`. If `spec.md:178` still claims `VenueContract` "cannot hold the maker capability variables," correct it — `VenueContract` already carries execution/rate_budget/maintenance_window/depth_availability/fee_schedule/settlement at schema_version=3 (§9).

**Do NOT touch `plan.md:68,114,131`** (the `src/bte_ingest.rs` references): per spec §14#8 the real BTE module is not yet confirmed — that fix is deferred to when the maker plan is (re)written against the BTE epic (#437/#438). Do not substitute a guessed path.

- [ ] **Step 2: Commit**

```bash
git add specs/488-binary-oracle-maker/spec.md
python3 ~/.claude/lib/safe_git.py commit -m "docs(488): correct stale order-budget assumption in spec (100/second -> 40/min, §16#9)"
```

---

## Self-Review

**Spec coverage (§16#12, §15, §16#9):**
- §16#12 FR-080 venue-name fence → Tasks 1-3. ✓
- §15 stale-feed gate hoist to shared module → Tasks 4-7. ✓
- §16#9 stale `spec.md` assumption → Task 8 (the clean part; `plan.md` BTE path explicitly deferred per §14#8). ✓

**Placeholder scan:** No "TBD/TODO/handle appropriately" — all code is complete; moves cite exact line ranges; the deferred `plan.md` fix is an explicit, justified exclusion, not a placeholder. ✓

**Type consistency:** `ForcedFlatInputs.frozen: bool` replaces `phase: SelectionPhase` consistently across the new module, both `mod.rs` callers, and the three test files. `ForcedFlatReason` variants/order unchanged. `evaluate_forced_flat_predicates` signature unchanged (still `&ForcedFlatInputs -> Vec<ForcedFlatReason>`). Fence symbols (`find_violations_in_text`, `collect_violations`, `collect_violations_from_files`) match between the test (Task 1) and the verifier (Task 2). ✓

**Fence interaction (the load-bearing risk):** the new `src/bolt_v3_feed_health.rs` is under the `src/bolt_v3_` scan prefix, so it must not reference `crate::strategies`; the `frozen: bool` redesign is what keeps it clean. Task 7 Steps 2-3 verify this in both fence modes and forbid allowlist growth. ✓
