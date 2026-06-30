use super::*;

#[derive(Debug, Clone, PartialEq)]
pub struct IvEngineLifecyclePlan {
    pub start_plans: Vec<IvSubscriptionPlan>,
    pub reload_plans: Vec<IvSubscriptionPlan>,
    pub stop_plans: Vec<IvSubscriptionPlan>,
}

pub fn plan_iv_engine_lifecycle(
    root: &BoltV3RootConfig,
) -> Result<IvEngineLifecyclePlan, IvSubscriptionError> {
    let Some(iv) = &root.iv else {
        return Ok(IvEngineLifecyclePlan {
            start_plans: Vec::new(),
            reload_plans: Vec::new(),
            stop_plans: Vec::new(),
        });
    };

    let mut start_plans = Vec::new();
    let reload_plans = Vec::new();
    let mut stop_plans = Vec::new();
    for profile in &iv.profiles {
        let subscription_config = profile.subscription_config();
        start_plans.extend(plan_profile_start(&subscription_config)?);
        stop_plans.extend(plan_profile_stop(&subscription_config)?);
    }

    Ok(IvEngineLifecyclePlan {
        start_plans,
        reload_plans,
        stop_plans,
    })
}

pub fn plan_iv_engine_reload_lifecycle(
    current_root: &BoltV3RootConfig,
    next_root: &BoltV3RootConfig,
) -> Result<IvEngineLifecyclePlan, IvSubscriptionError> {
    let current_profiles = current_root.iv.as_ref().map(|iv| &iv.profiles);
    let next_profiles = next_root.iv.as_ref().map(|iv| &iv.profiles);
    let mut start_plans = Vec::new();
    let mut reload_plans = Vec::new();
    let mut stop_plans = Vec::new();

    match (current_profiles, next_profiles) {
        (None, None) => {}
        (None, Some(next_profiles)) => {
            for profile in next_profiles {
                start_plans.extend(plan_profile_start(&profile.subscription_config())?);
            }
        }
        (Some(current_profiles), None) => {
            for profile in current_profiles {
                stop_plans.extend(plan_profile_stop(&profile.subscription_config())?);
            }
        }
        (Some(current_profiles), Some(next_profiles)) => {
            let current_by_id = current_profiles
                .iter()
                .map(|profile| (&profile.profile_id, profile))
                .collect::<BTreeMap<_, _>>();
            let next_by_id = next_profiles
                .iter()
                .map(|profile| (&profile.profile_id, profile))
                .collect::<BTreeMap<_, _>>();

            for current_profile in current_profiles {
                if let Some(next_profile) = next_by_id.get(&current_profile.profile_id) {
                    reload_plans.extend(plan_profile_reload(
                        &current_profile.subscription_config(),
                        &next_profile.subscription_config(),
                    )?);
                } else {
                    stop_plans.extend(plan_profile_stop(&current_profile.subscription_config())?);
                }
            }

            for next_profile in next_profiles {
                if !current_by_id.contains_key(&next_profile.profile_id) {
                    start_plans.extend(plan_profile_start(&next_profile.subscription_config())?);
                }
            }
        }
    }

    Ok(IvEngineLifecyclePlan {
        start_plans,
        reload_plans,
        stop_plans,
    })
}

pub struct BoltV3IvRuntimeEventBindings {
    option_greeks: Vec<BoltV3IvOptionGreeksRuntimeEventBinding>,
    option_chains: Vec<BoltV3IvOptionChainRuntimeEventBinding>,
    custom_data: Vec<BoltV3IvCustomDataRuntimeEventBinding>,
}

struct BoltV3IvOptionGreeksRuntimeEventBinding {
    pattern: MStr<Pattern>,
    handler: TypedHandler<OptionGreeks>,
}

struct BoltV3IvOptionChainRuntimeEventBinding {
    pattern: MStr<Pattern>,
    handler: TypedHandler<OptionChainSlice>,
}

struct BoltV3IvCustomDataRuntimeEventBinding {
    pattern: MStr<Pattern>,
    handler: ShareableMessageHandler,
}

impl Drop for BoltV3IvRuntimeEventBindings {
    fn drop(&mut self) {
        for binding in self.option_greeks.drain(..) {
            msgbus::unsubscribe_option_greeks(binding.pattern, &binding.handler);
        }
        for binding in self.option_chains.drain(..) {
            msgbus::unsubscribe_option_chain(binding.pattern, &binding.handler);
        }
        for binding in self.custom_data.drain(..) {
            msgbus::unsubscribe_any(binding.pattern, &binding.handler);
        }
    }
}

