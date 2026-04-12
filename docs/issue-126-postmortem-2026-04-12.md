## Purpose

This file is a preservation artifact for issue `#126` / PR `#131`.

It is intentionally long and not a high-level summary. It records:

- all external review rounds that were run against PR `#131`
- the findings from each reviewer lane
- how each round was answered
- residual risks and unresolved concerns
- the point where the user stopped further solutioning and requested research instead
- the exact uncommitted exploratory diff that existed at that stop point
- the research procedure that followed
- the resulting research findings
- the exact current state at the end of this session

This is meant to make later postmortem or continuation work possible without reconstructing the session from memory.

## Objects

- Issue: `#126` `Phase 3b: resolve Polymarket depth support vs request-snapshot fallback path`
- PR: `#131` `Phase 3b: formalize Polymarket fallback contract`
- Repo: `seungpyoson/bolt-v2`
- Clean authoritative main used during this work: `origin/main` at `005d489e2c80a2a84bc6e0870a28266706595823`
- `#126` work branch: `issue-126-polymarket-depth-followup-r2`
- Last coherent pushed branch head before the user stopped further solutioning: `be61a02f122dab4167dcadaf7c82cd7e4a447b67`

## Why `#126` existed

The narrow truth question for `#126` was:

- either Polymarket depth is a supported platform capability, or
- it is not, and the repo must formalize one fallback execution path instead.

The explicit instruction for the slice was:

- work only on issue `#126` as a platform/runtime slice
- do not drift into `#110` strategy logic
- do not leave both paths half-supported
- do not overclaim support in the venue contract

## What `#126` initially established

The initial coherent outcome of the `#126` branch was:

- native Polymarket `order_book_depths` remained unsupported
- the venue contract stayed truthful
- the branch formalized a fallback execution path instead of claiming native depth support
- the branch added config/runtime plumbing for fallback-related knobs in the `exec_tester` seam

The coherent branch state before later user objections was represented by PR `#131`.

## External Review Rounds

There were **2 external review rounds** on PR `#131`, followed by **1 internal subagent review round** and then the user stop/research pivot.

### External Review Round 1

#### Review target

- PR `#131`
- head SHA at the time of request: `57a0c8e4d0374837521691eda94d2349f41574a7`

#### Review package used

The review request explicitly asked reviewers to evaluate:

- whether the PR overclaimed native Polymarket depth support
- whether contract wording, runtime behavior, tests, and wording told one single fallback story
- whether the PR drifted into `#110`
- whether the fallback seam was precise enough for `#110`

#### GLM findings, round 1

GLM produced the following findings:

1. `MEDIUM`: duplicated `default_book_interval_ms` magic value
   - files: `src/live_config.rs` and `src/strategies/exec_tester.rs`
   - issue: the same `1_000` default existed in two places
   - requested fix: extract a shared constant or shared function

2. `LOW`: no config-level validation for `book_interval_ms = 0`
   - file: `src/live_config.rs`
   - issue: parse/materialize accepted `0`, and the error surfaced only later at strategy construction
   - requested fix: reject `0` in config validation

3. `LOW`: no config-level validation for `open_position_time_in_force`
   - file: `src/live_config.rs`
   - issue: arbitrary strings were accepted at config time and only failed later in `exec_tester`
   - requested fix: validate allowed `TimeInForce` values earlier

4. `OBSERVATION`: `RenderedStrategyConfig` always emitted `subscribe_book` and `book_interval_ms`
   - file: `src/live_config.rs`
   - noted as harmless/backward-compatible

5. `CLEAN`: no overclaim of native depth support
   - contract, config comments, and test names/assertions all consistently said unsupported + fallback

6. `CLEAN`: fallback story internally consistent
   - contract reason -> config fields -> runtime builder -> tests told one story

7. `CLEAN` with caveat: `open_position_*` fields were plumbing, not strategy logic
   - caveat: PR description should mention these fields as part of declared scope

8. `CLEAN`: backward compatibility preserved

GLM bottom line in round 1:

