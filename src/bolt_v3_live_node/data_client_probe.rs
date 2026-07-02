use super::*;

/// Run an already-built strategy-free data-client probe node.
///
/// The caller must build `runtime` at a synchronous startup boundary before
/// entering Tokio, because the build path owns SSM resolution through
/// `SsmResolverSession`.
pub async fn run_bolt_v3_data_client_probe(
    mut runtime: BoltV3LiveNodeRuntime,
    probe_loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<BoltV3DataClientProbeReport, BoltV3LiveNodeError> {
    let handle = strategy_free_data_client_readiness_quote_probe_handle(probe_loaded, client_key)?;
    let readiness_probe = probe_loaded
        .root
        .clients
        .get(client_key)
        .and_then(|client| client.readiness_probe.as_ref())
        .ok_or_else(|| {
            BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                "data-client readiness probe requires clients.<id>.readiness_probe"
            ))
        })?;
    let market_data_kind = readiness_probe.market_data_kind;
    let book_type = readiness_probe
        .book_type
        .map(readiness_probe_book_type_to_nt);
    let quote_target_source = readiness_probe.quote_target_source;
    let client_venue = probe_loaded
        .root
        .clients
        .get(client_key)
        .map(|client| client.venue)
        .ok_or_else(|| {
            BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                "data-client readiness probe client_key is not configured"
            ))
        })?;

    let mut subscribed = Vec::new();
    let mut observer = None;
    let mut metadata_observer = None;
    let mut metadata_driver = None;

    match quote_target_source {
        DataClientReadinessProbeQuoteTargetSource::Configured => {
            let subscriptions =
                strategy_free_configured_data_client_probe_subscriptions(probe_loaded, client_key)?;
            for subscription in &subscriptions {
                if let Err(error) = subscribe_strategy_free_probe_subscription(
                    &mut runtime,
                    subscription,
                    market_data_kind,
                    book_type,
                ) {
                    for previous in subscribed.iter().rev() {
                        unsubscribe_strategy_free_probe_subscription(
                            &mut runtime,
                            previous,
                            market_data_kind,
                        );
                    }
                    return Err(error);
                }
                subscribed.push(subscription.clone());
            }
            observer = Some(StrategyFreeDataClientProbeObserver::register(
                &handle,
                &subscriptions,
                runtime.handle(),
            ));
        }
        DataClientReadinessProbeQuoteTargetSource::MetadataResponse => {
            if handle.is_chunk_count_mode() {
                return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                    anyhow::anyhow!(
                        "ops data-client-probe does not support trade chunk-count metadata_response probes"
                    ),
                ));
            }
            runtime.ensure_strategy_free_data_client_registered(
                ClientId::from(client_key),
                readiness_probe_market_data_kind_label(market_data_kind),
            )?;
            let metadata = StrategyFreeMetadataResponseProbeObserver::register(
                &handle,
                client_venue,
                market_data_kind,
                book_type,
                runtime.handle(),
                Duration::from_millis(
                    probe_loaded
                        .root
                        .persistence
                        .data_client_readiness_probe_poll_interval_ms,
                ),
            )?;
            metadata_driver = Some(metadata.driver());
            metadata_observer = Some(metadata);
        }
    }

    let stop_handle = runtime.handle();
    let run_timeout = Duration::from_secs(strategy_free_start_timeout_secs(probe_loaded)?);
    let stop_timeout = Duration::from_secs(strategy_free_stop_timeout_secs(probe_loaded)?);
    let (run_result, driver_error) = if let Some(driver) = metadata_driver {
        let run_future = runtime.run_strategy_free_until_stop_or_timeout(run_timeout, stop_timeout);
        tokio::pin!(run_future);
        let driver_future = driver.drive_until_subscribed();
        tokio::pin!(driver_future);
        let mut driver_result = None;
        let run_result = loop {
            tokio::select! {
                result = &mut run_future => break result,
                result = &mut driver_future, if driver_result.is_none() => {
                    if result.is_err() {
                        stop_handle.stop();
                    }
                    driver_result = Some(result);
                }
            }
        };
        (run_result, driver_result.and_then(Result::err))
    } else {
        (
            runtime
                .run_strategy_free_until_stop_or_timeout(run_timeout, stop_timeout)
                .await,
            None,
        )
    };

    for subscription in subscribed.iter().rev() {
        unsubscribe_strategy_free_probe_subscription(&mut runtime, subscription, market_data_kind);
    }
    if let Some(metadata) = metadata_observer {
        for subscription in metadata.subscriptions().iter().rev() {
            unsubscribe_strategy_free_probe_subscription(
                &mut runtime,
                subscription,
                market_data_kind,
            );
        }
        metadata.unregister();
    }
    if let Some(observer) = observer {
        observer.unregister();
    }

    if let Some(error) = driver_error {
        return Err(error);
    }
    let run_timed_out = run_result?;
    if handle.has_all_required_market_data() {
        return Ok(BoltV3DataClientProbeReport {
            client_key: client_key.to_string(),
            market_data_kind: readiness_probe_market_data_kind_label(market_data_kind).to_string(),
            required_observation_count: handle.required_market_data_count(),
            observed_update_count: handle.observed_market_data_count(),
        });
    }

    let reason = handle.failure_error().unwrap_or_else(|| {
        let observed = handle.observed_market_data_count();
        let required = handle.required_market_data_count();
        if run_timed_out {
            format!(
                "timed out before observing required data-client market data ({observed}/{required} observed)"
            )
        } else {
            format!(
                "live node exited before observing required data-client market data ({observed}/{required} observed)"
            )
        }
    });
    Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed { reason })
}