pub fn wire_bolt_v3_iv_runtime_event_bindings(
    iv: &IvRootConfig,
    runtime: &IvRuntimeEngine,
) -> Result<BoltV3IvRuntimeEventBindings, BoltV3StrategyRegistrationError> {
    let mut bindings = BoltV3IvRuntimeEventBindings {
        option_greeks: Vec::new(),
        option_chains: Vec::new(),
        custom_data: Vec::new(),
    };

    for profile in &iv.profiles {
        for source in &profile.sources {
            match (&source.source_kind, &source.selector) {
                (
                    IvSourceKind::OptionGreeks,
                    IvSelector::SourceOptionGreeks { instrument_ids, .. },
                ) => {
                    let instrument_ids = parse_option_greeks_instrument_ids(instrument_ids)
                        .map_err(|message| {
                            iv_runtime_event_binding_error(
                                &profile.profile_id,
                                &source.source_id,
                                message,
                            )
                        })?;
                    for instrument_id in instrument_ids {
                        bindings
                            .option_greeks
                            .push(wire_option_greeks_event_binding(
                                &profile.profile_id,
                                &source.source_id,
                                instrument_id,
                                runtime,
                            ));
                    }
                }
                (IvSourceKind::OptionChain, IvSelector::SourceOptionChain { series_ids, .. }) => {
                    let series_ids =
                        parse_option_chain_series_ids(series_ids).map_err(|message| {
                            iv_runtime_event_binding_error(
                                &profile.profile_id,
                                &source.source_id,
                                message,
                            )
                        })?;
                    for series_id in series_ids {
                        bindings.option_chains.push(wire_option_chain_event_binding(
                            &profile.profile_id,
                            &source.source_id,
                            series_id,
                            runtime,
                        ));
                    }
                }
                (
                    IvSourceKind::AggregateGreeks,
                    IvSelector::SourceAggregateGreeks {
                        aggregate_key,
                        underlying_selectors,
                        nt_params,
                        ..
                    },
                ) => {
                    let (data_type, _) = aggregate_greeks_data_type_for_source(
                        &source.source_id,
                        aggregate_key,
                        underlying_selectors,
                        &source.params,
                        nt_params,
                    )
                    .map_err(|message| {
                        iv_runtime_event_binding_error(
                            &profile.profile_id,
                            &source.source_id,
                            message,
                        )
                    })?;
                    bindings
                        .custom_data
                        .push(wire_aggregate_greeks_custom_data_event_binding(
                            &profile.profile_id,
                            &source.source_id,
                            data_type,
                            runtime,
                        ));
                }
                (
                    IvSourceKind::CustomImpliedVolatility,
                    IvSelector::SourceCustomImpliedVolatility {
                        custom_iv_data_type,
                        nt_params,
                        ..
                    },
                ) => {
                    let (data_type, _) = custom_iv_data_type_for_source(
                        &source.source_id,
                        custom_iv_data_type,
                        &source.params,
                        nt_params,
                    )
                    .map_err(|message| {
                        iv_runtime_event_binding_error(
                            &profile.profile_id,
                            &source.source_id,
                            message,
                        )
                    })?;
                    bindings.custom_data.push(wire_custom_iv_event_binding(
                        &profile.profile_id,
                        &source.source_id,
                        data_type,
                        runtime,
                    ));
                }
                _ => {}
            }
        }
    }

    Ok(bindings)
}

pub(super) fn parse_option_greeks_instrument_ids(
    instrument_ids: &[String],
) -> Result<Vec<InstrumentId>, String> {
    instrument_ids
        .iter()
        .map(|instrument_id| {
            InstrumentId::from_str(instrument_id).map_err(|error| {
                format!("invalid NT option-greeks instrument_id {instrument_id}: {error}")
            })
        })
        .collect()
}

pub(super) fn parse_option_chain_series_ids(
    series_ids: &[String],
) -> Result<Vec<OptionSeriesId>, String> {
    series_ids
        .iter()
        .map(|series_id| {
            OptionSeriesId::from_str(series_id)
                .map_err(|error| format!("invalid NT option-chain series_id {series_id}: {error}"))
        })
        .collect()
}

