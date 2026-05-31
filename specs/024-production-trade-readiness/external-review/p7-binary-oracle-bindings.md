# PR #480 External Adversarial Review — P7: `binary_oracle_edge_taker` Strategy Bindings

You are a hostile, senior Rust reviewer auditing the **configuration bindings and wiring** of a trading strategy in a live-money system, against two project rules: *"NO HARDCODES — every runtime value comes from TOML config"* and *"NO DUAL PATHS — one way to do each thing; single source of truth."* Your job is to **break the claim that every binding is config-sourced and singular**, or state honestly you cannot.

**System:** `bolt-v2` — pure-Rust Polymarket trading bot on NautilusTrader. PR #480 (base `53fd50d2`, HEAD `ece4c2a5`). The strategy is `binary_oracle_edge_taker`. Real orders, real money.

**GROUND RULES:**
1. Judge only the embedded bytes. If a claim depends on unshown code, mark **UNVERIFIABLE** and name what you'd need.
2. No bluffing; cite specific lines.
3. A hedged verdict is not an approval.

---

### CLAIM TO BREAK

> Every binding of the `binary_oracle_edge_taker` strategy is **config-sourced with one source of truth and no dual path**:
> - `reference_venue` and `reference_instrument_id` are `Option<String>` deserialized from TOML, and are **co-required** — configuring exactly one of the pair is a hard validation error (`missing_reference_data_pair`).
> - The reference source is a **mutual exclusion**: a market binds its reference from *either* an NT `reference_data` client *or* a source-owned `decision_reference` provider, never both — the `(Some, Some)` case is `unreachable!` because it is rejected upstream.
> - The runtime readiness seed's `reference_venue` is **resolved from the source-owned `decision_reference` provider id**, not a hardcoded venue string. The per-strategy seed is bound by matching `strategy_instance_id`.
> - No venue / instrument / client **ID** is a string literal anywhere in the binding surface — all resolve from config or from a sha256-pinned `strategy_input_evidence` file.

### ATTACK SURFACE

1. **Hardcoded id:** find any binding that takes a venue/instrument/client id from a string literal instead of config or the pinned seed file.
2. **Break the co-requirement:** find a config shape where `reference_venue` is set without `reference_instrument_id` (or vice versa) that slips past `missing_reference_data_pair` — e.g., a code path that reads one field before the validation, or a default that fills the other.
3. **Break the mutual exclusion:** is `(Some, Some) => unreachable!()` *actually* unreachable? If both can be set, this is either a panic (DoS) or a silent wrong-path. Trace whether the "rejected above" guard is shown — if it isn't in the embedded bytes, mark that sub-claim UNVERIFIABLE.
4. **Fail-open via `Option`:** `reference_instrument_id()` now returns `Option<InstrumentId>` using `InstrumentId::from_str(..).ok()`. Does a **malformed** configured instrument id silently become `None` → `subscribe_reference_quotes` silently skips subscribing → the strategy runs with **no reference data** and makes entry decisions blind? Is that a fail-**open** regression versus the old `InstrumentId::from(..)` (which would have panicked/surfaced)? This is the sharpest concern — judge it explicitly.
5. **Seed fallback:** does `reference_venue` resolution in the runtime seed (`source_owned_decision_reference_provider_id(...)`) have any literal fallback if the source-owned provider is absent?

---

### EMBEDDED CODE (PR #480, scoped binding hunks; base→HEAD)

**`src/strategies/binary_oracle_edge_taker.rs` — config field declaration + accessors + validation + source wiring:**