pub async fn run_bolt_v3_data_client_census(
    mut runtime: BoltV3LiveNodeRuntime,
    census_loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<BoltV3DataClientCensusReport, BoltV3LiveNodeError> {
    let client = census_loaded.root.clients.get(client_key).ok_or_else(|| {
        BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client census client_key is not configured".to_string(),
        }
    })?;
    if client.data.is_none() {
        return Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client census requires the selected client to declare [data]".to_string(),
        });
    }
    runtime.ensure_strategy_free_data_client_registered(
        ClientId::from(client_key),
        "instrument census",
    )?;

    let start_timeout = Duration::from_secs(strategy_free_start_timeout_secs(census_loaded)?);
    let stop_timeout = Duration::from_secs(strategy_free_stop_timeout_secs(census_loaded)?);
    let poll_interval = Duration::from_millis(
        census_loaded
            .root
            .persistence
            .data_client_readiness_probe_poll_interval_ms,
    );
    runtime
        .run_strategy_free_until_running_then_stop(start_timeout, stop_timeout, poll_interval)
        .await?;
    data_client_census_report(client_key, runtime.cached_instrument_ids())
}

pub(super) fn data_client_census_report(
    client_key: &str,
    mut instrument_ids: Vec<String>,
) -> Result<BoltV3DataClientCensusReport, BoltV3LiveNodeError> {
    instrument_ids.sort();
    instrument_ids.dedup();
    if instrument_ids.is_empty() {
        return Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client census observed zero cached instruments".to_string(),
        });
    }
    Ok(BoltV3DataClientCensusReport {
        client_key: client_key.to_string(),
        cached_instrument_count: instrument_ids.len(),
        cached_instrument_ids_sha256: instrument_ids_sha256(&instrument_ids),
    })
}

fn instrument_ids_sha256(instrument_ids: &[String]) -> String {
    let mut hasher = Sha256::new();
    for instrument_id in instrument_ids {
        hasher.update(instrument_id.as_bytes());
        hasher.update(b"\0");
    }
    hex::encode(hasher.finalize())
}

enum StrategyFreeDataClientProbeHandler {
    Quote(MStr<Pattern>, TypedHandler<QuoteTick>),
    Book(MStr<Pattern>, TypedHandler<OrderBookDeltas>),
    Trade(MStr<Pattern>, TypedHandler<TradeTick>),
}

struct StrategyFreeDataClientProbeObserver {
    handlers: Vec<StrategyFreeDataClientProbeHandler>,
}