fn wire_aggregate_greeks_custom_data_event_binding(
    profile_id: &str,
    source_id: &str,
    data_type: DataType,
    runtime: &IvRuntimeEngine,
) -> BoltV3IvCustomDataRuntimeEventBinding {
    let pattern = switchboard::get_custom_topic(&data_type).into();
    let runtime = runtime.clone();
    let profile_id = profile_id.to_string();
    let source_id = source_id.to_string();
    let handler = ShareableMessageHandler::from_typed(move |custom_data: &CustomData| {
        if let Err(error) = runtime.ingest_nt_aggregate_greeks_custom_data(
            &profile_id,
            &source_id,
            custom_data,
            iv_runtime_event_received_ts_ns(),
        ) {
            log::error!("bolt-v3 IV aggregate-greeks custom-data ingest failed: {error:?}");
        }
    });
    msgbus::subscribe_any(pattern, handler.clone(), None);
    BoltV3IvCustomDataRuntimeEventBinding { pattern, handler }
}

fn wire_custom_iv_event_binding(
    profile_id: &str,
    source_id: &str,
    data_type: DataType,
    runtime: &IvRuntimeEngine,
) -> BoltV3IvCustomDataRuntimeEventBinding {
    let pattern = switchboard::get_custom_topic(&data_type).into();
    let runtime = runtime.clone();
    let profile_id = profile_id.to_string();
    let source_id = source_id.to_string();
    let handler = ShareableMessageHandler::from_typed(move |custom_data: &CustomData| {
        if let Err(error) = runtime.ingest_nt_custom_iv_data(
            &profile_id,
            &source_id,
            custom_data,
            iv_runtime_event_received_ts_ns(),
        ) {
            log::error!("bolt-v3 IV custom-IV custom-data ingest failed: {error:?}");
        }
    });
    msgbus::subscribe_any(pattern, handler.clone(), None);
    BoltV3IvCustomDataRuntimeEventBinding { pattern, handler }
}

fn wire_option_greeks_event_binding(
    profile_id: &str,
    source_id: &str,
    instrument_id: InstrumentId,
    runtime: &IvRuntimeEngine,
) -> BoltV3IvOptionGreeksRuntimeEventBinding {
    let pattern = switchboard::get_option_greeks_topic(instrument_id).into();
    let runtime = runtime.clone();
    let profile_id = profile_id.to_string();
    let source_id = source_id.to_string();
    let handler = TypedHandler::from(move |option_greeks: &OptionGreeks| {
        if let Err(error) = runtime.ingest_nt_option_greeks(
            &profile_id,
            &source_id,
            option_greeks,
            iv_runtime_event_received_ts_ns(),
        ) {
            log::error!("bolt-v3 IV option-greeks event ingest failed: {error:?}");
        }
    });
    msgbus::subscribe_option_greeks(pattern, handler.clone(), None);
    BoltV3IvOptionGreeksRuntimeEventBinding { pattern, handler }
}

fn wire_option_chain_event_binding(
    profile_id: &str,
    source_id: &str,
    series_id: OptionSeriesId,
    runtime: &IvRuntimeEngine,
) -> BoltV3IvOptionChainRuntimeEventBinding {
    let pattern = switchboard::get_option_chain_topic(series_id).into();
    let runtime = runtime.clone();
    let profile_id = profile_id.to_string();
    let source_id = source_id.to_string();
    let handler = TypedHandler::from(move |option_chain: &OptionChainSlice| {
        if let Err(error) = runtime.ingest_nt_option_chain_slice(
            &profile_id,
            &source_id,
            option_chain,
            iv_runtime_event_received_ts_ns(),
        ) {
            log::error!("bolt-v3 IV option-chain event ingest failed: {error:?}");
        }
    });
    msgbus::subscribe_option_chain(pattern, handler.clone(), None);
    BoltV3IvOptionChainRuntimeEventBinding { pattern, handler }
}

fn iv_runtime_event_received_ts_ns() -> UnixNanos {
    UnixNanos::new(get_atomic_clock_realtime().get_time_ns().as_u64())
}

fn iv_runtime_event_binding_error(
    profile_id: &str,
    source_id: &str,
    message: String,
) -> BoltV3StrategyRegistrationError {
    BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
        message: format!(
            "bolt-v3 IV runtime event binding failed for profile {profile_id} source {source_id}: {message}"
        ),
    }
}