- the PR truthfully froze the `#126` platform/runtime contract
- the actionable items were Findings 1-3
- Finding 1 was called out as the one that should be fixed before merge

#### Minimax findings, round 1

Minimax verdict:

- `APPROVE`
- no blocking issues found

Minimax positive findings:

- no overclaim of native Polymarket depth support
- contract / config / runtime / tests told one fallback story
- fallback wording was precise enough for `#110`
- no drift into `#110` or `#127`
- changes looked like the minimum necessary set

Minimax minor non-blocking observations:

- type consistency note: `book_interval_ms` was `u64` in config/rendered types but converted to `NonZeroUsize` in the builder
- commented example fields for `open_position_*` were appropriate
- `rust_decimal` dependency was required for decimal parsing

#### Claude findings, round 1

Claude panel result:

- DeepSeek V3.2 Reasoner: pass
- MiniMax M2.7: pass
- merge gate: passed

Claude conclusion in round 1:

- no unresolved findings
- contract wording, runtime/config behavior, and tests told a single truthful fallback story
- no overclaim of native depth support
- no drift into `#110` or `#127`

#### Gemini findings, round 1

Gemini verdict:

- severity none / clean
- no defects found

Gemini positive alignment checks:

- no hardcoded runtime trading values introduced; the seam was TOML-driven
- change locality remained in the `[strategy]` slice
- backward compatibility was preserved with `Option` fields and `skip_serializing_if`
- no overclaim of native depth support
- contract / runtime / tests told one consistent fallback story

#### Response to round 1

The following fixes were made after round 1:

- removed the duplicated `default_book_interval_ms` source
- moved `book_interval_ms = 0` rejection into config validation
- moved invalid `open_position_time_in_force` rejection into config validation
- tightened `open_position_on_start_qty` so invalid/zero values also failed during config validation instead of later
- updated the PR body to reflect the validation changes and current scope

Resulting pushed head after round 1 response:

- `ee0f019b44ae2e33183753546e8cc341ef54083d`

### External Review Round 2

#### Review target

- PR `#131`
- head SHA at the time of request: `ee0f019b44ae2e33183753546e8cc341ef54083d`

#### Claude findings, round 2

Claude produced the following findings:

1. `LOW`: `open_position_on_start_qty` silently accepted negative values
   - files: `src/validate.rs`, `src/validate/tests.rs`
   - detail: validation rejected zero and unparseable decimals but explicitly accepted `-1.5`
   - interpretation: this implied that signed decimal encoded side
   - Claude position: not blocking for `#126`, but this convention should be documented if intentional

2. `LOW`: lowercase TIF acceptance depended on upstream NT parser behavior
   - files: `src/validate.rs`, `src/strategies/exec_tester.rs`, `src/validate/tests.rs`
   - detail: acceptance of `"fok"` worked because the NT parser accepted it, not because the branch normalized case itself
   - suggestion: normalize case locally or explicitly accept the coupling

3. `NIT`: redundant runtime check on `book_interval_ms`
   - file: `src/strategies/exec_tester.rs`
   - detail: runtime builder still had a `NonZeroUsize` check even after config-level validation existed
   - Claude position: harmless defense in depth

Claude bottom line in round 2:

- pass on scope and contract truth
- two low items worth noting
- not blocking

#### Gemini findings, round 2

Gemini verdict:

- severity none / clean
- no defects found

Gemini positive findings in round 2:

- validation was now correctly moved earlier into `src/validate.rs`
- backward compatibility remained intact
- shared default logic no longer duplicated between config layers
- no overclaim of native Polymarket depth support
- one fallback story across contract, runtime, config, and tests

#### GLM findings, round 2

GLM produced the following:

1. `LOW`: no cross-field validation for the `open_position_*` pair
   - file: `src/validate.rs`
   - issue: config could specify only quantity or only time-in-force and still pass validation
   - requested fix: paired-presence rule

