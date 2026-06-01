# PR #480 External Adversarial Review — P6: Hardcodes Audit

You are a hostile, senior Rust reviewer enforcing a **strict no-hardcodes rule** on a live-money trading system. The project rule (verbatim): *"NO HARDCODES — every runtime value comes from TOML config. No string literals for IDs, quantities, timeouts, or any runtime value in code."* Your job is to **find a hardcoded runtime value that violates this rule**, or to state honestly that you cannot find one in the embedded evidence.

**System:** `bolt-v2` — pure-Rust Polymarket trading bot on NautilusTrader. PR #480 (base `53fd50d2`, HEAD `ece4c2a5`). Real orders, real money.

**What counts as a VIOLATION (a real hardcode):** a literal in non-test Rust source that is a *runtime/operational* value — a venue/instrument/client **ID**, an order **quantity/size**, a **price**, a **notional or bps cap**, a **timeout/duration** (seconds/ms/nanos), a **retry/cadence** interval, or a tunable strategy **parameter** — that should instead be read from TOML config.

**What is NOT a violation:** protocol/ABI layout constants, math/unit-conversion denominators (bps `10_000`, millis `1_000`, nanos `1_000_000_000`), schema-version stamps, sentinel zeros, and identifier/record-kind/JSON-field-name string constants. These are not operator-tunable runtime values.

**GROUND RULES:**
1. Judge only the embedded evidence. If you suspect a hardcode in code not shown, name the file/symbol and mark that point **UNVERIFIABLE**.
2. No bluffing. Cite the specific item.
3. "Looks clean" without engaging each suspect item is not an approval.

---

### CLAIM TO BREAK

> No runtime value is hardcoded in the changed **non-test** Rust source of PR #480. Every config-driven value (notional cap, timeouts, cadence, retry delays, bps params, volatility windows, strategy parameters) is read from TOML into struct/Wire fields or passed as a **variable** into `Duration::from_*`. Every added numeric literal is a protocol/ABI/math/unit constant, a schema-version stamp, or a sentinel; every added string literal is a record-kind / JSON-field-name / venue-identity / forbidden-env-var **identifier** constant. The audit below found **zero** genuine runtime hardcodes in non-test code.

### THE OTHER HALF OF THE CLAIM — the allowlist (challenge this hardest)

