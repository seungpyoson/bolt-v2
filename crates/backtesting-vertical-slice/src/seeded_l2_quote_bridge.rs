use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    rc::Rc,
    str::FromStr,
};

use anyhow::{Context, Result, bail, ensure};
use nautilus_backtest::engine::BacktestEngine;
use nautilus_common::{
    actor::{DataActor, DataActorConfig, DataActorCore, DataActorNative},
    msgbus::{self, switchboard},
    nautilus_actor,
};
use nautilus_model::{
    data::{OrderBookDelta, OrderBookDeltas, QuoteTick},
    enums::{BookType, RecordFlag},
    identifiers::{ActorId, ClientId, InstrumentId},
    orderbook::OrderBook,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    catalog_projection::{NT_DATA_TYPE_ORDER_BOOK_DELTA, NT_DATA_TYPE_QUOTE_TICK},
    conversion_boundary::{ConversionManifest, SeededL2QuotePlanV1},
    hashing::is_lowercase_sha256_hex,
    reference_artifact::canonical_json_sha256,
    seeded_level_set_deltas::{
        SEEDED_LEVEL_SET_DELTAS_TRANSFORM_IDENTITY, SEEDED_LEVEL_SET_DELTAS_TRANSFORM_VERSION,
    },
};

const PLAN_HASH_DOMAIN: &[u8] = b"seeded-l2-causal-quote-plan.v1\0";
const TRACE_HASH_DOMAIN: &[u8] = b"seeded-l2-causal-quote-trace.v1\0";
const REPORT_SCHEMA_VERSION: &str = "seeded-l2-quote-bridge-report.v1";

pub(crate) struct SeededL2QuoteBridgePlanInput<'a> {
    pub conversion_manifest: &'a ConversionManifest,
    pub client_id: Option<ClientId>,
    pub book_type: BookType,
    pub deltas: &'a [OrderBookDelta],
    pub audit_quotes: &'a [QuoteTick],
}

pub(crate) struct SeededL2QuoteBridgePlan {
    entries: BTreeMap<InstrumentId, InstrumentPlan>,
    plan_hash: String,
}

struct InstrumentPlan {
    client_id: Option<ClientId>,
    book_type: BookType,
    conversion_manifest_hash: String,
    durable_plan: SeededL2QuotePlanV1,
    batches: Vec<BatchPlan>,
    expected_trace_hash: String,
}

#[derive(Serialize)]
struct PlanHashEntry<'a> {
    nt_instrument_id: String,
    client_id: Option<String>,
    book_type: BookType,
    conversion_manifest_hash: &'a str,
    durable_plan: &'a SeededL2QuotePlanV1,
    expected_trace_hash: &'a str,
}