impl StrategyFreeDataClientProbeObserver {
    fn register(
        handle: &BoltV3StrategyFreeReferenceQuoteProbeHandle,
        subscriptions: &[StrategyFreeReferenceQuoteSubscription],
        stop_handle: LiveNodeHandle,
    ) -> Self {
        let mut handlers = Vec::new();
        for subscription in subscriptions {
            match handle.market_data_kind {
                DataClientReadinessProbeMarketDataKind::Quote => {
                    let probe_handle = handle.clone();
                    let stop_handle = stop_handle.clone();
                    let pattern: MStr<Pattern> =
                        switchboard::get_quotes_topic(subscription.instrument_id).into();
                    let handler = TypedHandler::from(move |quote: &QuoteTick| {
                        probe_handle.record_quote(
                            quote,
                            get_atomic_clock_realtime().get_time_ns().as_u64(),
                        );
                        if probe_handle.has_all_required_market_data() {
                            stop_handle.stop();
                        }
                    });
                    msgbus::subscribe_quotes(pattern, handler.clone(), None);
                    handlers.push(StrategyFreeDataClientProbeHandler::Quote(pattern, handler));
                }
                DataClientReadinessProbeMarketDataKind::Book => {
                    let probe_handle = handle.clone();
                    let stop_handle = stop_handle.clone();
                    let pattern: MStr<Pattern> =
                        switchboard::get_book_deltas_topic(subscription.instrument_id).into();
                    let handler = TypedHandler::from(move |deltas: &OrderBookDeltas| {
                        probe_handle.record_book_deltas(
                            deltas,
                            get_atomic_clock_realtime().get_time_ns().as_u64(),
                        );
                        if probe_handle.has_all_required_market_data() {
                            stop_handle.stop();
                        }
                    });
                    msgbus::subscribe_book_deltas(pattern, handler.clone(), None);
                    handlers.push(StrategyFreeDataClientProbeHandler::Book(pattern, handler));
                }
                DataClientReadinessProbeMarketDataKind::Trade => {
                    let probe_handle = handle.clone();
                    let stop_handle = stop_handle.clone();
                    let pattern: MStr<Pattern> =
                        switchboard::get_trades_topic(subscription.instrument_id).into();
                    let handler = TypedHandler::from(move |trade: &TradeTick| {
                        probe_handle.record_trade(trade);
                        if probe_handle.has_all_required_market_data() {
                            stop_handle.stop();
                        }
                    });
                    msgbus::subscribe_trades(pattern, handler.clone(), None);
                    handlers.push(StrategyFreeDataClientProbeHandler::Trade(pattern, handler));
                }
            }
        }
        Self { handlers }
    }

    fn unregister(self) {
        for handler in self.handlers {
            match handler {
                StrategyFreeDataClientProbeHandler::Quote(pattern, handler) => {
                    msgbus::unsubscribe_quotes(pattern, &handler);
                }
                StrategyFreeDataClientProbeHandler::Book(pattern, handler) => {
                    msgbus::unsubscribe_book_deltas(pattern, &handler);
                }
                StrategyFreeDataClientProbeHandler::Trade(pattern, handler) => {
                    msgbus::unsubscribe_trades(pattern, &handler);
                }
            }
        }
    }
}

#[derive(Clone)]
struct StrategyFreeMetadataResponseProbeDriver {
    state: Rc<StrategyFreeMetadataResponseProbeState>,
}

impl StrategyFreeMetadataResponseProbeDriver {
    async fn drive_until_subscribed(&self) -> Result<(), BoltV3LiveNodeError> {
        drive_metadata_response_probe_until_subscribed(self.state.as_ref()).await
    }
}

trait StrategyFreeMetadataResponseProbeDriverState {
    fn has_subscriptions(&self) -> bool;
    fn is_runtime_running(&self) -> bool;
    fn install_and_subscribe(&self) -> Result<(), BoltV3LiveNodeError>;
    fn notify(&self) -> &tokio::sync::Notify;
    fn runtime_state_poll_interval(&self) -> Duration;
}

async fn drive_metadata_response_probe_until_subscribed(
    state: &impl StrategyFreeMetadataResponseProbeDriverState,
) -> Result<(), BoltV3LiveNodeError> {
    loop {
        if state.has_subscriptions() {
            return Ok(());
        }
        if state.is_runtime_running() {
            return state.install_and_subscribe();
        }

        tokio::select! {
            () = state.notify().notified() => continue,
            () = tokio::time::sleep(state.runtime_state_poll_interval()) => continue,
        }
    }
}