2. `LOW`: `book_interval_ms` validated as positive even when `subscribe_book = false`
   - file: `src/validate.rs`
   - GLM position: likely okay and consistent with existing always-positive validation patterns
   - no action required

3. `OBSERVATION`: negative `open_position_on_start_qty` accepted
   - file: `src/validate.rs`, `src/validate/tests.rs`
   - GLM position: acceptable for `#126` platform scope because sign semantics belonged to strategy semantics

GLM also explicitly marked the previous three review findings from round 1 as resolved.

GLM bottom line in round 2:

- the only remaining actionable item was the cross-field validation gap
- PR otherwise clean and truthful

#### Minimax findings, round 2

Minimax verdict:

- `APPROVE`
- no blocking issues found

Minimax emphasized:

- invalid fallback settings were now failing during config validation rather than later at node startup
- validation flow now matched the desired lifecycle
- no drift into `#110` or `#127`

#### Response to round 2

After round 2, the branch was moved further to tighten the seam:

- paired-presence validation for `open_position_on_start_qty` and `open_position_time_in_force`
- explicit FOK-only interpretation for the accepted fallback entry seam
- additional runtime validation mirroring

This led to the coherent pushed branch head:

- `be61a02f122dab4167dcadaf7c82cd7e4a447b67`

At that point, the branch state was:

- native `order_book_depths` unsupported
- accepted fallback path formalized
- `open_position_on_start_qty` and `open_position_time_in_force` had to appear together
- accepted current time-in-force for the fallback entry seam was encoded as `FOK`
- signed quantity was intentionally accepted

### Internal subagent review after round 2

After the second external round, an internal subagent-driven review pass was also used.

Subagents used:

- verifier: `Wegener`
- code-reviewer: `Bacon`

Verifier result:

- passed with no findings

Code-reviewer result:

1. runtime `validate_runtime` did not yet mirror the new fallback-field validation
2. the entry-TIF seam still overclaimed beyond the accepted FOK-only contract

Response:

- branch was tightened so runtime validation mirrored the new seam rules
- PR body and branch were updated to match the branch head `be61a02`

## User objections after the above work

After the branch had reached the coherent `be61a02` state, the user raised an architectural objection.

The user objected specifically to the following code-level policy couplings:

- a code-level default for `book_interval_ms`
- code-level validation that the current fallback entry seam uses `FOK`
- code-level paired validation that `open_position_on_start_qty` and `open_position_time_in_force` must appear together

The user’s position was:

- in the long run the platform should be order-type agnostic
- the platform should not encode strategy policy like `FOK` vs maker vs taker vs partial
- the user did not want a hardcoded `book_interval_ms`
- all parameters should “go in one place”
- the correct next step was not to continue patching policy but to understand the complete path first

That objection is the direct reason the branch stopped moving as an implementation branch and pivoted into a research/postmortem state.

## Interrupted exploratory refactor after the user objection

After the user objected to policy hardcoding, an exploratory refactor was started in the `#126` worktree to relax some of the policy coupling.

That exploratory refactor was **not completed** and **was not coherent** with the rest of the branch because `src/validate.rs` and validation tests still reflected the older FOK-oriented assumptions.

The exploratory refactor was only in:

- `src/live_config.rs`
- `src/strategies/exec_tester.rs`

The exact uncommitted diff at the stop point was:

```diff
diff --git a/src/live_config.rs b/src/live_config.rs
index 100f94a..cdfde6e 100644
--- a/src/live_config.rs
+++ b/src/live_config.rs
@@ -70,12 +70,6 @@ fn default_order_qty() -> String {
     "5".to_string()
 }
 
-pub(crate) const DEFAULT_BOOK_INTERVAL_MS: u64 = 1_000;
-
-pub(crate) fn default_book_interval_ms() -> u64 {
-    DEFAULT_BOOK_INTERVAL_MS
-}
-
 pub(crate) fn parse_time_in_force_token(raw: &str) -> Result<TimeInForce, String> {
     let trimmed = raw.trim();
     let normalized = trimmed.to_ascii_uppercase();
@@ -85,6 +79,14 @@ pub(crate) fn parse_time_in_force_token(raw: &str) -> Result<TimeInForce, String
         .map_err(|e| e.to_string())
 }
 
+pub(crate) fn parse_polymarket_time_in_force(raw: &str) -> Result<TimeInForce, String> {
+    let tif = parse_time_in_force_token(raw)?;
+    match tif {
+        TimeInForce::Gtc | TimeInForce::Gtd | TimeInForce::Fok | TimeInForce::Ioc => Ok(tif),
+        other => Err(format!("unsupported TimeInForce for Polymarket: {other}")),
+    }
+}
+
 fn default_tob_offset_ticks() -> u64 {
     5
 }
@@ -259,8 +261,8 @@ pub struct LiveStrategyInput {
     pub log_data: bool,
     #[serde(default)]
     pub subscribe_book: bool,
-    #[serde(default = "default_book_interval_ms")]
-    pub book_interval_ms: u64,
+    #[serde(default)]
+    pub book_interval_ms: Option<u64>,
     #[serde(default)]
     pub open_position_on_start_qty: Option<String>,
     #[serde(default)]
@@ -284,7 +286,7 @@ impl Default for LiveStrategyInput {
             order_qty: default_order_qty(),
             log_data: false,
             subscribe_book: false,
-            book_interval_ms: default_book_interval_ms(),
+            book_interval_ms: None,
             open_position_on_start_qty: None,
             open_position_time_in_force: None,
             tob_offset_ticks: default_tob_offset_ticks(),
@@ -512,7 +514,8 @@ struct RenderedStrategyConfig {
     order_qty: String,
     log_data: bool,
     subscribe_book: bool,
-    book_interval_ms: u64,
+    #[serde(skip_serializing_if = "Option::is_none")]
+    book_interval_ms: Option<u64>,
     #[serde(skip_serializing_if = "Option::is_none")]
     open_position_on_start_qty: Option<String>,
     #[serde(skip_serializing_if = "Option::is_none")]
```

```diff
diff --git a/src/strategies/exec_tester.rs b/src/strategies/exec_tester.rs
index e7d77a3..931e3c9 100644
--- a/src/strategies/exec_tester.rs
+++ b/src/strategies/exec_tester.rs
@@ -1,8 +1,7 @@
 use std::{num::NonZeroUsize, str::FromStr};
 
-use crate::live_config::parse_time_in_force_token;
+use crate::live_config::parse_polymarket_time_in_force;
 use nautilus_model::{
-    enums::TimeInForce,
     identifiers::{ClientId, InstrumentId, StrategyId},
     types::Quantity,
 };
@@ -22,8 +21,8 @@ pub struct ExecTesterInput {
     pub log_data: bool,
     #[serde(default)]
     pub subscribe_book: bool,
-    #[serde(default = "crate::live_config::default_book_interval_ms")]
-    pub book_interval_ms: u64,
+    #[serde(default)]
+    pub book_interval_ms: Option<u64>,
     #[serde(default)]
     pub open_position_on_start_qty: Option<String>,
     #[serde(default)]
@@ -46,9 +45,6 @@ pub fn build_exec_tester(raw: &Value) -> Result<ExecTester, Box<dyn std::error::
     let instrument_id = InstrumentId::from(cfg.instrument_id.as_str());
     let strategy_id = StrategyId::from(cfg.strategy_id.as_str());
     let client_id = ClientId::new(cfg.client_id);
-    let book_interval_ms = NonZeroUsize::new(cfg.book_interval_ms as usize)
-        .ok_or("book_interval_ms must be greater than zero")?;
-
     let mut config = ExecTesterConfig::builder()
         .base(StrategyConfig {
             strategy_id: Some(strategy_id),
@@ -60,7 +56,6 @@ pub fn build_exec_tester(raw: &Value) -> Result<ExecTester, Box<dyn std::error::
         .order_qty(Quantity::from(cfg.order_qty.as_str()))
         .log_data(cfg.log_data)
         .subscribe_book(cfg.subscribe_book)
-        .book_interval_ms(book_interval_ms)
         .use_post_only(cfg.use_post_only)
         .tob_offset_ticks(cfg.tob_offset_ticks)
         .enable_limit_sells(cfg.enable_limit_sells)
@@ -73,16 +68,16 @@ pub fn build_exec_tester(raw: &Value) -> Result<ExecTester, Box<dyn std::error::
             Some(Decimal::from_str(open_position_on_start_qty.as_str())?);
     }
 
+    if let Some(book_interval_ms) = cfg.book_interval_ms {
+        config.book_interval_ms = NonZeroUsize::new(book_interval_ms as usize)
+            .ok_or("book_interval_ms must be greater than zero")?;
+    }
+
     if let Some(open_position_time_in_force) = cfg.open_position_time_in_force {
-        let time_in_force = parse_time_in_force_token(open_position_time_in_force.as_str())
-            .map_err(|e| format!("invalid open_position_time_in_force: {e}"))?;
-        if time_in_force != TimeInForce::Fok {
-            return Err(format!(
-                "open_position_time_in_force must be FOK for the current Polymarket fallback entry seam, got {time_in_force}"
-            )
-            .into());
-        }
-        config.open_position_time_in_force = time_in_force;
+        config.open_position_time_in_force = parse_polymarket_time_in_force(
+            open_position_time_in_force.as_str(),
+        )
+        .map_err(|e| format!("invalid open_position_time_in_force: {e}"))?;
     }
 
     Ok(ExecTester::new(config))
```