#[derive(Clone, PartialEq, Eq)]
struct BatchPlan {
    semantic_hash: String,
    row_count: usize,
    expected_update_count: u64,
    disposition: BatchDisposition,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BatchDisposition {
    SyntheticSeed,
    OneSided,
    EmitQuote(QuoteTick),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeededL2QuoteBridgeInstrumentReport {
    pub nt_instrument_id: String,
    pub conversion_manifest_hash: String,
    pub observed_event_batches: u64,
    pub observed_source_events: u64,
    pub observed_delta_rows: u64,
    pub emitted_quotes: u64,
    pub causal_trace_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeededL2QuoteBridgeReport {
    pub schema_version: String,
    pub plan_hash: String,
    pub instruments: Vec<SeededL2QuoteBridgeInstrumentReport>,
}

impl SeededL2QuoteBridgeReport {
    pub(crate) fn validate_for_conversion(&self, conversion_manifest_hash: &str) -> Result<()> {
        ensure!(
            self.schema_version == REPORT_SCHEMA_VERSION,
            "unsupported report schema {:?}",
            self.schema_version
        );
        ensure!(
            is_lowercase_sha256_hex(&self.plan_hash),
            "plan_hash is not a lowercase SHA-256 digest"
        );
        ensure!(!self.instruments.is_empty(), "report has no instruments");
        let mut instrument_ids = BTreeSet::new();
        for instrument in &self.instruments {
            ensure!(
                !instrument.nt_instrument_id.trim().is_empty(),
                "report instrument id is empty"
            );
            ensure!(
                instrument_ids.insert(&instrument.nt_instrument_id),
                "duplicate report instrument {}",
                instrument.nt_instrument_id
            );
            ensure!(
                instrument.conversion_manifest_hash == conversion_manifest_hash,
                "instrument {} binds a different conversion manifest",
                instrument.nt_instrument_id
            );
            ensure!(
                is_lowercase_sha256_hex(&instrument.conversion_manifest_hash),
                "instrument {} conversion manifest hash is invalid",
                instrument.nt_instrument_id
            );
            ensure!(
                is_lowercase_sha256_hex(&instrument.causal_trace_hash),
                "instrument {} causal trace hash is invalid",
                instrument.nt_instrument_id
            );
            ensure!(
                instrument.observed_source_events > 0,
                "instrument {} observed no source events",
                instrument.nt_instrument_id
            );
            let synthetic_seed_batches = instrument
                .observed_event_batches
                .checked_sub(instrument.observed_source_events)
                .context("source-event count exceeds observed event batches")?;
            ensure!(
                synthetic_seed_batches <= 1,
                "instrument {} observed more than one synthetic seed batch",
                instrument.nt_instrument_id
            );
            ensure!(
                instrument.observed_delta_rows >= instrument.observed_event_batches,
                "instrument {} observed fewer delta rows than event batches",
                instrument.nt_instrument_id
            );
            ensure!(
                instrument.emitted_quotes <= instrument.observed_source_events,
                "instrument {} emitted more quotes than source events",
                instrument.nt_instrument_id
            );
        }
        Ok(())
    }
}

pub(crate) struct SeededL2QuoteBridgeCapture {
    shared: Rc<RefCell<RuntimeState>>,
}

struct RuntimeState {
    plan_hash: String,
    entries: BTreeMap<InstrumentId, InstrumentRuntime>,
    first_error: Option<String>,
}

struct InstrumentRuntime {
    plan: InstrumentPlan,
    observed_batches: usize,
    observed_delta_rows: u64,
    emitted_quotes: u64,
    trace_hasher: Sha256,
}

struct SeededL2QuoteBridgeActor {
    core: DataActorCore,
    shared: Rc<RefCell<RuntimeState>>,
}

impl Debug for SeededL2QuoteBridgeActor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SeededL2QuoteBridgeActor")
            .field("actor_id", &self.core.actor_id())
            .finish_non_exhaustive()
    }
}

nautilus_actor!(SeededL2QuoteBridgeActor);

pub(crate) fn compile_seeded_l2_quote_bridge_plan(
    inputs: Vec<SeededL2QuoteBridgePlanInput<'_>>,
) -> Result<SeededL2QuoteBridgePlan> {
    ensure!(!inputs.is_empty(), "seeded L2 quote bridge plan is empty");
    let mut entries = BTreeMap::new();

    for input in inputs {
        let manifest = input.conversion_manifest;
        ensure!(
            manifest.fingerprint.converter_identity == SEEDED_LEVEL_SET_DELTAS_TRANSFORM_IDENTITY
                && manifest.fingerprint.converter_version
                    == SEEDED_LEVEL_SET_DELTAS_TRANSFORM_VERSION,
            "seeded L2 quote bridge requires the registered seeded converter identity/version"
        );
        ensure!(
            manifest.nt_data_type == NT_DATA_TYPE_ORDER_BOOK_DELTA,
            "seeded L2 quote bridge requires an OrderBookDelta primary conversion manifest"
        );
        let durable_plan = manifest
            .seeded_l2_quote_plan
            .as_ref()
            .context("seeded L2 conversion manifest has no causal quote plan")?;
        durable_plan.validate()?;
        let instrument_id =
            InstrumentId::from_str(&manifest.nt_instrument_id).with_context(|| {
                format!(
                    "seeded L2 conversion manifest has invalid nt_instrument_id {:?}",
                    manifest.nt_instrument_id
                )
            })?;
        let catalog_rows = manifest.effective_catalog_rows_by_nt_data_type();
        ensure!(
            catalog_rows.get(NT_DATA_TYPE_ORDER_BOOK_DELTA).copied() == Some(input.deltas.len()),
            "seeded L2 conversion manifest delta row count does not match exact catalog read-back"
        );
        ensure!(
            catalog_rows
                .get(NT_DATA_TYPE_QUOTE_TICK)
                .copied()
                .unwrap_or_default()
                == input.audit_quotes.len(),
            "seeded L2 conversion manifest audit quote row count does not match exact catalog read-back"
        );
        ensure!(
            !entries.contains_key(&instrument_id),
            "duplicate seeded L2 quote bridge plan for {}",
            instrument_id
        );
        ensure!(
            input.book_type == BookType::L2_MBP,
            "seeded L2 quote bridge requires manifest-resolved L2_MBP, got {:?}",
            input.book_type
        );

        let batches = compile_batches(&input, instrument_id, durable_plan)?;
        let expected_trace_hash = expected_trace_hash(&batches)?;
        let conversion_manifest_hash = manifest
            .content_hash()
            .context("hash seeded L2 conversion manifest")?;
        entries.insert(
            instrument_id,
            InstrumentPlan {
                client_id: input.client_id,
                book_type: input.book_type,
                conversion_manifest_hash,
                durable_plan: durable_plan.clone(),
                batches,
                expected_trace_hash,
            },
        );
    }

    Ok(SeededL2QuoteBridgePlan {
        plan_hash: hash_plan(&entries)?,
        entries,
    })
}

fn hash_plan(entries: &BTreeMap<InstrumentId, InstrumentPlan>) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(PLAN_HASH_DOMAIN);
    for (instrument_id, plan) in entries {
        let entry_hash = canonical_json_sha256(&PlanHashEntry {
            nt_instrument_id: instrument_id.to_string(),
            client_id: plan.client_id.map(|client_id| client_id.to_string()),
            book_type: plan.book_type,
            conversion_manifest_hash: &plan.conversion_manifest_hash,
            durable_plan: &plan.durable_plan,
            expected_trace_hash: &plan.expected_trace_hash,
        })
        .context("hash seeded L2 causal quote plan entry")?;
        hasher.update(entry_hash.as_bytes());
    }
    Ok(hex::encode(hasher.finalize()))
}