struct StrategyFreeMetadataResponseProbeObserver {
    pattern: MStr<Pattern>,
    handler: TypedHandler<InstrumentAny>,
    state: Rc<StrategyFreeMetadataResponseProbeState>,
}

#[derive(Debug, PartialEq, Eq)]
enum MetadataResponseInstrumentUpdate {
    Existing,
    NewBeforeSubscription,
    NewAfterSubscription,
}

fn record_source_owned_metadata_response_instrument(
    instruments: &mut BTreeMap<InstrumentId, InstrumentId>,
    subscriptions_installed: bool,
    instrument_id: InstrumentId,
) -> MetadataResponseInstrumentUpdate {
    if instruments.contains_key(&instrument_id) {
        return MetadataResponseInstrumentUpdate::Existing;
    }
    instruments.insert(instrument_id, instrument_id);
    if subscriptions_installed {
        MetadataResponseInstrumentUpdate::NewAfterSubscription
    } else {
        MetadataResponseInstrumentUpdate::NewBeforeSubscription
    }
}

impl StrategyFreeMetadataResponseProbeObserver {
    fn register(
        handle: &BoltV3StrategyFreeReferenceQuoteProbeHandle,
        venue: Venue,
        market_data_kind: DataClientReadinessProbeMarketDataKind,
        book_type: Option<BookType>,
        stop_handle: LiveNodeHandle,
        runtime_state_poll_interval: Duration,
    ) -> Result<Self, BoltV3LiveNodeError> {
        if handle.metadata_response_max_quote_targets.is_none() {
            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                anyhow::anyhow!(
                    "data-client readiness probe requires clients.<id>.readiness_probe.max_metadata_quote_targets when quote_target_source = \"metadata_response\""
                ),
            ));
        }
        let state = Rc::new(StrategyFreeMetadataResponseProbeState {
            handle: handle.clone(),
            venue,
            market_data_kind,
            book_type,
            instruments: RefCell::new(BTreeMap::new()),
            subscriptions: RefCell::new(Vec::new()),
            market_observer: RefCell::new(None),
            notify: tokio::sync::Notify::new(),
            stop_handle,
            runtime_state_poll_interval,
        });
        let handler_state = state.clone();
        let handler = TypedHandler::from(move |instrument: &InstrumentAny| {
            let instrument_id = instrument.id();
            if instrument_id.venue != handler_state.venue {
                return;
            }
            let mut instruments = handler_state.instruments.borrow_mut();
            match record_source_owned_metadata_response_instrument(
                &mut instruments,
                handler_state.has_subscriptions(),
                instrument_id,
            ) {
                MetadataResponseInstrumentUpdate::Existing => {}
                MetadataResponseInstrumentUpdate::NewBeforeSubscription => {
                    handler_state.notify.notify_one();
                }
                MetadataResponseInstrumentUpdate::NewAfterSubscription => {
                    handler_state
                        .handle
                        .fail_late_metadata_response_instrument(instrument_id);
                    handler_state.stop_handle.stop();
                }
            }
        });
        let pattern = crate::bolt_v3_instrument_metadata_bus::metadata_instrument_pattern(venue);
        crate::bolt_v3_instrument_metadata_bus::attach_metadata_instrument_handler(
            pattern,
            handler.clone(),
        );
        Ok(Self {
            pattern,
            handler,
            state,
        })
    }

    fn driver(&self) -> StrategyFreeMetadataResponseProbeDriver {
        StrategyFreeMetadataResponseProbeDriver {
            state: self.state.clone(),
        }
    }

    fn subscriptions(&self) -> Vec<StrategyFreeReferenceQuoteSubscription> {
        self.state.subscriptions.borrow().clone()
    }

    fn unregister(self) {
        crate::bolt_v3_instrument_metadata_bus::detach_metadata_instrument_handler(
            self.pattern,
            &self.handler,
        );
        if let Some(observer) = self.state.market_observer.borrow_mut().take() {
            observer.unregister();
        }
    }
}