Important note about this exploratory refactor:

- it was started **after** the user objected to hardcoded policy
- it was **not** finished
- it was inconsistent with `src/validate.rs` and validation tests
- it did **not** represent a coherent branch state
- it was the point where the user explicitly stopped further solutioning and asked for research instead

## Research request

After objecting to further policy patching, the user explicitly requested:

- a logically sound answer
- hard evidence
- research
- use of subagents if needed
- no guessing

The question being researched was effectively:

- what phase 3 actually needed
- whether phase 3 existed for good reason
- whether the branch churn reflected a fundamentally wrong slice or a different specific problem

## Research procedure used in this session

The research was conducted in the following way:

1. Checked current repo/worktree state and authoritative branches.
   - confirmed the original local `main` was stale
   - confirmed `origin/main` authoritative head was `005d489e2c80a2a84bc6e0870a28266706595823`

2. Pulled exact GitHub issue bodies for:
   - `#110`
   - `#118`
   - `#126`
   - `#129`
   - `#132`
   - and related context issues `#37`, `#109`, `#121`

3. Pulled PR `#128` metadata and diff context, because `#128` was the draft branch carrying the `#118` work.

4. Compared authoritative `origin/main` code against the `#118` branch head.
   - `origin/main` target: `005d489e2c80a2a84bc6e0870a28266706595823`
   - `#118` branch target: `27e7ef47140c50bb80d046ff9413dd8bce06d7c9`

5. Inspected the exact code paths needed to answer the question.
   - `src/platform/ruleset.rs`
   - `src/platform/polymarket_catalog.rs`
   - `src/platform/runtime.rs`
   - `src/live_config.rs`
   - `src/strategies/exec_tester.rs`
   - `tests/platform_runtime.rs`

6. Checked branch ancestry directly.
   - `git merge-base --is-ancestor origin/issue-118-runtime-enablement origin/main`
   - result: exit code `1`
   - meaning the `#118` branch head is **not** merged into authoritative `origin/main`

7. Checked the exact commit delta from `origin/main` to the `#118` branch.
   - `git log origin/main..origin/issue-118-runtime-enablement`
   - result:
     - `27e7ef4 Close startup preemption gap in runtime`
     - `bc574bc Remove redundant issue #118 fallback note`
     - `bd9bbb3 Complete issue #118 runtime enablement seams`

