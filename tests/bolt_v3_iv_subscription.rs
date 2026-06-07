use bolt_v2::bolt_v3_iv::{
    health::IvSourceHealthState,
    runtime::{IvRuntimeBindingAdapter, apply_subscription_plans},
    selector::IvSelector,
    subscription::{
        IvNtSubscriptionKind, IvProfileSubscriptionConfig, IvRuntimeOperation,
        IvSourceSubscriptionConfig, IvSubscriptionLifecycle, IvSubscriptionPlan,
        plan_profile_reload, plan_profile_start,
    },
    types::IvSourceKind,
};

fn profile_id() -> String {
    "iv-profile".to_string()
}

fn profile(sources: Vec<IvSourceSubscriptionConfig>) -> IvProfileSubscriptionConfig {
    IvProfileSubscriptionConfig {
        profile_id: profile_id(),
        sources,
    }
}

fn source(
    source_id: &str,
    source_kind: IvSourceKind,
    client_id: &str,
    selector: IvSelector,
    params: toml::Value,
    subscription_generation: u64,
) -> IvSourceSubscriptionConfig {
    IvSourceSubscriptionConfig {
        source_id: source_id.to_string(),
        source_kind,
        client_id: client_id.to_string(),
        selector,
        params,
        subscription_generation,
    }
}

#[derive(Default)]
struct RecordingRuntimeAdapter {
    applied: Vec<IvSubscriptionPlan>,
}

impl IvRuntimeBindingAdapter for RecordingRuntimeAdapter {
    fn apply_subscription_plan(
        &mut self,
        plan: &IvSubscriptionPlan,
    ) -> Result<(), bolt_v2::bolt_v3_iv::runtime::IvRuntimeBindingError> {
        self.applied.push(plan.clone());
        Ok(())
    }
}

#[test]
fn option_greeks_sources_plan_nt_subscribe_operations() {
    let selector = IvSelector::SourceOptionGreeks {
        instrument_ids: vec!["configured-instrument-a".to_string()],
        nt_params: toml::toml! {
            configured_nt_param = "greeks-value"
        }
        .into(),
    };
    let params: toml::Value = toml::toml! {
        configured_source_param = "greeks-source-value"
    }
    .into();
    let source = source(
        "greeks-source",
        IvSourceKind::OptionGreeks,
        "configured-client",
        selector.clone(),
        params.clone(),
        7,
    );

    let plans = plan_profile_start(&profile(vec![source])).unwrap();

    assert_eq!(
        plans,
        vec![IvSubscriptionPlan {
            profile_id: profile_id(),
            source_id: "greeks-source".to_string(),
            lifecycle: IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeOptionGreeks,
            nt_source_kind: IvNtSubscriptionKind::OptionGreeks,
            client_id: "configured-client".to_string(),
            selector,
            params,
            subscription_generation: 7,
        }]
    );

    let mut adapter = RecordingRuntimeAdapter::default();
    let outcomes = apply_subscription_plans(&mut adapter, &plans);

    assert_eq!(adapter.applied, plans);
    assert_eq!(
        outcomes[0].source_health.subscription_state,
        IvSourceHealthState::Active
    );
}

#[test]
fn option_chain_sources_plan_nt_subscribe_operations() {
    let selector = IvSelector::SourceOptionChain {
        series_ids: vec![
            "configured-series-a".to_string(),
            "configured-series-b".to_string(),
        ],
        strike_range_policy: "configured-strike-range-policy".to_string(),
        nt_params: toml::toml! {
            configured_nt_param = "chain-value"
        }
        .into(),
    };
    let params: toml::Value = toml::toml! {
        configured_source_param = "chain-source-value"
    }
    .into();

    let plans = plan_profile_start(&profile(vec![source(
        "chain-source",
        IvSourceKind::OptionChain,
        "configured-client",
        selector.clone(),
        params.clone(),
        8,
    )]))
    .unwrap();

    assert_eq!(
        plans,
        vec![IvSubscriptionPlan {
            profile_id: profile_id(),
            source_id: "chain-source".to_string(),
            lifecycle: IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeOptionChain,
            nt_source_kind: IvNtSubscriptionKind::OptionChain,
            client_id: "configured-client".to_string(),
            selector,
            params,
            subscription_generation: 8,
        }]
    );
}