pub(super) fn iv_runtime_data_commands_for_plan(
    plan: &IvSubscriptionPlan,
) -> Result<Vec<DataCommand>, IvRuntimeBindingError> {
    let ts_init = get_atomic_clock_realtime().get_time_ns();
    let client_id = Some(ClientId::from(plan.client_id.as_str()));

    match (plan.operation, &plan.selector) {
        (
            IvRuntimeOperation::SubscribeOptionGreeks | IvRuntimeOperation::UnsubscribeOptionGreeks,
            IvSelector::SourceOptionGreeks {
                instrument_ids,
                nt_params,
            },
        ) => {
            let params = merged_nt_params(plan, nt_params)?;
            let commands = parse_option_greeks_instrument_ids(instrument_ids)
                .map_err(|message| binding_error(plan, message))?
                .into_iter()
                .map(|instrument_id| {
                    if plan.operation == IvRuntimeOperation::SubscribeOptionGreeks {
                        DataCommand::Subscribe(SubscribeCommand::OptionGreeks(
                            SubscribeOptionGreeks::new(
                                instrument_id,
                                client_id,
                                None,
                                UUID4::new(),
                                ts_init,
                                None,
                                params.clone(),
                            ),
                        ))
                    } else {
                        DataCommand::Unsubscribe(UnsubscribeCommand::OptionGreeks(
                            UnsubscribeOptionGreeks::new(
                                instrument_id,
                                client_id,
                                None,
                                UUID4::new(),
                                ts_init,
                                None,
                                params.clone(),
                            ),
                        ))
                    }
                })
                .collect();
            Ok(commands)
        }
        (
            IvRuntimeOperation::SubscribeOptionChain | IvRuntimeOperation::UnsubscribeOptionChain,
            IvSelector::SourceOptionChain {
                series_ids,
                strike_range_policy,
                nt_params,
            },
        ) => {
            let params = merged_nt_params(plan, nt_params)?;
            let strike_range = parse_nt_strike_range(plan, strike_range_policy)?;
            let snapshot_interval_ms = params
                .as_ref()
                .and_then(|params| params.get_u64("snapshot_interval_ms"));
            let commands = parse_option_chain_series_ids(series_ids)
                .map_err(|message| binding_error(plan, message))?
                .into_iter()
                .map(|series_id| {
                    if plan.operation == IvRuntimeOperation::SubscribeOptionChain {
                        DataCommand::Subscribe(SubscribeCommand::OptionChain(
                            SubscribeOptionChain::new(
                                series_id,
                                strike_range.clone(),
                                snapshot_interval_ms,
                                UUID4::new(),
                                ts_init,
                                client_id,
                                None,
                                params.clone(),
                            ),
                        ))
                    } else {
                        DataCommand::Unsubscribe(UnsubscribeCommand::OptionChain(
                            UnsubscribeOptionChain::new(
                                series_id,
                                UUID4::new(),
                                ts_init,
                                client_id,
                                None,
                            ),
                        ))
                    }
                })
                .collect();
            Ok(commands)
        }
        (
            IvRuntimeOperation::SubscribeCustomData | IvRuntimeOperation::UnsubscribeCustomData,
            IvSelector::SourceCustomImpliedVolatility {
                custom_iv_data_type,
                nt_params,
                ..
            },
        ) => {
            let (data_type, params) = custom_iv_data_type_for_source(
                &plan.source_id,
                custom_iv_data_type,
                &plan.params,
                nt_params,
            )
            .map_err(|message| binding_error(plan, message))?;
            Ok(vec![custom_data_command(
                plan.operation,
                client_id,
                data_type,
                params,
                ts_init,
            )])
        }
        (
            IvRuntimeOperation::SubscribeAggregateGreeks
            | IvRuntimeOperation::UnsubscribeAggregateGreeks,
            IvSelector::SourceAggregateGreeks {
                aggregate_key,
                underlying_selectors,
                nt_params,
                ..
            },
        ) => {
            let (data_type, params) = aggregate_greeks_data_type_for_source(
                &plan.source_id,
                aggregate_key,
                underlying_selectors,
                &plan.params,
                nt_params,
            )
            .map_err(|message| binding_error(plan, message))?;
            Ok(vec![custom_data_command(
                plan.operation,
                client_id,
                data_type,
                params,
                ts_init,
            )])
        }
        (IvRuntimeOperation::RemoveSource, _) => Ok(Vec::new()),
        _ => Err(binding_error(
            plan,
            "IV subscription plan operation does not match selector kind".to_string(),
        )),
    }
}