8. Checked diff concentration between `origin/main` and the `#118` branch.
   - largest concentration was in:
     - `src/platform/runtime.rs`
     - `tests/platform_runtime.rs`
     - `src/strategies/exec_tester.rs`
     - `src/live_config.rs`

9. Spawned three subagents for independent evidence lanes.
   - analyst: issue/PR chain and dependency story
   - explorer: authoritative main code path vs `#118` branch contrast
   - critic: challenge the tentative conclusion and point out any inferential leaps

## Research findings

### Research finding 1: `#110` is not a simple “replace exec_tester” issue

Evidence:

- `#110` explicitly expects the strategy to:
  - consume the eligible ETH 5m and 15m candidate set already exposed by runtime
  - consume Chainlink via the shared reference pipeline
  - dynamically arbitrate across venues
  - track interval open price, warmup, recovery, cooldowns, re-entry accounting
  - compute conservative executable EV using live depth and live fee inputs supplied by runtime/platform
  - enforce fail-closed behavior on stale or missing feeds/book/metadata

Conclusion:

- `#110` was always broader than “swap out the placeholder strategy class”
- it assumed richer runtime/platform contracts already existed

### Research finding 2: authoritative `origin/main` does not provide the runtime contract that `#110` assumes

Evidence on `origin/main`:

- `CandidateMarket` lacks candidate timing and fee metadata:
  - `src/platform/ruleset.rs@005d489:8-16`
- `SelectionEvaluation` lacks the ranked eligible candidate set:
  - `src/platform/ruleset.rs@005d489:67-120`
- runtime only sends `Activate { instrument_id }` / `Clear` to the managed strategy:
  - `src/platform/runtime.rs@005d489:758-771`
- `build_runtime_exec_tester` injects only `instrument_id`:
  - `src/platform/runtime.rs@005d489:774-788`
- `LiveStrategyInput` on `origin/main` does not expose fallback book/open-position knobs:
  - `src/live_config.rs@005d489:236-253`
- `exec_tester` on `origin/main` only has the old basic knobs:
  - `src/strategies/exec_tester.rs@005d489:11-52`

Conclusion:

- from the point of view of authoritative `main`, the runtime/platform contract needed by `#110` does not yet exist

### Research finding 3: the `#118` branch does add the missing strategy-facing seams

Evidence on the `#118` branch:

- added `start_ts_ms`, `maker_base_fee_bps`, `taker_base_fee_bps` to `CandidateMarket`:
  - `src/platform/ruleset.rs@27e7ef4:8-18`
- added `eligible_candidates` to `SelectionEvaluation`:
  - `src/platform/ruleset.rs@27e7ef4:71-129`
- filled the new metadata in the Polymarket catalog path:
  - `src/platform/polymarket_catalog.rs@27e7ef4:104-118`
- added `RuntimeSelectionSnapshot` and strategy-facing selection handoff:
  - `src/platform/runtime.rs@27e7ef4:125-153`
  - `src/platform/runtime.rs@27e7ef4:831-836`
- added the fallback-related strategy config surface:
  - `src/live_config.rs@27e7ef4:236-280,492-509,694-710`
- added the `exec_tester` builder surface for the fallback knobs:
  - `src/strategies/exec_tester.rs@27e7ef4:15-78`

Conclusion:

- `#118` was not imaginary work
- it was adding missing runtime/platform seams that `#110` actually assumes

### Research finding 4: the real remaining gap is lifecycle, and that is why `#129` exists

Evidence:

- on the `#118` branch, runtime still removes the current strategy and adds a new one when the market changes:
  - `src/platform/runtime.rs@27e7ef4:977-1055`
- tests on the `#118` branch explicitly encode replacement on market switch:
  - `tests/platform_runtime.rs@27e7ef4` contains the `active_market_switch_replaces_runtime_strategy_with_new_market` behavior
- issue `#129` exists precisely to keep one persistent runtime-managed strategy instance alive across market switches so `#110` can keep state in the strategy layer

Conclusion:

- the biggest objectively real follow-up after `#118` is not “native depth”
- it is cross-market persistent strategy lifetime so `#110` can remain strategy-pure