```diff
@@ macro_rules! binary_oracle_edge_taker_config_fields {
             market_selection_rule: String => String;
             retry_interval_seconds: u64 => Integer;
             blocked_after_seconds: u64 => Integer;
-            reference_venue: String => String;
-            reference_instrument_id: String => String;
             use_uuid_client_order_ids: bool => Boolean;
@@ macro_rules! define_config_struct {
         #[serde(deny_unknown_fields)]
         struct BinaryOracleEdgeTakerConfig {
             $( $field: $ty, )+
+            reference_venue: Option<String>,
+            reference_instrument_id: Option<String>,
             entry_order: BinaryOracleEdgeTakerOrderConfig,
@@ impl BinaryOracleEdgeTaker {  // reference observation now gated on configured venue
         let observed_ts_ms = quote.ts_event.as_u64() / NANOS_PER_MILLI_U64;
+        let venue_name = self.config.reference_venue.as_ref()?;
         Some(FastSpotObservation {
-            venue_name: self.config.reference_venue.clone(),
+            venue_name: venue_name.clone(),
             price: midpoint,
             observed_ts_ms,
         })
@@ impl BinaryOracleEdgeTaker {  // instrument-id accessor now Option, parses via from_str().ok()
-    fn reference_instrument_id(&self) -> InstrumentId {
-        InstrumentId::from(self.config.reference_instrument_id.as_str())
+    fn reference_instrument_id(&self) -> Option<InstrumentId> {
+        self.config
+            .reference_instrument_id
+            .as_deref()
+            .and_then(|instrument_id| InstrumentId::from_str(instrument_id).ok())
     }

     fn subscribe_reference_quotes(&mut self) {
-        let instrument_id = self.reference_instrument_id();
-        #[cfg(not(test))]
-        self.subscribe_quotes(instrument_id, None, None);
+        if let Some(instrument_id) = self.reference_instrument_id() {
+            #[cfg(not(test))]
+            self.subscribe_quotes(instrument_id, None, None);
+            #[cfg(test)]
+            let _ = instrument_id;
+        }
     }
     // unsubscribe_reference_quotes mirrors the same Option-guard
@@ impl BinaryOracleEdgeTakerBuilder {  // field recognition + co-required pair validation
                 ENTRY_ORDER_FIELD | EXIT_ORDER_FIELD | FORCED_EXIT_ORDER_FIELD
+                    | "reference_venue"
+                    | "reference_instrument_id"
                     | binary_oracle_edge_taker_config_fields!(match_config_field_names)
             ) { Self::push_unknown_field(errors, format!("{field_prefix}.{key}"), key); }
+        Self::validate_optional_string_field(table, field_prefix, "reference_venue", errors);
+        Self::validate_optional_string_field(table, field_prefix, "reference_instrument_id", errors);
+        if table.contains_key("reference_venue") != table.contains_key("reference_instrument_id") {
+            let missing = if table.contains_key("reference_venue") {
+                "reference_instrument_id"
+            } else { "reference_venue" };
+            Self::push_missing(errors, format!("{field_prefix}.{missing}"),
+                "missing_reference_data_pair", BinaryOracleEdgeTakerFieldType::String);
+        }
@@ impl StrategyBuilder for BinaryOracleEdgeTakerBuilder {  // source-owned evidence wiring
+    let config = BinaryOracleEdgeTakerBuilder::parse_config(raw_config)?;
+    let reference_venue = config.reference_venue.as_ref().ok_or_else(|| {
+        anyhow::anyhow!("reference quote observation source requires configured reference_venue")
+    })?;
+    let reference_instrument_id = config.reference_instrument_id.as_ref().ok_or_else(|| {
+        anyhow::anyhow!("reference quote observation source requires configured reference_instrument_id")
+    })?;
+pub fn record_entry_decision_evidence_from_source(
+    raw_config: &Value,
+    decision_evidence: Arc<dyn crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter>,
+    trader_id: TraderId, source: &BinaryOracleEntryDecisionEvidenceSource,
+    instruments: &[InstrumentAny],
+) -> Result<()> {
+    let fee_provider = Arc::new(SourceFeeProvider {
+        fee_bps_by_instrument_id: source_fee_bps_by_instrument_id(source)?,
+    });
+    let submit_admission = Arc::new(
+        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(
+            decision_evidence.clone()));      // <-- starts UNARMED
+    let context = StrategyBuildContext::new(fee_provider, decision_evidence, submit_admission)
+        .with_readiness_evidence(readiness_evidence);
+    let mut strategy = BinaryOracleEdgeTaker::new(
+        BinaryOracleEdgeTakerBuilder::parse_config(raw_config)?, context);
```

**`src/strategies/registry.rs` — `StrategyBuildContext` binding container (the wiring carrier):**