struct StrategyFreeMetadataResponseProbeState {
    handle: BoltV3StrategyFreeReferenceQuoteProbeHandle,
    venue: Venue,
    market_data_kind: DataClientReadinessProbeMarketDataKind,
    book_type: Option<BookType>,
    instruments: RefCell<BTreeMap<InstrumentId, InstrumentId>>,
    subscriptions: RefCell<Vec<StrategyFreeReferenceQuoteSubscription>>,
    market_observer: RefCell<Option<StrategyFreeDataClientProbeObserver>>,
    notify: tokio::sync::Notify,
    stop_handle: LiveNodeHandle,
    runtime_state_poll_interval: Duration,
}

fn send_metadata_response_probe_subscriptions_with_rollback<TObserver>(
    subscriptions: &[StrategyFreeReferenceQuoteSubscription],
    register_observer: impl FnOnce() -> TObserver,
    mut send_subscription: impl FnMut(
        &StrategyFreeReferenceQuoteSubscription,
    ) -> Result<(), BoltV3LiveNodeError>,
    unregister_observer: impl FnOnce(TObserver),
) -> Result<TObserver, BoltV3LiveNodeError> {
    let observer = register_observer();
    for subscription in subscriptions {
        if let Err(error) = send_subscription(subscription) {
            unregister_observer(observer);
            return Err(error);
        }
    }
    Ok(observer)
}

impl StrategyFreeMetadataResponseProbeState {
    fn has_subscriptions(&self) -> bool {
        !self.subscriptions.borrow().is_empty()
    }

    fn install_and_subscribe(&self) -> Result<(), BoltV3LiveNodeError> {
        if self.has_subscriptions() {
            return Ok(());
        }
        let instrument_ids = self
            .instruments
            .borrow()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let subscriptions = self
            .handle
            .install_metadata_response_instrument_ids(instrument_ids);
        if subscriptions.is_empty() {
            let reason = self
                .handle
                .failure_error()
                .unwrap_or_else(|| METADATA_RESPONSE_EMPTY_TARGETS_FAILURE.to_string());
            return Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed { reason });
        }
        let mut sent_subscriptions = Vec::new();
        let market_observer = send_metadata_response_probe_subscriptions_with_rollback(
            &subscriptions,
            || {
                StrategyFreeDataClientProbeObserver::register(
                    &self.handle,
                    &subscriptions,
                    self.stop_handle.clone(),
                )
            },
            |subscription| {
                send_strategy_free_probe_subscription(
                    subscription,
                    self.market_data_kind,
                    self.book_type,
                )?;
                sent_subscriptions.push(subscription.clone());
                Ok(())
            },
            StrategyFreeDataClientProbeObserver::unregister,
        )
        .inspect_err(|_| {
            *self.subscriptions.borrow_mut() = sent_subscriptions.clone();
        })?;
        *self.subscriptions.borrow_mut() = subscriptions;
        *self.market_observer.borrow_mut() = Some(market_observer);
        Ok(())
    }
}

impl StrategyFreeMetadataResponseProbeDriverState for StrategyFreeMetadataResponseProbeState {
    fn has_subscriptions(&self) -> bool {
        StrategyFreeMetadataResponseProbeState::has_subscriptions(self)
    }

    fn is_runtime_running(&self) -> bool {
        self.stop_handle.is_running()
    }

    fn install_and_subscribe(&self) -> Result<(), BoltV3LiveNodeError> {
        StrategyFreeMetadataResponseProbeState::install_and_subscribe(self)
    }

    fn notify(&self) -> &tokio::sync::Notify {
        &self.notify
    }

    fn runtime_state_poll_interval(&self) -> Duration {
        self.runtime_state_poll_interval
    }
}

#[cfg(test)]
mod metadata_response_probe_driver_tests {
    use std::{
        cell::{Cell, RefCell},
        time::Duration,
    };

    use super::*;

    struct FakeMetadataResponseProbeState {
        has_subscriptions: Cell<bool>,
        runtime_running: Cell<bool>,
        install_calls: Cell<usize>,
        fail_install: bool,
        notify: tokio::sync::Notify,
        runtime_state_poll_interval: Duration,
    }