pub(crate) fn install_seeded_l2_quote_bridge(
    engine: &mut BacktestEngine,
    plan: SeededL2QuoteBridgePlan,
) -> Result<SeededL2QuoteBridgeCapture> {
    let actor_id = ActorId::from(plan.plan_hash.as_str());
    let entries = plan
        .entries
        .into_iter()
        .map(|(instrument_id, instrument_plan)| {
            let mut trace_hasher = Sha256::new();
            trace_hasher.update(TRACE_HASH_DOMAIN);
            (
                instrument_id,
                InstrumentRuntime {
                    plan: instrument_plan,
                    observed_batches: 0,
                    observed_delta_rows: 0,
                    emitted_quotes: 0,
                    trace_hasher,
                },
            )
        })
        .collect();
    let shared = Rc::new(RefCell::new(RuntimeState {
        plan_hash: plan.plan_hash,
        entries,
        first_error: None,
    }));
    engine
        .add_actor(SeededL2QuoteBridgeActor {
            core: DataActorCore::new(DataActorConfig {
                actor_id: Some(actor_id),
                ..Default::default()
            }),
            shared: Rc::clone(&shared),
        })
        .context("register seeded L2 causal quote bridge")?;
    Ok(SeededL2QuoteBridgeCapture { shared })
}

impl SeededL2QuoteBridgeCapture {
    pub(crate) fn finalize(self) -> Result<SeededL2QuoteBridgeReport> {
        let shared = self.shared.borrow();
        if let Some(error) = &shared.first_error {
            bail!("seeded L2 causal quote bridge failed: {error}");
        }
        let mut instruments = Vec::with_capacity(shared.entries.len());
        for (instrument_id, runtime) in &shared.entries {
            ensure!(
                runtime.observed_batches == runtime.plan.batches.len(),
                "seeded L2 causal quote bridge observed {} of {} batches for {instrument_id}",
                runtime.observed_batches,
                runtime.plan.batches.len()
            );
            let observed_source_events = runtime
                .observed_batches
                .checked_sub(usize::from(
                    runtime.plan.durable_plan.synthetic_seed_batches,
                ))
                .context("observed seeded L2 batches are shorter than the synthetic seed")?;
            ensure!(
                observed_source_events as u64 == runtime.plan.durable_plan.selected_source_events,
                "seeded L2 causal quote bridge source-event count mismatch for {instrument_id}"
            );
            let causal_trace_hash = hex::encode(runtime.trace_hasher.clone().finalize());
            ensure!(
                causal_trace_hash == runtime.plan.expected_trace_hash,
                "seeded L2 causal quote bridge trace mismatch for {instrument_id}"
            );
            instruments.push(SeededL2QuoteBridgeInstrumentReport {
                nt_instrument_id: instrument_id.to_string(),
                conversion_manifest_hash: runtime.plan.conversion_manifest_hash.clone(),
                observed_event_batches: runtime.observed_batches as u64,
                observed_source_events: observed_source_events as u64,
                observed_delta_rows: runtime.observed_delta_rows,
                emitted_quotes: runtime.emitted_quotes,
                causal_trace_hash,
            });
        }
        Ok(SeededL2QuoteBridgeReport {
            schema_version: REPORT_SCHEMA_VERSION.to_string(),
            plan_hash: shared.plan_hash.clone(),
            instruments,
        })
    }
}

impl DataActor for SeededL2QuoteBridgeActor {
    fn on_start(&mut self) -> Result<()> {
        for (instrument_id, client_id, book_type) in self.planned_subscriptions() {
            if self.cache().order_book(&instrument_id).is_some() {
                self.latch_failure(format!(
                    "preexisting order book for planned instrument {instrument_id}"
                ));
                return Ok(());
            }
            self.subscribe_book_deltas(instrument_id, book_type, None, client_id, true, None);
        }
        Ok(())
    }

    fn on_stop(&mut self) -> Result<()> {
        for (instrument_id, client_id, _) in self.planned_subscriptions() {
            self.unsubscribe_book_deltas(instrument_id, client_id, None);
        }
        Ok(())
    }

    fn on_book_deltas(&mut self, deltas: &OrderBookDeltas) -> Result<()> {
        if let Err(error) = self.process_batch(deltas) {
            self.latch_failure(format!("{error:#}"));
        }
        Ok(())
    }
}

impl SeededL2QuoteBridgeActor {
    fn planned_subscriptions(&self) -> Vec<(InstrumentId, Option<ClientId>, BookType)> {
        self.shared
            .borrow()
            .entries
            .iter()
            .map(|(instrument_id, runtime)| {
                (
                    *instrument_id,
                    runtime.plan.client_id,
                    runtime.plan.book_type,
                )
            })
            .collect()
    }