#[test]
fn aggregate_greeks_sources_plan_topic_subscribe_operations() {
    let selector = IvSelector::SourceAggregateGreeks {
        aggregate_key: "configured-aggregate-key".to_string(),
        underlying_selectors: vec!["configured-underlying-selector".to_string()],
        nt_params: toml::toml! {
            configured_nt_param = "aggregate-value"
        }
        .into(),
    };
    let params: toml::Value = toml::toml! {
        configured_source_param = "aggregate-source-value"
    }
    .into();

    let plans = plan_profile_start(&profile(vec![source(
        "aggregate-source",
        IvSourceKind::AggregateGreeks,
        "configured-client",
        selector.clone(),
        params.clone(),
        9,
    )]))
    .unwrap();

    assert_eq!(
        plans,
        vec![IvSubscriptionPlan {
            profile_id: profile_id(),
            source_id: "aggregate-source".to_string(),
            lifecycle: IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeAggregateGreeks,
            nt_source_kind: IvNtSubscriptionKind::AggregateGreeksTopic,
            client_id: "configured-client".to_string(),
            selector,
            params,
            subscription_generation: 9,
        }]
    );
}

#[test]
fn custom_implied_volatility_sources_plan_custom_data_subscribe_operations() {
    let selector = IvSelector::SourceCustomImpliedVolatility {
        custom_iv_data_type: "configured-custom-iv-data-type".to_string(),
        custom_iv_data_fields: vec!["configured-custom-iv-field".to_string()],
        nt_params: toml::toml! {
            configured_nt_param = "custom-value"
        }
        .into(),
    };
    let params: toml::Value = toml::toml! {
        configured_source_param = "custom-source-value"
    }
    .into();

    let plans = plan_profile_start(&profile(vec![source(
        "custom-source",
        IvSourceKind::CustomImpliedVolatility,
        "configured-client",
        selector.clone(),
        params.clone(),
        10,
    )]))
    .unwrap();

    assert_eq!(
        plans,
        vec![IvSubscriptionPlan {
            profile_id: profile_id(),
            source_id: "custom-source".to_string(),
            lifecycle: IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeCustomData,
            nt_source_kind: IvNtSubscriptionKind::CustomData,
            client_id: "configured-client".to_string(),
            selector,
            params,
            subscription_generation: 10,
        }]
    );
}

#[test]
fn reload_unsubscribes_old_generation_subscribes_new_generation_and_removes_deleted_sources() {
    let current_greeks = source(
        "greeks-source",
        IvSourceKind::OptionGreeks,
        "configured-client",
        IvSelector::SourceOptionGreeks {
            instrument_ids: vec!["configured-instrument-a".to_string()],
            nt_params: toml::toml! {
                configured_nt_param = "old-greeks-value"
            }
            .into(),
        },
        toml::toml! {
            configured_source_param = "old-greeks-source-value"
        }
        .into(),
        3,
    );
    let next_greeks = source(
        "greeks-source",
        IvSourceKind::OptionGreeks,
        "configured-client",
        IvSelector::SourceOptionGreeks {
            instrument_ids: vec!["configured-instrument-b".to_string()],
            nt_params: toml::toml! {
                configured_nt_param = "new-greeks-value"
            }
            .into(),
        },
        toml::toml! {
            configured_source_param = "new-greeks-source-value"
        }
        .into(),
        4,
    );
    let removed_chain = source(
        "removed-chain-source",
        IvSourceKind::OptionChain,
        "configured-client",
        IvSelector::SourceOptionChain {
            series_ids: vec!["configured-series-a".to_string()],
            strike_range_policy: "configured-strike-range-policy".to_string(),
            nt_params: toml::toml! {
                configured_nt_param = "removed-chain-value"
            }
            .into(),
        },
        toml::toml! {
            configured_source_param = "removed-chain-source-value"
        }
        .into(),
        5,
    );

    let plans = plan_profile_reload(
        &profile(vec![current_greeks.clone(), removed_chain.clone()]),
        &profile(vec![next_greeks.clone()]),
    )
    .unwrap();

    assert_eq!(
        plans,
        vec![
            IvSubscriptionPlan::from_source(
                &profile_id(),
                &current_greeks,
                IvSubscriptionLifecycle::Reload,
                IvRuntimeOperation::UnsubscribeOptionGreeks,
                IvNtSubscriptionKind::OptionGreeks,
            ),
            IvSubscriptionPlan::from_source(
                &profile_id(),
                &next_greeks,
                IvSubscriptionLifecycle::Reload,
                IvRuntimeOperation::SubscribeOptionGreeks,
                IvNtSubscriptionKind::OptionGreeks,
            ),
            IvSubscriptionPlan::from_source(
                &profile_id(),
                &removed_chain,
                IvSubscriptionLifecycle::SourceRemoval,
                IvRuntimeOperation::UnsubscribeOptionChain,
                IvNtSubscriptionKind::OptionChain,
            ),
            IvSubscriptionPlan::from_source(
                &profile_id(),
                &removed_chain,
                IvSubscriptionLifecycle::SourceRemoval,
                IvRuntimeOperation::RemoveSource,
                IvNtSubscriptionKind::OptionChain,
            ),
        ]
    );
}
