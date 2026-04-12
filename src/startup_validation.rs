use std::collections::BTreeSet;

use nautilus_model::instruments::Instrument;
use nautilus_polymarket::http::query::GetGammaEventsParams;

use crate::raw_capture_transport::build_gamma_instrument_client;
use crate::{clients::polymarket, config::Config};

type AppResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PolymarketDiscoveryMode {
    EventSlugs,
    TagSlugs,
}

impl PolymarketDiscoveryMode {
    fn noun(&self) -> &'static str {
        match self {
            Self::EventSlugs => "event slugs",
            Self::TagSlugs => "tag slugs",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct PolymarketStartupValidationTargets {
    discovery_mode: PolymarketDiscoveryMode,
    discovery_keys: Vec<String>,
    configured_instrument_ids: Vec<String>,
}

pub fn validate_polymarket_startup(cfg: &Config) -> AppResult {
    let Some(targets) = collect_polymarket_startup_validation_targets(cfg)? else {
        return Ok(());
    };

    let instrument_client = build_gamma_instrument_client(cfg.node.timeout_connection_secs)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let instruments = runtime.block_on(async {
        let mut instruments = Vec::new();
        for discovery_key in &targets.discovery_keys {
            let mut batch = match targets.discovery_mode {
                PolymarketDiscoveryMode::EventSlugs => instrument_client
                    .request_instruments_by_event_slugs(vec![discovery_key.clone()])
                    .await,
                PolymarketDiscoveryMode::TagSlugs => {
                    instrument_client
                        .request_instruments_by_event_params(GetGammaEventsParams {
                            tag_slug: Some(discovery_key.clone()),
                            ..Default::default()
                        })
                        .await
                }
            }
            .map_err(|error| {
                std::io::Error::other(format!(
                    "Polymarket startup validation failed while resolving {} [{}]: {error}",
                    targets.discovery_mode.noun(),
                    targets.discovery_keys.join(", ")
                ))
            })?;
            instruments.append(&mut batch);
        }
        Ok::<_, Box<dyn std::error::Error>>(instruments)
    })?;
    let discovered_instrument_ids: BTreeSet<String> = instruments
        .iter()
        .map(|instrument| instrument.id().to_string())
        .collect();

    validate_polymarket_startup_results(&targets, &discovered_instrument_ids)
}

fn collect_polymarket_startup_validation_targets(
    cfg: &Config,
) -> Result<Option<PolymarketStartupValidationTargets>, Box<dyn std::error::Error>> {
    let mut discovery_mode = PolymarketDiscoveryMode::EventSlugs;
    let mut discovery_keys = BTreeSet::new();

    let selector_tag_slugs = polymarket::polymarket_ruleset_tag_slugs(&cfg.rulesets)?;
    if !selector_tag_slugs.is_empty() {
        discovery_mode = PolymarketDiscoveryMode::TagSlugs;
        for tag_slug in selector_tag_slugs {
            discovery_keys.insert(tag_slug);
        }
    } else {
        for client in &cfg.data_clients {
            if client.kind != "polymarket" {
                continue;
            }

            let input: polymarket::PolymarketDataClientInput = client.config.clone().try_into()?;
            for event_slug in input.event_slugs {
                let trimmed = event_slug.trim();
                if !trimmed.is_empty() {
                    discovery_keys.insert(trimmed.to_string());
                }
            }
        }
    }

    let mut configured_instrument_ids = BTreeSet::new();
    for strategy in &cfg.strategies {
        let Some(instrument_id) = strategy
            .config
            .get("instrument_id")
            .and_then(toml::Value::as_str)
        else {
            continue;
        };

        if instrument_id.ends_with(".POLYMARKET") {
            configured_instrument_ids.insert(instrument_id.to_string());
        }
    }

    if discovery_keys.is_empty() {
        if configured_instrument_ids.is_empty() {
            return Ok(None);
        }

        return Err(std::io::Error::other(format!(
            "Polymarket startup validation cannot validate configured instrument_id(s) [{}] because no Polymarket {} are available for discovery",
            configured_instrument_ids
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            discovery_mode.noun(),
        ))
        .into());
    }

    Ok(Some(PolymarketStartupValidationTargets {
        discovery_mode,
        discovery_keys: discovery_keys.into_iter().collect(),
        configured_instrument_ids: configured_instrument_ids.into_iter().collect(),
    }))
}

fn validate_polymarket_startup_results(
    targets: &PolymarketStartupValidationTargets,
    discovered_instrument_ids: &BTreeSet<String>,
) -> AppResult {
    if discovered_instrument_ids.is_empty() {
        return Err(std::io::Error::other(format!(
            "Polymarket startup validation resolved zero instruments while querying {} [{}]",
            targets.discovery_mode.noun(),
            targets.discovery_keys.join(", ")
        ))
        .into());
    }

    let missing_instrument_ids: Vec<String> = targets
        .configured_instrument_ids
        .iter()
        .filter(|instrument_id| !discovered_instrument_ids.contains(*instrument_id))
        .cloned()
        .collect();

    if !missing_instrument_ids.is_empty() {
        return Err(std::io::Error::other(format!(
            "Polymarket startup validation could not find configured instrument_id(s) [{}] in Gamma-discovered instrument IDs while querying {} [{}]",
            missing_instrument_ids.join(", "),
            targets.discovery_mode.noun(),
            targets.discovery_keys.join(", ")
        ))
        .into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        DataClientEntry, ExecClientEntry, ExecClientSecrets, LoggingConfig, NodeConfig,
        RawCaptureConfig, ReferenceConfig, RulesetConfig, RulesetVenueKind, StrategyEntry,
        StreamingCaptureConfig,
    };

    #[test]
    fn startup_validation_collects_deduped_polymarket_targets() {
        let cfg = test_config(
            vec!["tag-b", "tag-a", "tag-b"],
            vec!["beta.POLYMARKET", "alpha.POLYMARKET", "beta.POLYMARKET"],
        );

        let targets = collect_polymarket_startup_validation_targets(&cfg)
            .expect("targets should collect")
            .expect("polymarket targets should exist");

        assert_eq!(targets.discovery_mode, PolymarketDiscoveryMode::TagSlugs);
        assert_eq!(targets.discovery_keys, vec!["tag-a", "tag-b"]);
        assert_eq!(
            targets.configured_instrument_ids,
            vec!["alpha.POLYMARKET", "beta.POLYMARKET"]
        );
    }

    #[test]
    fn startup_validation_collects_targets_from_multiple_polymarket_rulesets() {
        let mut cfg = test_config(vec!["tag-b"], vec!["alpha.POLYMARKET"]);
        cfg.rulesets.push(polymarket_ruleset("SECONDARY", "tag-c"));
        cfg.rulesets.push(polymarket_ruleset("TERTIARY", "tag-a"));

        let targets = collect_polymarket_startup_validation_targets(&cfg)
            .expect("targets should collect")
            .expect("polymarket targets should exist");

        assert_eq!(targets.discovery_mode, PolymarketDiscoveryMode::TagSlugs);
        assert_eq!(targets.discovery_keys, vec!["tag-a", "tag-b", "tag-c"]);
    }

    #[test]
    fn startup_validation_fails_closed_when_polymarket_strategy_has_no_legacy_event_slugs() {
        let mut cfg = test_config(
            vec!["bitcoin"],
            vec!["0xpresent-condition-present-token.POLYMARKET"],
        );
        cfg.rulesets.clear();

        let err = collect_polymarket_startup_validation_targets(&cfg)
            .expect_err("missing event slugs should fail closed")
            .to_string();

        assert!(err.contains("event slugs"), "{err}");
        assert!(
            err.contains("0xpresent-condition-present-token.POLYMARKET"),
            "{err}"
        );
    }

    #[test]
    fn startup_validation_skips_when_no_polymarket_inputs_exist() {
        let mut cfg = test_config(vec!["bitcoin"], vec![]);
        cfg.rulesets.clear();

        let targets = collect_polymarket_startup_validation_targets(&cfg)
            .expect("non-polymarket startup should not error");

        assert_eq!(targets, None);
    }

    #[test]
    fn startup_validation_rejects_whitespace_tag_slugs_without_polymarket_instruments() {
        let cfg = test_config(vec!["  ", "\t"], vec!["BTCUSDT.BINANCE"]);

        let err = collect_polymarket_startup_validation_targets(&cfg)
            .expect_err("whitespace tag slugs should fail selector parsing")
            .to_string();

        assert!(err.contains("must not contain whitespace"), "{err}");
    }

    #[test]
    fn startup_validation_rejects_whitespace_tag_slugs_with_polymarket_instruments() {
        let cfg = test_config(
            vec!["  ", "\t"],
            vec!["0xpresent-condition-present-token.POLYMARKET"],
        );

        let err = collect_polymarket_startup_validation_targets(&cfg)
            .expect_err("whitespace tag slugs should fail selector parsing")
            .to_string();

        assert!(err.contains("must not contain whitespace"), "{err}");
    }

    #[test]
    fn startup_validation_skips_when_only_non_polymarket_instruments_exist_without_selectors() {
        let mut cfg = test_config(vec!["bitcoin"], vec!["BTCUSDT.BINANCE"]);
        cfg.rulesets.clear();

        let targets = collect_polymarket_startup_validation_targets(&cfg)
            .expect("non-polymarket strategies should not require Polymarket selectors");

        assert_eq!(targets, None);
    }

    #[test]
    fn startup_validation_ignores_strategies_without_instrument_id_key() {
        let mut cfg = test_config(vec!["bitcoin"], vec!["alpha.POLYMARKET"]);
        cfg.strategies.push(StrategyEntry {
            kind: "exec_tester".to_string(),
            config: toml::toml! {
                strategy_id = "EXEC_TESTER-002"
                client_id = "POLYMARKET"
                order_qty = "5"
            }
            .into(),
        });

        let targets = collect_polymarket_startup_validation_targets(&cfg)
            .expect("targets should collect")
            .expect("polymarket targets should exist");

        assert_eq!(targets.configured_instrument_ids, vec!["alpha.POLYMARKET"]);
    }

    #[test]
    fn startup_validation_fails_when_zero_instruments_resolve() {
        let cfg = test_config(vec!["bitcoin"], vec![]);
        let targets = collect_polymarket_startup_validation_targets(&cfg)
            .expect("targets should collect")
            .expect("polymarket targets should exist");

        let err = validate_polymarket_startup_results(&targets, &BTreeSet::new())
            .expect_err("validation should fail")
            .to_string();

        assert!(err.contains("tag slugs [bitcoin]"), "{err}");
        assert!(err.contains("zero instruments"), "{err}");
    }

    #[test]
    fn startup_validation_accepts_matching_discovered_instrument_ids() {
        let cfg = test_config(
            vec!["bitcoin"],
            vec!["0xpresent-condition-present-token.POLYMARKET"],
        );
        let targets = collect_polymarket_startup_validation_targets(&cfg)
            .expect("targets should collect")
            .expect("polymarket targets should exist");

        validate_polymarket_startup_results(
            &targets,
            &BTreeSet::from([String::from("0xpresent-condition-present-token.POLYMARKET")]),
        )
        .expect("discovered instrument ids should satisfy startup validation");
    }

    #[test]
    fn startup_validation_fails_when_configured_instrument_id_is_missing() {
        let cfg = test_config(
            vec!["bitcoin"],
            vec![
                "0xmissing-condition-missing-token.POLYMARKET",
                "0xpresent-condition-present-token.POLYMARKET",
            ],
        );
        let targets = collect_polymarket_startup_validation_targets(&cfg)
            .expect("targets should collect")
            .expect("polymarket targets should exist");

        let err = validate_polymarket_startup_results(
            &targets,
            &BTreeSet::from([String::from("0xpresent-condition-present-token.POLYMARKET")]),
        )
        .expect_err("validation should fail")
        .to_string();

        assert!(
            err.contains("0xmissing-condition-missing-token.POLYMARKET"),
            "{err}"
        );
        assert!(err.contains("tag slugs [bitcoin]"), "{err}");
        assert!(err.contains("Gamma-discovered instrument IDs"), "{err}");
        assert!(!err.contains("token IDs"), "{err}");
    }

    #[test]
    fn startup_validation_accepts_matching_instrument_id() {
        let cfg = test_config(
            vec!["bitcoin"],
            vec![
                "0xpresent-condition-present-token.POLYMARKET",
                "0xalso-condition-also-present.POLYMARKET",
            ],
        );
        let targets = collect_polymarket_startup_validation_targets(&cfg)
            .expect("targets should collect")
            .expect("polymarket targets should exist");

        validate_polymarket_startup_results(
            &targets,
            &BTreeSet::from([
                String::from("0xalso-condition-also-present.POLYMARKET"),
                String::from("0xpresent-condition-present-token.POLYMARKET"),
            ]),
        )
        .expect("matching instrument ids should pass");
    }

    fn test_config(selector_tag_slugs: Vec<&str>, strategy_instrument_ids: Vec<&str>) -> Config {
        Config {
            node: NodeConfig {
                name: "BOLT-V2-001".to_string(),
                trader_id: "BOLT-001".to_string(),
                environment: "Live".to_string(),
                load_state: false,
                save_state: false,
                timeout_connection_secs: 60,
                timeout_reconciliation_secs: 30,
                timeout_portfolio_secs: 10,
                timeout_disconnection_secs: 10,
                delay_post_stop_secs: 10,
                delay_shutdown_secs: 5,
            },
            logging: LoggingConfig {
                stdout_level: "Info".to_string(),
                file_level: "Debug".to_string(),
            },
            data_clients: vec![DataClientEntry {
                name: "POLYMARKET".to_string(),
                kind: "polymarket".to_string(),
                config: toml::toml! {
                    subscribe_new_markets = false
                    update_instruments_interval_mins = 60
                    ws_max_subscriptions = 200
                }
                .into(),
            }],
            exec_clients: vec![ExecClientEntry {
                name: "POLYMARKET".to_string(),
                kind: "polymarket".to_string(),
                config: toml::toml! {
                    account_id = "POLYMARKET-001"
                    signature_type = 2
                    funder = "0xabc"
                }
                .into(),
                secrets: ExecClientSecrets {
                    region: "eu-west-1".to_string(),
                    pk: Some("/x/pk".to_string()),
                    api_key: Some("/x/key".to_string()),
                    api_secret: Some("/x/secret".to_string()),
                    passphrase: Some("/x/pass".to_string()),
                },
            }],
            strategies: strategy_instrument_ids
                .into_iter()
                .map(|instrument_id| StrategyEntry {
                    kind: "exec_tester".to_string(),
                    config: toml::toml! {
                        strategy_id = "EXEC_TESTER-001"
                        instrument_id = instrument_id
                        client_id = "POLYMARKET"
                        order_qty = "5"
                    }
                    .into(),
                })
                .collect(),
            raw_capture: RawCaptureConfig::default(),
            streaming: StreamingCaptureConfig::default(),
            reference: ReferenceConfig::default(),
            rulesets: selector_tag_slugs
                .into_iter()
                .enumerate()
                .map(|(index, tag_slug)| polymarket_ruleset(&format!("PRIMARY-{index}"), tag_slug))
                .collect(),
            audit: None,
        }
    }

    fn polymarket_ruleset(id: &str, tag_slug: &str) -> RulesetConfig {
        RulesetConfig {
            id: id.to_string(),
            venue: RulesetVenueKind::Polymarket,
            selector: toml::toml! {
                tag_slug = tag_slug
            }
            .into(),
            resolution_basis: "binance_btcusdt_1m".to_string(),
            min_time_to_expiry_secs: 60,
            max_time_to_expiry_secs: 900,
            min_liquidity_num: 1000.0,
            require_accepting_orders: true,
            freeze_before_end_secs: 90,
            selector_poll_interval_ms: 1_000,
            candidate_load_timeout_secs: 30,
        }
    }
}