fn custom_data_command(
    operation: IvRuntimeOperation,
    client_id: Option<ClientId>,
    data_type: DataType,
    params: Option<Params>,
    ts_init: nautilus_core::UnixNanos,
) -> DataCommand {
    match operation {
        IvRuntimeOperation::SubscribeCustomData | IvRuntimeOperation::SubscribeAggregateGreeks => {
            DataCommand::Subscribe(SubscribeCommand::Data(SubscribeCustomData::new(
                client_id,
                None,
                data_type,
                UUID4::new(),
                ts_init,
                None,
                params,
            )))
        }
        IvRuntimeOperation::UnsubscribeCustomData
        | IvRuntimeOperation::UnsubscribeAggregateGreeks => {
            DataCommand::Unsubscribe(UnsubscribeCommand::Data(UnsubscribeCustomData::new(
                client_id,
                None,
                data_type,
                UUID4::new(),
                ts_init,
                None,
                params,
            )))
        }
        _ => unreachable!("custom data command requires a custom-data IV runtime operation"),
    }
}

pub(super) struct NtIvRuntimeCommandSenderAdapter {
    allowed_data_client_ids: BTreeSet<ClientId>,
    external_client_ids: BTreeSet<ClientId>,
}

impl NtIvRuntimeCommandSenderAdapter {
    pub(super) fn new(
        registered_data_clients: &[ClientId],
        configured_external_clients: &[ClientId],
    ) -> Self {
        let mut allowed_data_client_ids = registered_data_clients
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        allowed_data_client_ids.extend(configured_external_clients.iter().cloned());
        Self {
            allowed_data_client_ids,
            external_client_ids: configured_external_clients.iter().cloned().collect(),
        }
    }

    fn is_external_client(&self, plan: &IvSubscriptionPlan) -> bool {
        self.external_client_ids
            .contains(&ClientId::from(plan.client_id.as_str()))
    }

    fn validate_client_id(&self, plan: &IvSubscriptionPlan) -> Result<(), IvRuntimeBindingError> {
        let client_id = ClientId::from(plan.client_id.as_str());
        if self.allowed_data_client_ids.contains(&client_id) {
            Ok(())
        } else {
            Err(binding_error(
                plan,
                format!(
                    "IV source client_id {} is not registered as an NT data client or configured external data client",
                    plan.client_id
                ),
            ))
        }
    }
}

impl IvRuntimeBindingAdapter for NtIvRuntimeCommandSenderAdapter {
    fn apply_subscription_plan(
        &mut self,
        plan: &IvSubscriptionPlan,
    ) -> Result<(), IvRuntimeBindingError> {
        if self.is_external_client(plan) {
            return Ok(());
        }
        self.validate_client_id(plan)?;

        let sender = get_data_cmd_sender();
        for command in iv_runtime_data_commands_for_plan(plan)? {
            sender.execute(command);
        }
        Ok(())
    }
}

pub(super) struct NtIvRuntimePlanValidationAdapter {
    allowed_data_client_ids: BTreeSet<ClientId>,
}

impl NtIvRuntimePlanValidationAdapter {
    pub(super) fn new(node: &LiveNode, configured_external_clients: &[ClientId]) -> Self {
        let mut allowed_data_client_ids = node
            .kernel()
            .data_engine
            .borrow()
            .registered_clients()
            .into_iter()
            .collect::<BTreeSet<_>>();
        allowed_data_client_ids.extend(configured_external_clients.iter().cloned());
        Self {
            allowed_data_client_ids,
        }
    }

    fn validate_client_id(&self, plan: &IvSubscriptionPlan) -> Result<(), IvRuntimeBindingError> {
        let client_id = ClientId::from(plan.client_id.as_str());
        if self.allowed_data_client_ids.contains(&client_id) {
            Ok(())
        } else {
            Err(binding_error(
                plan,
                format!(
                    "IV source client_id {} is not registered as an NT data client or configured external data client",
                    plan.client_id
                ),
            ))
        }
    }
}

impl IvRuntimeBindingAdapter for NtIvRuntimePlanValidationAdapter {
    fn apply_subscription_plan(
        &mut self,
        plan: &IvSubscriptionPlan,
    ) -> Result<(), IvRuntimeBindingError> {
        self.validate_client_id(plan)?;
        iv_runtime_data_commands_for_plan(plan)?;
        Ok(())
    }
}

pub(super) struct NtIvRuntimeBindingAdapter<'a> {
    node: &'a mut LiveNode,
    allowed_data_client_ids: BTreeSet<ClientId>,
    external_client_ids: BTreeSet<ClientId>,
}