    impl FakeMetadataResponseProbeState {
        fn pending() -> Self {
            Self {
                has_subscriptions: Cell::new(false),
                runtime_running: Cell::new(false),
                install_calls: Cell::new(0),
                fail_install: false,
                notify: tokio::sync::Notify::new(),
                runtime_state_poll_interval: Duration::from_secs(60),
            }
        }

        fn running() -> Self {
            let state = Self::pending();
            state.runtime_running.set(true);
            state
        }

        fn running_with_empty_metadata_failure() -> Self {
            Self {
                fail_install: true,
                ..Self::running()
            }
        }

        fn mark_runtime_running(&self) {
            self.runtime_running.set(true);
            self.notify.notify_one();
        }
    }

    impl StrategyFreeMetadataResponseProbeDriverState for FakeMetadataResponseProbeState {
        fn has_subscriptions(&self) -> bool {
            self.has_subscriptions.get()
        }

        fn is_runtime_running(&self) -> bool {
            self.runtime_running.get()
        }

        fn install_and_subscribe(&self) -> Result<(), BoltV3LiveNodeError> {
            self.install_calls.set(self.install_calls.get() + 1);
            if self.fail_install {
                return Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
                    reason: METADATA_RESPONSE_EMPTY_TARGETS_FAILURE.to_string(),
                });
            }
            self.has_subscriptions.set(true);
            Ok(())
        }

        fn notify(&self) -> &tokio::sync::Notify {
            &self.notify
        }

        fn runtime_state_poll_interval(&self) -> Duration {
            self.runtime_state_poll_interval
        }
    }

    #[test]
    fn metadata_response_driver_waits_for_runtime_running_before_installing() {
        let state = FakeMetadataResponseProbeState::pending();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime should build");

        runtime.block_on(async {
            let driver = drive_metadata_response_probe_until_subscribed(&state);
            tokio::pin!(driver);
            state.notify.notify_one();

            tokio::select! {
                result = &mut driver => {
                    panic!("driver installed before runtime startup drain completed: {result:?}");
                }
                () = tokio::task::yield_now() => {}
            }

            assert_eq!(
                state.install_calls.get(),
                0,
                "metadata_response driver must wait for LiveNode running state before installing"
            );
            state.mark_runtime_running();
            driver
                .await
                .expect("driver should install after runtime is running");
        });

        assert_eq!(state.install_calls.get(), 1);
        assert!(state.has_subscriptions.get());
    }

    #[test]
    fn metadata_response_driver_installs_after_runtime_running() {
        let state = FakeMetadataResponseProbeState::running();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime should build");

        runtime
            .block_on(drive_metadata_response_probe_until_subscribed(&state))
            .expect("driver should install available metadata targets");

        assert_eq!(state.install_calls.get(), 1);
        assert!(state.has_subscriptions.get());
    }

    #[test]
    fn metadata_response_driver_fails_empty_metadata_after_runtime_running() {
        let state = FakeMetadataResponseProbeState::running_with_empty_metadata_failure();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime should build");

        let failure = runtime
            .block_on(drive_metadata_response_probe_until_subscribed(&state))
            .expect_err("empty metadata universe should fail closed after startup drain");

        assert!(
            failure
                .to_string()
                .contains("no source-owned instrument targets"),
            "failure should explain the empty metadata target set: {failure}"
        );
        assert_eq!(state.install_calls.get(), 1);
        assert!(!state.has_subscriptions.get());
    }

    #[test]
    fn metadata_response_records_late_new_instrument_after_subscription() {
        let early = InstrumentId::from("EARLY.POLYMARKET");
        let mut instruments = BTreeMap::from([(early, early)]);

        let update = record_source_owned_metadata_response_instrument(
            &mut instruments,
            true,
            InstrumentId::from("LATE.POLYMARKET"),
        );

        assert_eq!(
            update,
            MetadataResponseInstrumentUpdate::NewAfterSubscription,
            "new metadata after subscription must be reported as a contract violation"
        );
        assert_eq!(instruments.len(), 2);
    }

    #[test]
    fn metadata_response_ignores_duplicate_instrument_after_subscription() {
        let early = InstrumentId::from("EARLY.POLYMARKET");
        let mut instruments = BTreeMap::from([(early, early)]);

        let update = record_source_owned_metadata_response_instrument(
            &mut instruments,
            true,
            InstrumentId::from("EARLY.POLYMARKET"),
        );

        assert_eq!(
            update,
            MetadataResponseInstrumentUpdate::Existing,
            "duplicate metadata after subscription should not fail the probe"
        );
        assert_eq!(instruments.len(), 1);
    }

    #[test]
    fn metadata_response_install_unregisters_observer_when_send_fails() {
        let subscriptions = vec![
            StrategyFreeReferenceQuoteSubscription {
                data_client_id: ClientId::from("POLYMARKET_MAIN"),
                instrument_id: InstrumentId::from("FIRST.POLYMARKET"),
            },
            StrategyFreeReferenceQuoteSubscription {
                data_client_id: ClientId::from("POLYMARKET_MAIN"),
                instrument_id: InstrumentId::from("SECOND.POLYMARKET"),
            },
        ];
        let sent = RefCell::new(Vec::new());
        let unregistered = Cell::new(false);

        let result = send_metadata_response_probe_subscriptions_with_rollback(
            &subscriptions,
            || (),
            |subscription| {
                sent.borrow_mut()
                    .push(subscription.instrument_id.to_string());
                if subscription.instrument_id == InstrumentId::from("SECOND.POLYMARKET") {
                    return Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
                        reason: "forced subscription failure".to_string(),
                    });
                }
                Ok(())
            },
            |_| unregistered.set(true),
        );

        let error = result.expect_err("second subscription should fail");
        assert!(
            error.to_string().contains("forced subscription failure"),
            "expected send failure to propagate, got: {error}"
        );
        assert_eq!(
            sent.borrow().as_slice(),
            ["FIRST.POLYMARKET", "SECOND.POLYMARKET"]
        );
        assert!(
            unregistered.get(),
            "observer handlers must be unregistered when install fails"
        );
    }
}