The repo has an automated hardcode fence: `scripts/verify_bolt_v3_runtime_literals.py` (runs in CI's source-fence job) checks source literals against an **allowlist** file `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`. In PR #480 that allowlist grew by **+9,792 / −1,000 (net +8,792 lines)**. The fence script itself is unchanged in this range.

> **The bypass risk:** a real hardcode can be hidden from CI by simply adding it to the allowlist. The +8,792 new allowlist entries were **NOT line-audited** in producing this packet. The reviewer must reason about what a malicious or careless allowlist entry would look like, and tell the operator exactly which allowlist entries to spot-check: **any allowlist entry whose value is a venue/instrument/client ID, an order size/quantity, a price, a timeout/duration, a cadence interval, or a notional/bps cap is a real hardcode parked in the allowlist — not a legitimate constant.**

### ATTACK SURFACE

1. Challenge any item in the "config-driven" list below: is it *really* sourced from TOML, or is there a literal default hiding via `serde(default = "...")`, `unwrap_or(LITERAL)`, or `unwrap_or_else(|| LITERAL)`? (The audit asserts no such defaults exist — disprove it if you can reason a likely spot.)
2. Challenge the two borderline numeric items (`PAGE_SIZE = 100`, `FEE_PROBE_SIZE = 1`): are they genuinely protocol/math constants, or smuggled trade parameters?
3. Is there any `Duration::from_secs(LITERAL)` / `from_millis(LITERAL)` where the argument is a number, not a variable? (Audit says all are variables.)
4. Reason about the allowlist delta: describe the smallest set of grep patterns the operator should run against `bolt-v3-runtime-literal-audit.toml` to surface a parked hardcode.

---

### EMBEDDED EVIDENCE — curated hardcode audit (non-test `src/**/*.rs`, base→HEAD)

**Verification commands used (reproducible):**
```
git diff 53fd50d2 ece4c2a5 -- src | grep -E '^\+' | \
  grep -iE '"[a-z0-9_\-]+"|[0-9]{2,}|Duration::|_secs|_millis|_nanos|basis_point|notional|quantity|min_|max_|timeout'
git diff --numstat 53fd50d2 ece4c2a5 -- docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml
```

**=== SUSPECT runtime values (configurable trade params that must come from TOML) ===**
```
(none found in non-test code)
```

**=== Config-driven values — CORRECTLY sourced from TOML (shown to prove the negative) ===**
```
src/bolt_v3_archetypes/binary_oracle_edge_taker.rs  Wire{ warmup_tick_count, reentry_cooldown_secs,
   book_impact_cap_bps, risk_lambda, exit_hysteresis_bps, vol_window_secs, vol_gap_reset_secs,
   vol_min_observations, vol_bridge_valid_secs, pricing_kurtosis, theta_decay_factor,
   forced_flat_thin_book_min_liquidity, lead_agreement_min_corr, lead_jitter_max_ms }
   -> all deserialized from [parameters.runtime] TOML; no literal/serde defaults
   (spot-verified: reentry_cooldown_secs/book_impact_cap_bps are bare struct fields fed from
    parameters.runtime.*; grep for serde(default) on these returns nothing)
src/bolt_v3_live_node.rs  Duration::from_secs(timeout_secs | stop_timeout_secs | start_timeout_secs
   | walk_timeout_secs)  -> argument is always a config-fed VARIABLE, never a literal
src/bolt_v3_canary_proof_executor.rs  sleep(Duration::from_millis(retry_delay_ms))  -> variable
src/...  Duration::from_*(execution.http_timeout_secs | execution.retry_delay_initial_ms
   | live_canary.reference_quote_max_age_seconds | reference_quote_wait_timeout_seconds) -> config fields
src/bolt_v3_live_canary_gate.rs  max_notional_per_order: Decimal -> sourced from
   [live_canary].max_notional_per_order; entry/proof notional validated against it, no literal cap
```

**=== Likely-legit: protocol / ABI / math / unit-conversion constants (NOT runtime-tunable) ===**
```
NANOS_PER_SECOND = 1_000_000_000 ; MILLIS_PER_SECOND_U64 = 1_000          (time units)
CHAINLINK_REPORT_ABI_WORD_BYTES = 32 ; _BLOB_OFFSET_WORD_INDEX = 3 ;
   _CALLBACK_MIN_BYTES = 4*WORD ; _V3_WORD_COUNT = 9 ; _V3_*_WORD_INDEX (0/1/2/6)  (EVM ABI layout)
CHAINLINK_REPORT_BASE256_RADIX = 256.0 ; _DECIMAL_RADIX = 10.0                (radices)
CHAINLINK_DATA_STREAMS_HMAC_BLOCK_BYTES = 64 ; CHAINLINK_FEED_ID_HEX_LENGTH = 64   (crypto/hex)
PRE_RUN_CLOB_V2_COLLATERAL_ACCOUNTING_BPS_DENOMINATOR = 10_000 ; ENTRY_DECISION_FEE_BPS_SCALE = 10_000.0
   ; ENTRY_FEE_BPS_SCALE = 10_000 ; SUBMIT_ADMISSION_BPS_DENOMINATOR = 10_000   (bps denominators)
REQUIRED_UPDOWN_OUTCOME_INSTRUMENT_COUNT = 2                                  (binary up/down arity)
ON_CHAIN_COLLATERAL_JSON_RPC_ID = 1 ; EVM_WORD_HEX_LEN = 64 ; EVM_ADDRESS_HEX_LEN = 40  (JSON-RPC/EVM)
EXTERNAL_SNAPSHOT_NO_REMAINING_RETRIES = 0 ; RETRY_DECREMENT = 1             (loop control)
*_SCHEMA_VERSION = 1 (ENTRY_DECISION_EVIDENCE_SOURCE_SCHEMA_VERSION = 2)     (schema stamps)
(latency_millis.len()*95).div_ceil(100)                                      (p95 percentile math)
```

**=== Two BORDERLINE items examined and cleared (challenge these) ===**
```
src/bolt_v3_providers/polymarket/venue_account_state_source.rs
   const POLYMARKET_DATA_API_PAGE_SIZE: u32 = 100;
   -> used ONLY as the Polymarket Data-API pagination batch size; the fetch loop terminates
      when `count < POLYMARKET_DATA_API_PAGE_SIZE` (:265). Vendor-API protocol constant, not a
      trade-tunable value. [spot-verified at HEAD :23, :233, :263, :265]
src/bolt_v3_providers/polymarket/entry_decision_source_inputs.rs
   const ENTRY_DECISION_FEE_PROBE_SIZE: i64 = 1;
   -> used as Decimal::from(PROBE_SIZE) at :776 to normalize a per-unit fee into bps
      (fee_bps = commission / entry_price * SCALE). Unit-quantity math constant, not an order size.
      [spot-verified at HEAD :42, :776]
```

**=== Identifier / record-kind / field-name string constants (NOT runtime values) ===**
```
src/bolt_v3_providers/market_data.rs  BITMEX_KEY..KRAKEN_KEY = "BITMEX".."KRAKEN"  (NT venue-kind identity)
   *_CREDENTIAL_LOG_MODULES = &["nautilus_*::common::credential"]               (log-fence module names)
   *_FORBIDDEN_ENV_VARS (per-venue testnet+prod key/secret/passphrase var names) (credential-fence deny-list)
src/bolt_v3_config.rs  TEST_DOUBLE_PROVIDER_KIND / CHAINLINK_DATA_STREAMS_PROVIDER_KIND / NO_RESOLUTION_KIND
   / RESOLUTION_GATE_ROLE / DECISION_REFERENCE_GATE_ROLE / PRICE_GATE_VALUE_KIND / GATE_PROVIDER_KINDS[]
   / GATE_ROLES[] / GATE_VALUE_KINDS[] / SSM_CREDENTIAL_PARAMETER_FIELD       (enum/role identity strings)
src/bolt_v3_canary_proof_policy.rs  CANARY_PROOF_*_RECORD_KIND / CANARY_PROOF_CLAIM = "proof_only"  (evidence kinds)
src/bolt_v3_market_families/{mod,updown}.rs  SELECTED_MARKET_*_FIELD / METADATA_*_FIELD / JSON field names ;
   BINARY_OPTION_MARKET_CLASS = "binary_option"                              (JSON field + market-class id)
src/bolt_v3_no_submit_readiness.rs  DATA_CLIENT_*_RECORD_KIND / *_STATUS_* / VENUE_NATIVE_PROVIDER_KIND  (record-kind ids)
src/bolt_v3_operator_artifacts.rs / src/bolt_v3_submit_admission.rs  "execution_client_id" /
   "venue_order_state_path" / "venue_order_outcome"                          (JSON field names)
```

**=== Excluded as test data (post `#[cfg(test)]`) ===**
```
e.g. live_canary_gate.rs fixtures "0.50"/"polymarket", "1.00"/"5.00"/"10.00" notional fixtures,
Price::from("1.00") / Quantity::from("2.00") — test data, not runtime.
```

---

### OUTPUT FORMAT (required)

```
VERDICT: HOLDS | FLAWED | UNCERTAIN | UNVERIFIABLE
HARDCODE FOUND (if FLAWED): <file/symbol + value + why it is a runtime value that must be config>
ALLOWLIST RISK: <the grep patterns the operator must run against bolt-v3-runtime-literal-audit.toml,
   and what a parked hardcode would look like>
BORDERLINE: <your independent ruling on PAGE_SIZE=100 and FEE_PROBE_SIZE=1>
EVIDENCE:
  - <item> : <ruling>
```

No praise. Engage each suspect item or mark it UNVERIFIABLE.