impl<'a> NtIvRuntimeBindingAdapter<'a> {
    pub(super) fn new(node: &'a mut LiveNode, configured_external_clients: &[ClientId]) -> Self {
        let mut allowed_data_client_ids = node
            .kernel()
            .data_engine
            .borrow()
            .registered_clients()
            .into_iter()
            .collect::<BTreeSet<_>>();
        allowed_data_client_ids.extend(configured_external_clients.iter().cloned());
        Self {
            node,
            allowed_data_client_ids,
            external_client_ids: configured_external_clients.iter().cloned().collect(),
        }
    }

    fn is_external_client(&self, plan: &IvSubscriptionPlan) -> bool {
        self.external_client_ids
            .contains(&ClientId::from(plan.client_id.as_str()))
    }

    fn client_id(&self, plan: &IvSubscriptionPlan) -> Result<ClientId, IvRuntimeBindingError> {
        let client_id = ClientId::from(plan.client_id.as_str());
        if self.allowed_data_client_ids.contains(&client_id) {
            Ok(client_id)
        } else {
            Err(binding_error(
                plan,
                format!(
                    "IV source client_id {} is not registered as an NT data client or configured external data client",
                    plan.client_id
                ),
            ))
        }
    }

    fn apply_option_greeks(
        &mut self,
        plan: &IvSubscriptionPlan,
        instrument_ids: &[String],
        nt_params: &toml::Value,
        subscribe: bool,
    ) -> Result<(), IvRuntimeBindingError> {
        let params = merged_nt_params(plan, nt_params)?;
        let client_id = Some(self.client_id(plan)?);
        let instrument_ids = parse_option_greeks_instrument_ids(instrument_ids)
            .map_err(|message| binding_error(plan, message))?;
        for instrument_id in instrument_ids {
            let ts_init = self.node.kernel().clock.borrow().timestamp_ns();
            if subscribe {
                let command = SubscribeOptionGreeks::new(
                    instrument_id,
                    client_id,
                    None,
                    UUID4::new(),
                    ts_init,
                    None,
                    params.clone(),
                );
                self.node
                    .kernel()
                    .data_engine
                    .borrow_mut()
                    .execute_subscribe(SubscribeCommand::OptionGreeks(command))
                    .map_err(|error| binding_error(plan, error.to_string()))?;
            } else {
                let command = UnsubscribeOptionGreeks::new(
                    instrument_id,
                    client_id,
                    None,
                    UUID4::new(),
                    ts_init,
                    None,
                    params.clone(),
                );
                self.node
                    .kernel()
                    .data_engine
                    .borrow_mut()
                    .execute_unsubscribe(&UnsubscribeCommand::OptionGreeks(command))
                    .map_err(|error| binding_error(plan, error.to_string()))?;
            }
        }
        Ok(())
    }

    fn apply_option_chain(
        &mut self,
        plan: &IvSubscriptionPlan,
        series_ids: &[String],
        strike_range_policy: &str,
        nt_params: &toml::Value,
        subscribe: bool,
    ) -> Result<(), IvRuntimeBindingError> {
        let params = merged_nt_params(plan, nt_params)?;
        let strike_range = parse_nt_strike_range(plan, strike_range_policy)?;
        let snapshot_interval_ms = params
            .as_ref()
            .and_then(|params| params.get_u64("snapshot_interval_ms"));
        let client_id = Some(self.client_id(plan)?);
        let series_ids = parse_option_chain_series_ids(series_ids)
            .map_err(|message| binding_error(plan, message))?;
        for series_id in series_ids {
            let ts_init = self.node.kernel().clock.borrow().timestamp_ns();
            if subscribe {
                let command = SubscribeOptionChain::new(
                    series_id,
                    strike_range.clone(),
                    snapshot_interval_ms,
                    UUID4::new(),
                    ts_init,
                    client_id,
                    None,
                    params.clone(),
                );
                self.node
                    .kernel()
                    .data_engine
                    .borrow_mut()
                    .execute_subscribe(SubscribeCommand::OptionChain(command))
                    .map_err(|error| binding_error(plan, error.to_string()))?;
            } else {
                let command =
                    UnsubscribeOptionChain::new(series_id, UUID4::new(), ts_init, client_id, None);
                self.node
                    .kernel()
                    .data_engine
                    .borrow_mut()
                    .execute_unsubscribe(&UnsubscribeCommand::OptionChain(command))
                    .map_err(|error| binding_error(plan, error.to_string()))?;
            }
        }
        Ok(())
    }