fn strategy_free_configured_data_client_probe_subscriptions(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<Vec<StrategyFreeReferenceQuoteSubscription>, BoltV3LiveNodeError> {
    let client = loaded.root.clients.get(client_key).ok_or_else(|| {
        BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
            "data-client readiness probe client_key is not configured"
        ))
    })?;
    let readiness_probe = client.readiness_probe.as_ref().ok_or_else(|| {
        BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
            "data-client readiness probe requires clients.<id>.readiness_probe"
        ))
    })?;

    match readiness_probe.quote_target_source {
        DataClientReadinessProbeQuoteTargetSource::Configured => {
            strategy_free_data_client_readiness_quote_subscription_plan(loaded, client_key)
                .map(|(subscriptions, _)| subscriptions)
        }
        DataClientReadinessProbeQuoteTargetSource::MetadataResponse => Err(
            BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                "configured data-client probe subscription planning requires quote_target_source = \"configured\""
            )),
        ),
    }
}

fn send_strategy_free_probe_subscription(
    subscription: &StrategyFreeReferenceQuoteSubscription,
    market_data_kind: DataClientReadinessProbeMarketDataKind,
    book_type: Option<BookType>,
) -> Result<(), BoltV3LiveNodeError> {
    let ts_init = get_atomic_clock_realtime().get_time_ns();
    let sender = get_data_cmd_sender();
    match market_data_kind {
        DataClientReadinessProbeMarketDataKind::Quote => {
            let command = SubscribeQuotes::new(
                subscription.instrument_id,
                Some(subscription.data_client_id),
                None,
                UUID4::new(),
                ts_init,
                None,
                None,
            );
            sender.execute(DataCommand::Subscribe(SubscribeCommand::Quotes(command)));
        }
        DataClientReadinessProbeMarketDataKind::Book => {
            let command = SubscribeBookDeltas::new(
                subscription.instrument_id,
                book_type.ok_or_else(|| {
                    BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                        "data-client readiness book probe requires clients.<id>.readiness_probe.book_type"
                    ))
                })?,
                Some(subscription.data_client_id),
                None,
                UUID4::new(),
                ts_init,
                None,
                false,
                None,
                None,
            );
            sender.execute(DataCommand::Subscribe(SubscribeCommand::BookDeltas(
                command,
            )));
        }
        DataClientReadinessProbeMarketDataKind::Trade => {
            let command = SubscribeTrades::new(
                subscription.instrument_id,
                Some(subscription.data_client_id),
                None,
                UUID4::new(),
                ts_init,
                None,
                None,
            );
            sender.execute(DataCommand::Subscribe(SubscribeCommand::Trades(command)));
        }
    }
    Ok(())
}

