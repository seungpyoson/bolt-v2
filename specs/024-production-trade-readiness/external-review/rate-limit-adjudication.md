# Rate-Limit / Venue-Egress Tier-1 Adjudication — PR #480 add-on

Anchored at HEAD `f54181f0`, pinned NT rev `6e059dcbb59ac1e582132fc431a581936c216c3c`.
6 external models. Every fact below re-verified personally against HEAD bytes / pinned NT
source — not promoted from any reviewer verdict.

## Verdict: NEEDS-CHANGES — one LIVE-MONEY-CRITICAL gap (5/6 models), confirmed real

DeepSeek=UNSAFE, GPT=NEEDS-CHANGES, Gemini=NEEDS-CHANGES, Grok=UNSAFE, Kimi=NEEDS-CHANGES,
GLM=SOUND (lone dissent — see below). All hardening/overflow/cap-source/const items from the
prior review verify CLEAN and FIXED at `f54181f0` (overflow checked-math, u128 compare,
fail-closed unmodeled-venue, `SECONDS_PER_*` + `PRIVATE_ARTIFACT_FILE_MODE` consts, cap alias).

## The gap (CONFIRMED — three facts, personally re-verified)

`validate_order_rate_within_venue_egress` (src/bolt_v3_validate.rs:1024) reconciles the NT
RiskEngine submit/modify **command** rate 1:1 against the Polymarket REST **request** cap
(100/min). That 1:1 assumption is false on the production path:

1. **Production fires market orders.** `config/strategies/binary_oracle.toml`:
   `exit_order.order_type = "market"` (:91), `forced_exit_order.order_type = "market"` (:100).
   Entry is limit (:82). The taker's exit/forced-exit path is market.
2. **One market submit = 2 REST requests.** NT `submit_market_order`
   (adapters/polymarket/src/execution/submitter.rs:144) calls `get_book` (:165) **then**
   `post_order` (:226), each wrapped in `execute_with_retry`. (Market BUY may add a collateral
   fetch elsewhere → up to 3; floor within the fn is 2.) Limit submit = 1 (`post_order`).
   Modify = 0 (rejected locally, execution/mod.rs:1272).
3. **The check counts 1 command ≈ 1 request** (validate.rs:1073) and its error message
   (:1077-1081) tells operators that keeping rate ≤ cap stops egress blocking.

Net: at 100 cmd/min (the ceiling the error message calls safe), the market-exit path emits
~200 REST/min against the 100/min cap → silent egress blocking with stale quotes on a
live-money exit — the exact failure this guard exists to prevent. The check is
anti-conservative by the per-command fanout (~2×).

**Scope:** the docstring's #488 deferral note (:1021-1023) defers the *shared* budget across
*other* call types (cancels + status + readiness/account probes). It does NOT cover the fanout
*within* a single submit. So this sits inside Tier-1's own declared scope, not deferred.

**GLM dissent — outvoted and wrong on this point:** GLM only considered transient *retries*
(which it called unobservable at config time) and missed the **deterministic** market-order
`get_book` fanout, which IS knowable at config time (order type is config). Its "conservative"
framing is inverted: command_rate ≤ cap with fanout=2 permits REST_rate up to 2×cap, i.e. it
*permits* an over-drive — anti-conservative, not conservative.

## Approved fix (Option A — model the fanout, fail-closed/tighter)

Model the worst-case REST-requests-per-order-command as a per-venue capability, sourced like
the existing cap (NO HARDCODES — a modeled venue fact with NT-source provenance, not a runtime
literal). Derate the ceiling to `cap / fanout`.

1. **`src/bolt_v3_providers/polymarket.rs`** — new const beside `REST_EGRESS_CAP_PER_MINUTE`:
   `pub const MAX_REST_REQUESTS_PER_ORDER_COMMAND: u32 = 2;` with a doc comment citing
   nautilus_trader@6e059dc submitter.rs `submit_market_order` (get_book + post_order = 2;
   limit = 1; modify = 0). Worst case (2) is used because the global submit/modify throttle
   does not distinguish order type. Excludes transient RetryManager retries (#501 headroom —
   the agnostic venue egress-capability contract). ALSO re-point validate.rs:1023's existing
   "#488" deferral note to #501 (#488 is the maker umbrella, not the owner of this contract).
2. **`src/bolt_v3_providers/mod.rs`** — extend the venue-egress dispatch. Prefer grouping both
   values (cap + fanout) so a venue's egress model is fetched in ONE call (group-by-change):
   either a small `VenueEgressModel { cap_per_minute, max_rest_requests_per_order_command }`
   returned by one fn, or a sibling `venue_max_rest_requests_per_order_command(venue)`. Decide
   against the POST-workflow state of this file (G2 is mid-editing it).
3. **`src/bolt_v3_validate.rs:1073`** — reconcile `limit × fanout × 60 > cap × interval`
   (u128). Update the error message: the venue cap is `{cap}/min` but each order command costs
   up to `{fanout}` REST requests (market submit = book + post), so the rate must not exceed
   `{cap/fanout}/00:01:00`. Apply the same fanout to both submit and modify rates (uniform,
   conservative; modify's true cost is 0 so this only tightens).

## Tests (TDD — reproduction first)

- **Reproduction (RED→GREEN):** a Polymarket-execution config with
  `max_order_submit_rate = "100/00:01:00"` must now be **rejected** (100×2=200 > 100). Find +
  update any existing test that asserts 100/min PASSES (it encodes the old buggy ceiling).
- **New boundary:** `50/00:01:00` PASSES (50×2=100 ≤ 100); `51/00:01:00` fails (102 > 100).
- Unmodeled-venue fail-closed test (config_parsing.rs:~5416) unaffected.

## Sequencing constraint

Fix touches `providers/mod.rs` + `polymarket.rs`, both owned by the running fix-workflow
(`w13hnr04c`, G2 cluster). MUST implement AFTER the workflow completes + its clusters are
re-verified + committed, against the post-workflow file state. Then: fences + rustfmt + commit
+ CI. This is a fail-closed TIGHTENING; never loosen the guard.

## Full review adjudication — every rate-limit finding (6 models @ `f54181f0` / NT `6e059dc`)

GLM's overall verdict was RATE-LIMIT-SOUND; it is **overruled on R1** (5 models + my own
re-verification). All anchors below personally checked at HEAD / pinned NT source.