    fn apply_custom_data(
        &mut self,
        plan: &IvSubscriptionPlan,
        custom_iv_data_type: &str,
        nt_params: &toml::Value,
        subscribe: bool,
    ) -> Result<(), IvRuntimeBindingError> {
        let (data_type, params) = custom_iv_data_type_for_source(
            &plan.source_id,
            custom_iv_data_type,
            &plan.params,
            nt_params,
        )
        .map_err(|message| binding_error(plan, message))?;
        self.execute_custom_data(plan, data_type, params, subscribe)
    }

    fn apply_aggregate_greeks(
        &mut self,
        plan: &IvSubscriptionPlan,
        aggregate_key: &str,
        underlying_selectors: &[String],
        nt_params: &toml::Value,
        subscribe: bool,
    ) -> Result<(), IvRuntimeBindingError> {
        let (data_type, params) = aggregate_greeks_data_type_for_source(
            &plan.source_id,
            aggregate_key,
            underlying_selectors,
            &plan.params,
            nt_params,
        )
        .map_err(|message| binding_error(plan, message))?;
        self.execute_custom_data(plan, data_type, params, subscribe)
    }

    fn execute_custom_data(
        &mut self,
        plan: &IvSubscriptionPlan,
        data_type: DataType,
        params: Option<Params>,
        subscribe: bool,
    ) -> Result<(), IvRuntimeBindingError> {
        let client_id = Some(self.client_id(plan)?);
        let ts_init = self.node.kernel().clock.borrow().timestamp_ns();
        if subscribe {
            let command = SubscribeCustomData::new(
                client_id,
                None,
                data_type,
                UUID4::new(),
                ts_init,
                None,
                params,
            );
            self.node
                .kernel()
                .data_engine
                .borrow_mut()
                .execute_subscribe(SubscribeCommand::Data(command))
                .map_err(|error| binding_error(plan, error.to_string()))?;
        } else {
            let command = UnsubscribeCustomData::new(
                client_id,
                None,
                data_type,
                UUID4::new(),
                ts_init,
                None,
                params,
            );
            self.node
                .kernel()
                .data_engine
                .borrow_mut()
                .execute_unsubscribe(&UnsubscribeCommand::Data(command))
                .map_err(|error| binding_error(plan, error.to_string()))?;
        }
        Ok(())
    }
}

impl IvRuntimeBindingAdapter for NtIvRuntimeBindingAdapter<'_> {
    fn apply_subscription_plan(
        &mut self,
        plan: &IvSubscriptionPlan,
    ) -> Result<(), IvRuntimeBindingError> {
        if self.is_external_client(plan) {
            return Ok(());
        }

        match (plan.operation, &plan.selector) {
            (
                IvRuntimeOperation::SubscribeOptionGreeks
                | IvRuntimeOperation::UnsubscribeOptionGreeks,
                IvSelector::SourceOptionGreeks {
                    instrument_ids,
                    nt_params,
                },
            ) => self.apply_option_greeks(
                plan,
                instrument_ids,
                nt_params,
                plan.operation == IvRuntimeOperation::SubscribeOptionGreeks,
            ),
            (
                IvRuntimeOperation::SubscribeOptionChain
                | IvRuntimeOperation::UnsubscribeOptionChain,
                IvSelector::SourceOptionChain {
                    series_ids,
                    strike_range_policy,
                    nt_params,
                },
            ) => self.apply_option_chain(
                plan,
                series_ids,
                strike_range_policy,
                nt_params,
                plan.operation == IvRuntimeOperation::SubscribeOptionChain,
            ),
            (
                IvRuntimeOperation::SubscribeCustomData | IvRuntimeOperation::UnsubscribeCustomData,
                IvSelector::SourceCustomImpliedVolatility {
                    custom_iv_data_type,
                    nt_params,
                    ..
                },
            ) => self.apply_custom_data(
                plan,
                custom_iv_data_type,
                nt_params,
                plan.operation == IvRuntimeOperation::SubscribeCustomData,
            ),
            (
                IvRuntimeOperation::SubscribeAggregateGreeks
                | IvRuntimeOperation::UnsubscribeAggregateGreeks,
                IvSelector::SourceAggregateGreeks {
                    aggregate_key,
                    underlying_selectors,
                    nt_params,
                    ..
                },
            ) => self.apply_aggregate_greeks(
                plan,
                aggregate_key,
                underlying_selectors,
                nt_params,
                plan.operation == IvRuntimeOperation::SubscribeAggregateGreeks,
            ),
            (IvRuntimeOperation::RemoveSource, _) => Ok(()),
            _ => Err(binding_error(
                plan,
                "IV subscription operation does not match selector kind".to_string(),
            )),
        }
    }
}