    fn process_batch(&mut self, deltas: &OrderBookDeltas) -> Result<()> {
        let (batch_ordinal, expected, book_type) = {
            let state = self.shared.borrow();
            ensure!(
                state.first_error.is_none(),
                "seeded L2 bridge already failed"
            );
            let runtime = state.entries.get(&deltas.instrument_id).with_context(|| {
                format!(
                    "unplanned order-book delta instrument {}",
                    deltas.instrument_id
                )
            })?;
            (
                runtime.observed_batches,
                runtime
                    .plan
                    .batches
                    .get(runtime.observed_batches)
                    .context("received excess seeded L2 source-event batch")?
                    .clone(),
                runtime.plan.book_type,
            )
        };
        ensure!(
            deltas.flags & RecordFlag::F_LAST as u8 != 0,
            "seeded L2 bridge received an unclosed source-event batch"
        );
        ensure!(
            deltas.deltas.len() == expected.row_count,
            "seeded L2 source-event batch row count mismatch for {} at batch {}: expected {}, got {}; delivered ts_event={} ts_init={}",
            deltas.instrument_id,
            batch_ordinal,
            expected.row_count,
            deltas.deltas.len(),
            deltas.ts_event,
            deltas.ts_init,
        );
        let observed_semantic_hash = hash_batch(deltas)?;
        ensure!(
            observed_semantic_hash == expected.semantic_hash,
            "seeded L2 source-event batch semantic hash mismatch"
        );

        let cache = self.cache_rc();
        let (observed_book_type, observed_update_count, actual_quote) = {
            let cache = cache.borrow();
            let book = cache
                .order_book(&deltas.instrument_id)
                .context("managed NT order book is missing after delta delivery")?;
            (
                book.book_type,
                book.update_count,
                quote_from_book(book, deltas),
            )
        };
        ensure!(
            observed_book_type == book_type,
            "managed NT order book has wrong type {:?}; expected {:?}",
            observed_book_type,
            book_type
        );
        ensure!(
            observed_update_count == expected.expected_update_count,
            "managed NT order book update_count mismatch: expected {}, got {}",
            expected.expected_update_count,
            observed_update_count
        );
        let observed_disposition = match expected.disposition {
            BatchDisposition::SyntheticSeed => BatchDisposition::SyntheticSeed,
            BatchDisposition::OneSided => {
                ensure!(
                    actual_quote.is_none(),
                    "managed NT order book unexpectedly became two-sided"
                );
                BatchDisposition::OneSided
            }
            BatchDisposition::EmitQuote(expected_quote) => {
                let quote =
                    actual_quote.context("managed NT order book is unexpectedly one-sided")?;
                ensure!(
                    quote == expected_quote,
                    "managed NT order book BBO does not match compiled audit evidence"
                );
                cache
                    .borrow_mut()
                    .add_quote(quote)
                    .context("cache seeded L2 causal quote")?;
                ensure!(
                    cache.borrow().quote(&quote.instrument_id) == Some(&quote),
                    "seeded L2 causal quote cache insertion did not round trip"
                );
                msgbus::publish_quote(switchboard::get_quotes_topic(quote.instrument_id), &quote);
                BatchDisposition::EmitQuote(quote)
            }
        };
        let observed = BatchPlan {
            semantic_hash: observed_semantic_hash,
            row_count: deltas.deltas.len(),
            expected_update_count: observed_update_count,
            disposition: observed_disposition,
        };
        ensure!(
            observed == expected,
            "seeded L2 observed causal event differs from its compiled plan"
        );

        let mut state = self.shared.borrow_mut();
        let runtime = state
            .entries
            .get_mut(&deltas.instrument_id)
            .context("seeded L2 runtime entry disappeared")?;
        update_trace(&mut runtime.trace_hasher, &observed)?;
        runtime.observed_batches += 1;
        runtime.observed_delta_rows = runtime
            .observed_delta_rows
            .checked_add(observed.row_count as u64)
            .context("seeded L2 observed row count overflow")?;
        runtime.emitted_quotes += u64::from(matches!(
            observed.disposition,
            BatchDisposition::EmitQuote(_)
        ));
        Ok(())
    }

    fn latch_failure(&self, message: String) {
        let should_shutdown = {
            let mut state = self.shared.borrow_mut();
            if state.first_error.is_some() {
                false
            } else {
                state.first_error = Some(message.clone());
                true
            }
        };
        if should_shutdown {
            self.shutdown_system(Some(message));
        }
    }
}