| ID | Finding | Raised by | Class | Verdict @ HEAD | Action |
|----|---------|-----------|-------|----------------|--------|
| R1 | 1 command ≠ 1 REST request (market submit = `get_book`+`post_order` = 2); production fires market exits | DeepSeek, GPT, Gemini, Grok, Kimi | LIVE-MONEY | **CONFIRMED real** | Must-have `×fanout`+margin (queued); #501 |
| R2 | Retry amplification (`max_retries`=3 → up to 4× per request) | GLM, GPT, Grok, Gemini, Kimi | HARDENING (non-deterministic) | CONFIRMED | Margin (must-have) gives headroom; full model → #501 |
| R3 | Docstring "shared REST budget" wrong — CLOB & Gamma are SEPARATE per-client 100/min buckets | GLM, Grok | HARDENING (doc accuracy) | CONFIRMED (rate_limits.rs:25-30 = two `Quota`) | Correct docstring "shared"→"separate per-client buckets" in must-have fix; topology owned by #501 |
| R4 | modify-rate checked vs cap, but Polymarket modify = 0 REST | GLM, GPT, Gemini | NIT (dead-but-harmless) | CONFIRMED (execution/mod.rs:1272-1284) | NO ACTION — keep provider-agnostic; fanout applies conservatively |
| R5 | Overflow: checked math on unbounded `hours` | DeepSeek, GLM, GPT, Grok, Gemini | HARDENING | VERIFIED CLEAN (validate.rs:1003-1007) | None |
| R6 | u128 comparison (no saturation bypass) | GLM, GPT, Grok, Gemini | HARDENING | VERIFIED CLEAN (validate.rs:1073-1074) | None |
| R7 | Fail-closed on unmodeled execution venue | DeepSeek, GLM, GPT, Grok | HARDENING | VERIFIED CLEAN/FIXED (validate.rs:1042-1051) | None |
| R8 | `SECONDS_PER_*` consts (no bare literals) | DeepSeek, GLM | NIT | VERIFIED CLEAN | None |
| R9 | `PRIVATE_ARTIFACT_FILE_MODE = 0o600` const | DeepSeek, GLM | NIT | reviewer-VERIFIED; covered by runtime-literal fence | None |
| R10 | Cap source `HTTP_RATE_LIMIT=100` per-minute alias correct | DeepSeek, GLM, GPT, Grok | VERIFY | CONFIRMED (consts.rs; rate_limits.rs) | None |
| R11 | Tightest-ceiling (min-cap) selection correct | GLM, GPT | VERIFY | VERIFIED CLEAN (validate.rs:1038) | None |

**Net:** 1 live-money (R1) → must-have fix + #501; 2 hardening folded (R2 retries via
margin+#501, R3 docstring corrected in-fix + #501); 1 NIT accepted-by-design (R4); 7
verified-clean (R5–R11), no action. Every prior-review actionable (overflow, silent-skip)
confirmed FIXED at `f54181f0`.

## External re-review (round 1) — HEAD 633ea9f0 (R1 fix `92480763` on base `f54181f0`)

6 models re-reviewed the R1 fix. **4 CLOSE-CONFIRMED** (DeepSeek, Gemini, Grok, Kimi); **2
STILL-OPEN** (GLM, GPT) on a new LIVE-MONEY finding, confirmed real against the pinned NT source.

- **R12 (LIVE-MONEY) — CONFIRMED + FIXED.** A market + quote-quantity BUY issues a 3rd REST
  request (`fetch_collateral_balance_pusd`, NT `6e059dc` `execution/mod.rs:559,586` — only when
  `side==Buy && is_quote_quantity`), so `MAX_REST_REQUESTS_PER_ORDER_COMMAND = 2` under-counts.
  The archetype rejects `is_quote_quantity` for exits/forced-exits but NOT for entries
  (`check_entry_order_combination`), and entries are BUYs — so a config with
  `entry_order = market + quote_qty` was a reachable over-drive (40/min × 3 = 120 > 100 cap).
  Re-verified personally against NT `6e059dc`. **FIX (operator decision — Option A):** forbid
  `entry_order` `order_type=market` + `is_quote_quantity=true` at load
  (`check_entry_order_combination`), making fanout=2 the provable worst-case across allowed
  configs; production (limit entry, 40/min) is unaffected. Test:
  `bolt_v3_archetype_rejects_market_quote_quantity_entry_order` (config_parsing.rs). The
  order-template-aware fanout (Option C — re-enable the mode by modeling fanout per order
  shape) is filed as **#506**.
- **HARDENING (DeepSeek H1/H2) — FIXED.** (H1) the `MAX_REST_REQUESTS_PER_ORDER_COMMAND` comment
  now documents the excluded 3rd-REST collateral fetch and why fanout=2 stays correct (the entry
  combo is forbidden; exits are SELLs). (H2) schema-doc example `max_order_submit/modify_rate`
  updated 100 → 40/00:01:00 to match the derated config.
- **R4–R11:** re-verified clean at HEAD by the 6 models; no regression.

**Closure:** R1 + R12 fixed; pending a round-2 re-review of the fix before close.