fn binding_error(plan: &IvSubscriptionPlan, message: String) -> IvRuntimeBindingError {
    IvRuntimeBindingError::subscription_failed(plan, message)
}

fn custom_iv_data_type_for_source(
    source_id: &str,
    custom_iv_data_type: &str,
    source_params: &toml::Value,
    selector_nt_params: &toml::Value,
) -> Result<(DataType, Option<Params>), String> {
    let params = merged_nt_params_from_values(source_params, selector_nt_params)?;
    let data_type = DataType::new(
        custom_iv_data_type,
        params.clone(),
        Some(source_id.to_string()),
    );
    Ok((data_type, params))
}

fn aggregate_greeks_data_type_for_source(
    source_id: &str,
    aggregate_key: &str,
    underlying_selectors: &[String],
    source_params: &toml::Value,
    selector_nt_params: &toml::Value,
) -> Result<(DataType, Option<Params>), String> {
    let mut params = merged_nt_params_from_values(source_params, selector_nt_params)?
        .unwrap_or_else(Params::new);
    params.insert(
        "underlying_selectors".to_string(),
        serde_json::Value::Array(
            underlying_selectors
                .iter()
                .cloned()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    let params = Some(params);
    let data_type = DataType::new(aggregate_key, params.clone(), Some(source_id.to_string()));
    Ok((data_type, params))
}

fn merged_nt_params(
    plan: &IvSubscriptionPlan,
    selector_nt_params: &toml::Value,
) -> Result<Option<Params>, IvRuntimeBindingError> {
    merged_nt_params_from_values(&plan.params, selector_nt_params)
        .map_err(|message| binding_error(plan, message))
}

fn merged_nt_params_from_values(
    source_params: &toml::Value,
    selector_nt_params: &toml::Value,
) -> Result<Option<Params>, String> {
    let mut params = Params::new();
    insert_toml_params(&mut params, source_params, "source params")?;
    insert_toml_params(&mut params, selector_nt_params, "selector nt_params")?;
    if params.is_empty() {
        Ok(None)
    } else {
        Ok(Some(params))
    }
}

fn insert_toml_params(params: &mut Params, value: &toml::Value, label: &str) -> Result<(), String> {
    let toml::Value::Table(table) = value else {
        return Err(format!(
            "{label} must be a TOML table for NT params conversion"
        ));
    };
    for (key, value) in table {
        let value = serde_json::to_value(value).map_err(|error| {
            format!("failed to convert {label} key {key} into NT params: {error}")
        })?;
        params.insert(key.clone(), value);
    }
    Ok(())
}

fn parse_nt_strike_range(
    plan: &IvSubscriptionPlan,
    strike_range_policy: &str,
) -> Result<StrikeRange, IvRuntimeBindingError> {
    if let Some(pct) = strike_range_policy.strip_prefix("atm_percent:") {
        return pct
            .parse::<f64>()
            .map(|pct| StrikeRange::AtmPercent { pct })
            .map_err(|error| {
                binding_error(
                    plan,
                    format!("invalid atm_percent strike range policy: {error}"),
                )
            });
    }
    if let Some(relative) = strike_range_policy.strip_prefix("atm_relative:") {
        let Some((above, below)) = relative.split_once(':') else {
            return Err(binding_error(
                plan,
                "atm_relative strike range policy must be atm_relative:<above>:<below>".to_string(),
            ));
        };
        let strikes_above = above.parse::<usize>().map_err(|error| {
            binding_error(
                plan,
                format!("invalid atm_relative strikes_above value: {error}"),
            )
        })?;
        let strikes_below = below.parse::<usize>().map_err(|error| {
            binding_error(
                plan,
                format!("invalid atm_relative strikes_below value: {error}"),
            )
        })?;
        return Ok(StrikeRange::AtmRelative {
            strikes_above,
            strikes_below,
        });
    }
    if let Some(fixed) = strike_range_policy.strip_prefix("fixed:") {
        let mut strikes = Vec::new();
        for strike in fixed.split(',') {
            strikes.push(Price::from_str(strike.trim()).map_err(|error| {
                binding_error(plan, format!("invalid fixed strike range value: {error}"))
            })?);
        }
        return Ok(StrikeRange::Fixed(strikes));
    }
    Err(binding_error(
        plan,
        "strike_range_policy must be parseable as atm_percent:<pct>, atm_relative:<above>:<below>, or fixed:<strike,...>".to_string(),
    ))
}