fn subscribe_strategy_free_probe_subscription(
    runtime: &mut BoltV3LiveNodeRuntime,
    subscription: &StrategyFreeReferenceQuoteSubscription,
    market_data_kind: DataClientReadinessProbeMarketDataKind,
    book_type: Option<BookType>,
) -> Result<(), BoltV3LiveNodeError> {
    match market_data_kind {
        DataClientReadinessProbeMarketDataKind::Quote => runtime.subscribe_strategy_free_quotes(
            subscription.data_client_id,
            subscription.instrument_id,
        ),
        DataClientReadinessProbeMarketDataKind::Book => runtime.subscribe_strategy_free_book_deltas(
            subscription.data_client_id,
            subscription.instrument_id,
            book_type.ok_or_else(|| {
                BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                    "data-client readiness book probe requires clients.<id>.readiness_probe.book_type"
                ))
            })?,
        ),
        DataClientReadinessProbeMarketDataKind::Trade => runtime.subscribe_strategy_free_trades(
            subscription.data_client_id,
            subscription.instrument_id,
        ),
    }
}

fn unsubscribe_strategy_free_probe_subscription(
    runtime: &mut BoltV3LiveNodeRuntime,
    subscription: &StrategyFreeReferenceQuoteSubscription,
    market_data_kind: DataClientReadinessProbeMarketDataKind,
) {
    match market_data_kind {
        DataClientReadinessProbeMarketDataKind::Quote => runtime.unsubscribe_strategy_free_quotes(
            subscription.data_client_id,
            subscription.instrument_id,
        ),
        DataClientReadinessProbeMarketDataKind::Book => runtime
            .unsubscribe_strategy_free_book_deltas(
                subscription.data_client_id,
                subscription.instrument_id,
            ),
        DataClientReadinessProbeMarketDataKind::Trade => runtime.unsubscribe_strategy_free_trades(
            subscription.data_client_id,
            subscription.instrument_id,
        ),
    }
}

fn readiness_probe_book_type_to_nt(book_type: DataClientReadinessProbeBookType) -> BookType {
    match book_type {
        DataClientReadinessProbeBookType::L1Mbp => BookType::L1_MBP,
        DataClientReadinessProbeBookType::L2Mbp => BookType::L2_MBP,
        DataClientReadinessProbeBookType::L3Mbo => BookType::L3_MBO,
    }
}

fn readiness_probe_market_data_kind_label(
    market_data_kind: DataClientReadinessProbeMarketDataKind,
) -> &'static str {
    match market_data_kind {
        DataClientReadinessProbeMarketDataKind::Quote => "quote",
        DataClientReadinessProbeMarketDataKind::Book => "book",
        DataClientReadinessProbeMarketDataKind::Trade => "trade",
    }
}

pub(super) fn data_client_probe_loaded_config(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<LoadedBoltV3Config, BoltV3LiveNodeError> {
    if client_key.trim().is_empty() {
        return Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client probe client_key is not configured".to_string(),
        });
    }
    let client = loaded
        .root
        .clients
        .get(client_key)
        .cloned()
        .ok_or_else(|| BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client probe client_key is not configured".to_string(),
        })?;
    if client.data.is_none() {
        return Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client probe requires the selected client to declare [data]".to_string(),
        });
    }
    let mut probe_loaded = loaded.clone();
    probe_loaded
        .root
        .clients
        .retain(|configured_key, _| configured_key == client_key);
    probe_loaded
        .strategies
        .retain(|strategy| strategy.config.execution_client_id == ClientId::from(client_key));
    Ok(probe_loaded)
}