### Research finding 5: `#126` may have overstated the amount of remaining ambiguity

Evidence:

- `contracts/polymarket.toml` remains truthful and unsupported for `order_book_depths`
- the `#118` branch already proved a concrete delta-backed fallback path for `exec_tester`
- the `#118` branch also proved FOK opening-order submission in that branch’s scope

Conclusion:

- the remaining question in `#126` was narrower than “do we have any book path at all?”
- a concrete fallback path had already been demonstrated on the `#118` branch
- however, because `#118` was not merged, authoritative `main` still did not have that path

### Research finding 6: the earlier “wrong seam” conclusion was too strong

Initial tentative claim that was later challenged:

- “Phase 3 did not close cleanly because the work was sliced at the wrong seam.”

After challenge and evidence review, the stronger corrected conclusion is:

- the issue decomposition itself was intentional and defensible
- `#118` was an intentional runtime-enablement slice
- `#126` was an intentional platform-contract slice
- `#129` was an intentional lifecycle slice
- the cleaner evidence-backed statement is not that the seam was obviously wrong, but that:
  - `#118` is still branch-only, not landed in authoritative `main`
  - `#118` leaves a real cross-market lifecycle gap that `#129` legitimately exists to solve
  - `#126` branch work then began encoding more platform policy than the user was comfortable with

## Internal research subagent outputs

### Analyst subagent result

The analyst lane concluded:

- `#110` materially requires more than a strategy swap
- `#118`, `#126`, and `#129` were each intended to supply a specific prerequisite
- none of those prerequisite branches is merged into authoritative `main`

### Explorer subagent result

The explorer lane concluded:

- `origin/main` can only activate or clear a runtime-managed strategy by instrument id
- `origin/main` cannot give the strategy the full selected market object, ranked eligible set, or candidate timing/fee metadata
- `origin/main` cannot give the runtime-managed `exec_tester` the fallback book/open-position seam knobs
- the `#118` branch adds those missing surfaces

### Critic subagent result

The critic lane explicitly challenged the earlier “wrong seam” conclusion and returned:

- the stronger corrected conclusion is narrower
- the evidence supports:
  - `#118` as an intentional runtime-enablement slice
  - `#129` as a real remaining prerequisite
  - `#126` as somewhat overstating ambiguity because the `#118` branch already proved a fallback path
- the evidence does **not** support the broader claim that the original seam was obviously wrong

## Residual risks after the research pass

1. The coherent `#126` branch head `be61a02` may still reflect platform-policy choices the user no longer wants encoded in Bolt.
   - specifically:
     - hardcoded default-like behavior around `book_interval_ms`
     - FOK-oriented seam enforcement
     - paired-field validation policy

2. The stopped exploratory refactor after the user objection was not completed and did not reach a coherent compile/validation state.

3. Authoritative `origin/main` still does not contain the runtime-enablement branch work from `#118`.

4. The real remaining prerequisite for `#110` appears to be persistent strategy lifetime across market switches (`#129`), and that work is not landed.

5. The branch/issue chain for phase 3 has generated review churn because the coherent `#126` branch state and the user’s desired architectural direction diverged after the second review round.

## Current state at the end of this session

Current state being preserved here:

- coherent pushed `#126` branch head: `be61a02f122dab4167dcadaf7c82cd7e4a447b67`
- PR `#131` exists and reflects that coherent head
- user then stopped further solutioning and requested research instead
- the only subsequent code state was the interrupted two-file exploratory refactor shown above
- that exploratory state was not coherent and should not be treated as the branch’s accepted code state

## Preservation intent

The intent of this document is:

- later sessions should not need to reconstruct the review history
- later sessions should not need to guess why the branch stopped
- later sessions should know the difference between:
  - the coherent pushed branch state (`be61a02`)
  - the later interrupted exploratory refactor (never coherent, never pushed)
  - the later research findings (which changed the interpretation of what phase 3’s remaining problem actually is)