fn compile_batches(
    input: &SeededL2QuoteBridgePlanInput<'_>,
    instrument_id: InstrumentId,
    durable_plan: &SeededL2QuotePlanV1,
) -> Result<Vec<BatchPlan>> {
    let replay_start_time = i64::try_from(
        input
            .deltas
            .first()
            .context("seeded L2 delta stream is empty")?
            .ts_init
            .as_u64(),
    )
    .context("seeded L2 first replay timestamp exceeds i64")?;
    let replay_end_time = i64::try_from(
        input
            .deltas
            .last()
            .context("seeded L2 delta stream is empty")?
            .ts_init
            .as_u64(),
    )
    .context("seeded L2 terminal replay timestamp exceeds i64")?;
    ensure!(
        replay_start_time == durable_plan.replay_start_time,
        "seeded L2 replay_start_time does not match the authoritative delta stream"
    );
    ensure!(
        replay_end_time == durable_plan.replay_end_time,
        "seeded L2 replay_end_time does not match the authoritative delta stream"
    );
    let mut book = OrderBook::new(instrument_id, input.book_type);
    let mut batches = Vec::new();
    let mut rows = Vec::new();
    let mut expected_update_count = 0u64;
    let mut audit_quotes = input.audit_quotes.iter();

    for delta in input.deltas {
        ensure!(
            delta.instrument_id == instrument_id,
            "seeded L2 quote bridge delta instrument mismatch"
        );
        rows.push(*delta);
        if delta.flags & RecordFlag::F_LAST as u8 == 0 {
            continue;
        }

        let batch = OrderBookDeltas::new_checked(instrument_id, std::mem::take(&mut rows))
            .context("construct seeded L2 source-event batch")?;
        book.apply_deltas(&batch)
            .context("apply seeded L2 source-event batch")?;
        expected_update_count = expected_update_count
            .checked_add(batch.deltas.len() as u64)
            .context("seeded L2 update count overflow")?;
        let batch_ordinal = batches.len();
        let disposition = match (
            batch_ordinal < usize::from(durable_plan.synthetic_seed_batches),
            quote_from_book(&book, &batch),
        ) {
            (true, _) => BatchDisposition::SyntheticSeed,
            (false, None) => BatchDisposition::OneSided,
            (false, Some(derived)) => {
                let expected = audit_quotes
                    .next()
                    .context("seeded L2 audit quote missing for two-sided source event")?;
                ensure!(
                    expected.ts_init == expected.ts_event,
                    "seeded L2 audit quote ts_init does not retain source availability time"
                );
                // Audit evidence retains the source availability timestamp;
                // the runtime quote carries the delta catalog's explicit
                // transport replay clock. Every other field must be identical.
                let mut expected_at_replay_time = *expected;
                expected_at_replay_time.ts_init = derived.ts_init;
                ensure!(
                    expected_at_replay_time == derived,
                    "seeded L2 audit quote does not match NT-derived source-event BBO"
                );
                BatchDisposition::EmitQuote(derived)
            }
        };
        batches.push(BatchPlan {
            semantic_hash: hash_batch(&batch)?,
            row_count: batch.deltas.len(),
            expected_update_count,
            disposition,
        });
    }
    ensure!(
        rows.is_empty(),
        "seeded L2 delta stream ended without F_LAST"
    );
    ensure!(
        audit_quotes.next().is_none(),
        "seeded L2 audit quote has no corresponding two-sided source event"
    );
    let source_events = batches
        .len()
        .checked_sub(usize::from(durable_plan.synthetic_seed_batches))
        .context("seeded L2 synthetic seed exceeds event batches")?;
    ensure!(
        source_events as u64 == durable_plan.selected_source_events,
        "seeded L2 source-event count does not match durable conversion plan"
    );
    Ok(batches)
}

fn quote_from_book(book: &OrderBook, batch: &OrderBookDeltas) -> Option<QuoteTick> {
    Some(QuoteTick::new(
        batch.instrument_id,
        book.best_bid_price()?,
        book.best_ask_price()?,
        book.best_bid_size()?,
        book.best_ask_size()?,
        batch.ts_event,
        batch.ts_init,
    ))
}

fn hash_batch(batch: &OrderBookDeltas) -> Result<String> {
    canonical_json_sha256(batch).context("hash seeded L2 source-event batch")
}

fn expected_trace_hash(batches: &[BatchPlan]) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(TRACE_HASH_DOMAIN);
    for batch in batches {
        update_trace(&mut hasher, batch)?;
    }
    Ok(hex::encode(hasher.finalize()))
}

