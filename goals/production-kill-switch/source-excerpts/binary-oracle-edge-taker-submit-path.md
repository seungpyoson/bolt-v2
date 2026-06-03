# Binary Oracle Edge Taker Submit Path Excerpt

This excerpt replaces `src/strategies/binary_oracle_edge_taker.rs` in the external review packet because the full file is 589246 bytes and exceeds the direct API reviewer 262144 byte per-file limit.

Source file: `src/strategies/binary_oracle_edge_taker.rs`

Extraction commands:

- `rg -n "record_order_intent|submit_admission|submit_order|RiskReducingExit|ReplaceSubmit|OrderIntent|admission|submit" src/strategies/binary_oracle_edge_taker.rs`
- `sed -n '3770,3842p' src/strategies/binary_oracle_edge_taker.rs`
- `sed -n '4228,4318p' src/strategies/binary_oracle_edge_taker.rs`
- `sed -n '4412,4552p' src/strategies/binary_oracle_edge_taker.rs`
- `sed -n '7136,7340p' src/strategies/binary_oracle_edge_taker.rs`

## Relevant Line Map

- Lines 3775-3792: shared submit helper records order intent, builds admission request, calls submit admission, then calls NT `submit_order`.
- Lines 3794-3837: admission request maps entry intents to `BoltV3SubmitIntentKind::Entry` and exit intents to `BoltV3SubmitIntentKind::RiskReducingExit`.
- Lines 4231-4318: exit submit path builds an exit order and routes through the shared submit helper.
- Lines 4415-4550: entry submit path builds an entry order and routes through the shared submit helper after recording strategy input evidence.
- Lines 7139-7338: tests prove evidence/admission failures reject before NT submit and armed admission reaches NT submit.

## Shared Submit Helper

```rust
fn submit_order_with_decision_evidence(
    &mut self,
    intent: BoltV3OrderIntentEvidence,
    order: nautilus_model::orders::OrderAny,
    submit_context: SubmitContext,
) -> Result<()> {
    self.context
        .decision_evidence()
        .record_order_intent(&intent)?;
    let request = self.submit_admission_request_from_order(&intent, &order)?;
    let _permit = self.context.submit_admission().admit(&request)?;
    self.submit_order(
        order,
        submit_context.position_id,
        submit_context.client_id,
        submit_context.params,
    )
}
```

## Admission Intent Mapping

```rust
Ok(BoltV3SubmitAdmissionRequest {
    strategy_id: intent.strategy_id.clone(),
    client_order_id,
    instrument_id: order.instrument_id().to_string(),
    notional,
    intent_kind: match intent.intent_kind {
        BoltV3OrderIntentKind::Entry => BoltV3SubmitIntentKind::Entry,
        BoltV3OrderIntentKind::Exit => BoltV3SubmitIntentKind::RiskReducingExit,
    },
    lifecycle_policy: self.submit_lifecycle_policy(),
})
```

## Exit Submit Route

```rust
let intent = BoltV3OrderIntentEvidence::from_compiled_order(
    self.config.strategy_id.clone(),
    BoltV3OrderIntentKind::Exit,
    price.to_string(),
    &order,
);

if let Err(error) = self.submit_order_with_decision_evidence(
    intent,
    order,
    SubmitContext::with_client_id_and_position_id(
        client_id,
        managed_position.position.position_id,
    ),
) {
    self.clear_pending_exit_state();
    return Err(error);
}
```

## Entry Submit Route

```rust
let intent = BoltV3OrderIntentEvidence::from_compiled_order(
    self.config.strategy_id.clone(),
    BoltV3OrderIntentKind::Entry,
    price.to_string(),
    &order,
);

if let Err(error) = self
    .context
    .decision_evidence()
    .record_strategy_input_snapshot(&strategy_input_snapshot)
    .and_then(|()| {
        self.submit_order_with_decision_evidence(
            intent,
            order,
            SubmitContext::with_client_id(client_id),
        )
    })
{
    self.clear_pending_entry_state();
    return Err(error);
}
```

## Submit-Path Tests

```rust
#[test]
fn decision_evidence_failure_rejects_before_nt_submit() {
    let error = strategy
        .submit_order_with_decision_evidence(
            intent,
            order,
            SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
        )
        .expect_err("evidence failure must reject before NT submit");

    assert!(
        error.to_string().contains("intent write failed"),
        "{error:#}"
    );
}

#[test]
fn unarmed_submit_admission_rejects_after_evidence_before_nt_submit() {
    let error = strategy
        .submit_order_with_decision_evidence(
            intent,
            order,
            SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
        )
        .expect_err("unarmed submit admission must reject before NT submit");

    assert!(
        error.to_string().contains("submit admission is not armed"),
        "{error:#}"
    );
    assert_eq!(submit_admission.admitted_order_count(), 0);
}

#[test]
fn armed_submit_admission_allows_nt_submit_after_evidence() {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        strategy.submit_order_with_decision_evidence(
            intent,
            order,
            SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
        )
    }));

    assert!(
        result.is_err(),
        "test strategy is intentionally not registered with NT; reaching NT submit should panic"
    );
    assert_eq!(submit_admission.admitted_order_count(), 1);
}
```