```diff
@@ pub struct StrategyBuildContext {
     fee_provider: Arc<dyn FeeProvider>,
     decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
     submit_admission: Arc<BoltV3SubmitAdmissionState>,
+    readiness_evidence: Option<BoltV3ReadinessGateEvidenceSnapshot>,
+    runtime_readiness_seed: Option<BoltV3RuntimeReadinessSeed>,
 }
@@ impl StrategyBuildContext {  // new() defaults both new fields to None; builders set them
+    pub fn with_readiness_evidence(mut self, e: BoltV3ReadinessGateEvidenceSnapshot) -> Self {
+        self.readiness_evidence = Some(e); self }
+    pub fn with_runtime_readiness_seed(mut self, s: BoltV3RuntimeReadinessSeed) -> Self {
+        self.runtime_readiness_seed = Some(s); self }
+    pub fn readiness_evidence(&self) -> Option<&BoltV3ReadinessGateEvidenceSnapshot> {
+        self.readiness_evidence.as_ref() }
+    pub fn runtime_readiness_seed(&self) -> Option<&BoltV3RuntimeReadinessSeed> {
+        self.runtime_readiness_seed.as_ref() }
 }
```

**`src/bolt_v3_strategy_registration.rs` — per-strategy seed binding (the `reference_venue` resolution):**

```diff
@@ pub struct StrategyRegistrationContext<'a> {
     pub resolved: &'a ResolvedBoltV3Secrets,
     pub decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
     pub submit_admission: Arc<BoltV3SubmitAdmissionState>,
+    pub readiness_evidence: Option<BoltV3ReadinessGateEvidenceSnapshot>,
+    pub runtime_readiness_seed: Option<BoltV3RuntimeReadinessSeed>,
 }
@@ // The seed FILE is deserialized; every id comes from this sha256-pinned file, not a literal
+#[derive(Deserialize)]
+struct StrategyInputRuntimeSeedFile {
+    strategy_instance_id: Option<String>,
+    gate_session_hash: Option<String>,
+    selected_market_key: Option<String>,
+    gate_evidence: Option<BTreeMap<String, BoltV3GateEvidenceIdentity>>,
+    realized_volatility: String, spot_price: String, price_to_beat_value: String,
+    reference_quote_ts_event: u64,
+    polymarket_condition_id: String, polymarket_market_slug: String, polymarket_question_id: String,
+    up_instrument_id: String, down_instrument_id: String,
+    polymarket_market_start_timestamp_ms: u64, polymarket_market_end_timestamp_ms: u64,
+}
@@ pub fn register_bolt_v3_strategies_on_node_with_bindings(...)  // seed bound by matching instance id
+                runtime_readiness_seed: readiness_evidence
+                    .as_ref()
+                    .filter(|evidence| {
+                        evidence.strategy_instance_id == strategy.config.strategy_instance_id.as_str()
+                    })
+                    .and_then(|evidence| evidence.runtime_seed.clone()),
@@ // reference_venue resolves from the SOURCE-OWNED decision_reference provider id (no literal)
+    let reference_venue = source_owned_decision_reference_provider_id(
+        loaded, snapshot, input.gate_evidence.as_ref())?;
+    Ok(Some(BoltV3RuntimeReadinessSeed {
+        strategy_instance_id: strategy_instance_id.to_string(),
+        polymarket_condition_id: required_owned_seed_string("polymarket_condition_id",
+            input.polymarket_condition_id)?,
+        up_instrument_id: required_owned_seed_string("up_instrument_id", input.up_instrument_id)?,
+        down_instrument_id: required_owned_seed_string("down_instrument_id", input.down_instrument_id)?,
+        reference_venue,
+    }))
```

> Note: the `(reference_data, decision_reference)` mutual-exclusion match — including `(Some, Some) => unreachable!("dual reference paths are rejected above")` and the `(None, Some(decision_reference)) => reference_venue = decision_reference.provider_id` arm — lives in `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs` and is the subject of review slice **P3**. If your assessment of attack #3 depends on seeing the "rejected above" guard, mark that sub-point UNVERIFIABLE here and defer it to P3.

---

### OUTPUT FORMAT (required)

```
VERDICT: HOLDS | FLAWED | UNCERTAIN | UNVERIFIABLE
BREAK (if FLAWED): <the config shape / code path that violates config-sourcing or single-path, citing lines>
FAIL-OPEN RULING (attack #4): <does a malformed reference_instrument_id silently disable reference
   subscription? is that a fail-open regression? cite reference_instrument_id() + subscribe_reference_quotes>
MISSING (if UNVERIFIABLE): <unshown code needed>
EVIDENCE:
  - <line> : <ruling>
```

No praise. Attack #4 (fail-open via `Option`) must get an explicit ruling.