fn update_trace(hasher: &mut Sha256, batch: &BatchPlan) -> Result<()> {
    hasher.update(batch.semantic_hash.as_bytes());
    hasher.update((batch.row_count as u64).to_le_bytes());
    hasher.update(batch.expected_update_count.to_le_bytes());
    match batch.disposition {
        BatchDisposition::SyntheticSeed => hasher.update([0]),
        BatchDisposition::OneSided => hasher.update([1]),
        BatchDisposition::EmitQuote(quote) => {
            hasher.update([2]);
            let quote_hash =
                canonical_json_sha256(&quote).context("hash seeded L2 causal quote")?;
            hasher.update(quote_hash.as_bytes());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, str::FromStr};

    use nautilus_core::UnixNanos;
    use nautilus_model::{
        data::{BookOrder, NULL_ORDER, OrderBookDelta, QuoteTick},
        enums::{BookAction, BookType, OrderSide, RecordFlag},
        identifiers::{ClientId, InstrumentId},
        types::{Price, Quantity},
    };

    use super::{
        BatchDisposition, SeededL2QuoteBridgePlanInput, compile_seeded_l2_quote_bridge_plan,
    };
    use crate::{
        catalog_projection::{NT_DATA_TYPE_ORDER_BOOK_DELTA, NT_DATA_TYPE_QUOTE_TICK},
        conversion_boundary::{ConversionFingerprint, ConversionManifest, SeededL2QuotePlanV1},
        seeded_level_set_deltas::{
            SEEDED_LEVEL_SET_DELTAS_TRANSFORM_IDENTITY, SEEDED_LEVEL_SET_DELTAS_TRANSFORM_VERSION,
        },
    };

    fn flags(values: &[RecordFlag]) -> u8 {
        values
            .iter()
            .fold(0, |combined, value| combined | *value as u8)
    }

    fn delta(
        instrument_id: InstrumentId,
        action: BookAction,
        side: OrderSide,
        price: &str,
        size: &str,
        flags: u8,
        timestamp: u64,
    ) -> OrderBookDelta {
        let order = if action == BookAction::Clear {
            NULL_ORDER
        } else {
            BookOrder::new(
                side,
                Price::from_str(price).unwrap(),
                Quantity::from_str(size).unwrap(),
                0,
            )
        };
        OrderBookDelta::new_checked(
            instrument_id,
            action,
            order,
            flags,
            0,
            UnixNanos::from(timestamp),
            UnixNanos::from(timestamp),
        )
        .unwrap()
    }

    fn quote(instrument_id: InstrumentId, timestamp: u64) -> QuoteTick {
        QuoteTick::new(
            instrument_id,
            Price::from("100"),
            Price::from("102"),
            Quantity::from("10"),
            Quantity::from("12"),
            UnixNanos::from(timestamp),
            UnixNanos::from(timestamp),
        )
    }

    fn conversion_manifest(
        instrument_id: InstrumentId,
        durable_plan: SeededL2QuotePlanV1,
        delta_rows: usize,
        audit_quote_rows: usize,
        source_proof_id: &str,
    ) -> ConversionManifest {
        ConversionManifest::completed(
            ConversionFingerprint {
                source_proof_id: source_proof_id.to_string(),
                source_proof_version: 1,
                accepted_object_sha256: "a".repeat(64),
                converter_identity: SEEDED_LEVEL_SET_DELTAS_TRANSFORM_IDENTITY.to_string(),
                converter_version: SEEDED_LEVEL_SET_DELTAS_TRANSFORM_VERSION.to_string(),
                converter_config_hash: "b".repeat(64),
            },
            "canonical-order-book-deltas.v1",
            NT_DATA_TYPE_ORDER_BOOK_DELTA,
            instrument_id.to_string(),
            delta_rows,
            "memory://catalog",
            "c".repeat(64),
            "d".repeat(64),
            "2026-08-21T00:00:00Z",
        )
        .with_catalog_rows_by_nt_data_type(BTreeMap::from([
            (NT_DATA_TYPE_ORDER_BOOK_DELTA.to_string(), delta_rows),
            (NT_DATA_TYPE_QUOTE_TICK.to_string(), audit_quote_rows),
        ]))
        .with_seeded_l2_quote_plan(durable_plan)
        .expect("bind seeded replay plan")
    }

    #[test]
    fn compiler_derives_identity_and_plan_from_conversion_manifest() {
        let instrument_id = InstrumentId::from("BTC-USDT.OKX");
        let durable_plan = SeededL2QuotePlanV1 {
            synthetic_seed_batches: 0,
            selected_source_events: 1,
            replay_start_time: 10,
            replay_end_time: 10,
        };
        let manifest = conversion_manifest(instrument_id, durable_plan, 1, 0, "source-proof");
        let deltas = [delta(
            instrument_id,
            BookAction::Clear,
            OrderSide::NoOrderSide,
            "0",
            "0",
            flags(&[
                RecordFlag::F_SNAPSHOT,
                RecordFlag::F_MBP,
                RecordFlag::F_LAST,
            ]),
            10,
        )];

        let compiled = compile_seeded_l2_quote_bridge_plan(vec![SeededL2QuoteBridgePlanInput {
            conversion_manifest: &manifest,
            client_id: None,
            book_type: BookType::L2_MBP,
            deltas: &deltas,
            audit_quotes: &[],
        }])
        .expect("compile manifest-bound plan");

        let entry = compiled.entries.get(&instrument_id).unwrap();
        assert_eq!(
            entry.conversion_manifest_hash,
            manifest.content_hash().unwrap()
        );
        assert_eq!(entry.durable_plan, manifest.seeded_l2_quote_plan.unwrap());
    }

    #[test]
    fn compiler_preserves_every_source_event_even_when_bbo_is_unchanged() {
        let instrument_id = InstrumentId::from("BTC-USDT.OKX");
        let seed_time = 10;
        let first_time = 20;
        let second_time = 30;
        let deltas = vec![
            delta(
                instrument_id,
                BookAction::Clear,
                OrderSide::NoOrderSide,
                "0",
                "0",
                flags(&[RecordFlag::F_SNAPSHOT, RecordFlag::F_MBP]),
                seed_time,
            ),
            delta(
                instrument_id,
                BookAction::Add,
                OrderSide::Buy,
                "100",
                "10",
                flags(&[RecordFlag::F_SNAPSHOT, RecordFlag::F_MBP]),
                seed_time,
            ),
            delta(
                instrument_id,
                BookAction::Add,
                OrderSide::Sell,
                "102",
                "12",
                flags(&[
                    RecordFlag::F_SNAPSHOT,
                    RecordFlag::F_MBP,
                    RecordFlag::F_LAST,
                ]),
                seed_time,
            ),
            delta(
                instrument_id,
                BookAction::Update,
                OrderSide::Buy,
                "99",
                "20",
                flags(&[RecordFlag::F_MBP, RecordFlag::F_LAST]),
                first_time,
            ),
            delta(
                instrument_id,
                BookAction::Update,
                OrderSide::Sell,
                "103",
                "21",
                flags(&[RecordFlag::F_MBP, RecordFlag::F_LAST]),
                second_time,
            ),
        ];
        let manifest = conversion_manifest(
            instrument_id,
            SeededL2QuotePlanV1 {
                synthetic_seed_batches: 1,
                selected_source_events: 2,
                replay_start_time: seed_time as i64,
                replay_end_time: second_time as i64,
            },
            deltas.len(),
            2,
            "source-proof",
        );
        let plan = compile_seeded_l2_quote_bridge_plan(vec![SeededL2QuoteBridgePlanInput {
            conversion_manifest: &manifest,
            client_id: Some(ClientId::from("OKX")),
            book_type: BookType::L2_MBP,
            deltas: &deltas,
            audit_quotes: &[
                quote(instrument_id, first_time),
                quote(instrument_id, second_time),
            ],
        }])
        .expect("compile causal quote plan");

        let entry = plan.entries.get(&instrument_id).unwrap();
        assert_eq!(entry.batches.len(), 3);
        assert!(matches!(
            entry.batches[0].disposition,
            BatchDisposition::SyntheticSeed
        ));
        assert!(matches!(
            entry.batches[1].disposition,
            BatchDisposition::EmitQuote(value) if value == quote(instrument_id, first_time)
        ));
        assert!(matches!(
            entry.batches[2].disposition,
            BatchDisposition::EmitQuote(value) if value == quote(instrument_id, second_time)
        ));
    }

    #[test]
    fn compiler_rejects_audit_quote_with_tampered_source_availability_time() {
        let instrument_id = InstrumentId::from("BTC-USDT.OKX");
        let timestamp = 10;
        let deltas = vec![
            delta(
                instrument_id,
                BookAction::Clear,
                OrderSide::NoOrderSide,
                "0",
                "0",
                flags(&[RecordFlag::F_SNAPSHOT, RecordFlag::F_MBP]),
                timestamp,
            ),
            delta(
                instrument_id,
                BookAction::Add,
                OrderSide::Buy,
                "100",
                "10",
                flags(&[RecordFlag::F_SNAPSHOT, RecordFlag::F_MBP]),
                timestamp,
            ),
            delta(
                instrument_id,
                BookAction::Add,
                OrderSide::Sell,
                "102",
                "12",
                flags(&[
                    RecordFlag::F_SNAPSHOT,
                    RecordFlag::F_MBP,
                    RecordFlag::F_LAST,
                ]),
                timestamp,
            ),
        ];
        let manifest = conversion_manifest(
            instrument_id,
            SeededL2QuotePlanV1 {
                synthetic_seed_batches: 0,
                selected_source_events: 1,
                replay_start_time: timestamp as i64,
                replay_end_time: timestamp as i64,
            },
            deltas.len(),
            1,
            "source-proof",
        );
        let mut tampered = quote(instrument_id, timestamp);
        tampered.ts_init = UnixNanos::from(timestamp + 1);

        let error = compile_seeded_l2_quote_bridge_plan(vec![SeededL2QuoteBridgePlanInput {
            conversion_manifest: &manifest,
            client_id: None,
            book_type: BookType::L2_MBP,
            deltas: &deltas,
            audit_quotes: &[tampered],
        }])
        .err()
        .expect("timestamp-tampered audit evidence must fail closed");

        assert!(error.to_string().contains("source availability time"));
    }

    #[test]
    fn compiler_rejects_non_l2_manifest_book_type() {
        let instrument_id = InstrumentId::from("BTC-USDT.OKX");
        let manifest = conversion_manifest(
            instrument_id,
            SeededL2QuotePlanV1 {
                synthetic_seed_batches: 0,
                selected_source_events: 1,
                replay_start_time: 1,
                replay_end_time: 1,
            },
            0,
            0,
            "source-proof",
        );
        let error = compile_seeded_l2_quote_bridge_plan(vec![SeededL2QuoteBridgePlanInput {
            conversion_manifest: &manifest,
            client_id: None,
            book_type: BookType::L1_MBP,
            deltas: &[],
            audit_quotes: &[],
        }])
        .err()
        .expect("manifest-resolved non-L2 book must fail closed");

        assert!(error.to_string().contains("manifest-resolved L2_MBP"));
    }

    #[test]
    fn compiler_rejects_replay_bounds_not_bound_to_delta_stream() {
        let instrument_id = InstrumentId::from("BTC-USDT.OKX");
        let deltas = [delta(
            instrument_id,
            BookAction::Clear,
            OrderSide::NoOrderSide,
            "0",
            "0",
            flags(&[
                RecordFlag::F_SNAPSHOT,
                RecordFlag::F_MBP,
                RecordFlag::F_LAST,
            ]),
            10,
        )];
        let manifest = conversion_manifest(
            instrument_id,
            SeededL2QuotePlanV1 {
                synthetic_seed_batches: 0,
                selected_source_events: 1,
                replay_start_time: 10,
                replay_end_time: 11,
            },
            deltas.len(),
            0,
            "source-proof",
        );
        let error = compile_seeded_l2_quote_bridge_plan(vec![SeededL2QuoteBridgePlanInput {
            conversion_manifest: &manifest,
            client_id: None,
            book_type: BookType::L2_MBP,
            deltas: &deltas,
            audit_quotes: &[],
        }])
        .err()
        .expect("tampered durable replay bounds must fail closed");

        assert!(error.to_string().contains("replay_end_time"));
    }

    #[test]
    fn compiler_rejects_superseded_seeded_converter_manifest() {
        let instrument_id = InstrumentId::from("BTC-USDT.OKX");
        let deltas = [delta(
            instrument_id,
            BookAction::Clear,
            OrderSide::NoOrderSide,
            "0",
            "0",
            flags(&[
                RecordFlag::F_SNAPSHOT,
                RecordFlag::F_MBP,
                RecordFlag::F_LAST,
            ]),
            10,
        )];
        let mut manifest = conversion_manifest(
            instrument_id,
            SeededL2QuotePlanV1 {
                synthetic_seed_batches: 0,
                selected_source_events: 1,
                replay_start_time: 10,
                replay_end_time: 10,
            },
            deltas.len(),
            0,
            "source-proof",
        );
        manifest.fingerprint.converter_version = "1".to_string();
        let error = compile_seeded_l2_quote_bridge_plan(vec![SeededL2QuoteBridgePlanInput {
            conversion_manifest: &manifest,
            client_id: None,
            book_type: BookType::L2_MBP,
            deltas: &deltas,
            audit_quotes: &[],
        }])
        .err()
        .expect("superseded seeded converter manifest must fail closed");

        assert!(
            error.to_string().contains("registered seeded converter"),
            "{error}"
        );
    }

    #[test]
    fn compiler_plan_hash_is_independent_of_input_order() {
        let first = InstrumentId::from("BTC-USDT.OKX");
        let second = InstrumentId::from("BTCUSDT-SPOT.BYBIT");
        let first_deltas = [delta(
            first,
            BookAction::Clear,
            OrderSide::NoOrderSide,
            "0",
            "0",
            flags(&[
                RecordFlag::F_SNAPSHOT,
                RecordFlag::F_MBP,
                RecordFlag::F_LAST,
            ]),
            10,
        )];
        let second_deltas = [delta(
            second,
            BookAction::Clear,
            OrderSide::NoOrderSide,
            "0",
            "0",
            flags(&[
                RecordFlag::F_SNAPSHOT,
                RecordFlag::F_MBP,
                RecordFlag::F_LAST,
            ]),
            10,
        )];
        let first_manifest = conversion_manifest(
            first,
            SeededL2QuotePlanV1 {
                synthetic_seed_batches: 0,
                selected_source_events: 1,
                replay_start_time: 10,
                replay_end_time: 10,
            },
            first_deltas.len(),
            0,
            "first-source-proof",
        );
        let second_manifest = conversion_manifest(
            second,
            SeededL2QuotePlanV1 {
                synthetic_seed_batches: 0,
                selected_source_events: 1,
                replay_start_time: 10,
                replay_end_time: 10,
            },
            second_deltas.len(),
            0,
            "second-source-proof",
        );
        let compile = |reverse: bool| {
            let first_input = SeededL2QuoteBridgePlanInput {
                conversion_manifest: &first_manifest,
                client_id: None,
                book_type: BookType::L2_MBP,
                deltas: &first_deltas,
                audit_quotes: &[],
            };
            let second_input = SeededL2QuoteBridgePlanInput {
                conversion_manifest: &second_manifest,
                client_id: None,
                book_type: BookType::L2_MBP,
                deltas: &second_deltas,
                audit_quotes: &[],
            };
            let inputs = if reverse {
                vec![second_input, first_input]
            } else {
                vec![first_input, second_input]
            };
            compile_seeded_l2_quote_bridge_plan(inputs).expect("compile equivalent plan")
        };

        assert_eq!(compile(false).plan_hash, compile(true).plan_hash);
    }
}
