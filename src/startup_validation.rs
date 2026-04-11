use std::collections::BTreeSet;

use crate::raw_capture_transport::{
    build_gamma_instrument_client, market_token_ids_from_instruments,
};
use crate::{clients::polymarket, config::Config};

type AppResult = Result<(), Box<dyn std::error::Error>>;

const POLYMARKET_VENUE_SUFFIX: &str = ".POLYMARKET";

#[derive(Debug, PartialEq, Eq)]
struct PolymarketStartupValidationTargets {
    event_slugs: Vec<String>,
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
    let instruments = runtime
        .block_on(async {
            instrument_client
                .request_instruments_by_event_slugs(targets.event_slugs.clone())
                .await
        })
        .map_err(|error| {
            std::io::Error::other(format!(
                "Polymarket startup validation failed while resolving event slugs [{}]: {error}",
                targets.event_slugs.join(", ")
            ))
        })?;
    let discovered_token_ids: BTreeSet<String> = market_token_ids_from_instruments(&instruments)
        .into_iter()
        .collect();

    validate_polymarket_startup_results(&targets, &discovered_token_ids)
}

fn collect_polymarket_startup_validation_targets(
    cfg: &Config,
) -> Result<Option<PolymarketStartupValidationTargets>, Box<dyn std::error::Error>> {
    let mut event_slugs = BTreeSet::new();
    for client in &cfg.data_clients {
        if client.kind != "polymarket" {
            continue;
        }

        let input: polymarket::PolymarketDataClientInput = client.config.clone().try_into()?;
        for event_slug in input.event_slugs {
            let trimmed = event_slug.trim();
            if !trimmed.is_empty() {
                event_slugs.insert(trimmed.to_string());
            }
        }
    }

    if event_slugs.is_empty() {
        return Ok(None);
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

        if instrument_id.ends_with(POLYMARKET_VENUE_SUFFIX) {
            configured_instrument_ids.insert(instrument_id.to_string());
        }
    }

    Ok(Some(PolymarketStartupValidationTargets {
        event_slugs: event_slugs.into_iter().collect(),
        configured_instrument_ids: configured_instrument_ids.into_iter().collect(),
    }))
}

fn validate_polymarket_startup_results(
    targets: &PolymarketStartupValidationTargets,
    discovered_token_ids: &BTreeSet<String>,
) -> AppResult {
    if discovered_token_ids.is_empty() {
        return Err(std::io::Error::other(format!(
            "Polymarket startup validation resolved zero instruments for event slugs [{}]",
            targets.event_slugs.join(", ")
        ))
        .into());
    }

    let missing_instrument_ids: Vec<String> = targets
        .configured_instrument_ids
        .iter()
        .filter(|instrument_id| {
            let token_id = instrument_id
                .strip_suffix(POLYMARKET_VENUE_SUFFIX)
                .unwrap_or(instrument_id.as_str());
            !discovered_token_ids.contains(token_id)
        })
        .cloned()
        .collect();

    if !missing_instrument_ids.is_empty() {
        return Err(std::io::Error::other(format!(
            "Polymarket startup validation could not find configured instrument_id(s) [{}] in Gamma-discovered token IDs for event slugs [{}]",
            missing_instrument_ids.join(", "),
            targets.event_slugs.join(", ")
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
        RawCaptureConfig, ReferenceConfig, StrategyEntry, StreamingCaptureConfig,
    };

    #[test]
    fn startup_validation_collects_deduped_polymarket_targets() {
        let cfg = test_config(
            vec!["slug-b", "slug-a", "slug-b"],
            vec!["beta.POLYMARKET", "alpha.POLYMARKET", "beta.POLYMARKET"],
        );

        let targets = collect_polymarket_startup_validation_targets(&cfg)
            .expect("targets should collect")
            .expect("polymarket targets should exist");

        assert_eq!(targets.event_slugs, vec!["slug-a", "slug-b"]);
        assert_eq!(
            targets.configured_instrument_ids,
            vec!["alpha.POLYMARKET", "beta.POLYMARKET"]
        );
    }

    #[test]
    fn startup_validation_fails_when_zero_instruments_resolve() {
        let cfg = test_config(vec!["stale-market"], vec![]);
        let targets = collect_polymarket_startup_validation_targets(&cfg)
            .expect("targets should collect")
            .expect("polymarket targets should exist");

        let err = validate_polymarket_startup_results(&targets, &BTreeSet::new())
            .expect_err("validation should fail")
            .to_string();

        assert!(err.contains("stale-market"), "{err}");
        assert!(err.contains("zero instruments"), "{err}");
    }

    #[test]
    fn startup_validation_fails_when_configured_instrument_id_is_missing() {
        let cfg = test_config(
            vec!["btc-updown-5m"],
            vec!["missing-token.POLYMARKET", "present-token.POLYMARKET"],
        );
        let targets = collect_polymarket_startup_validation_targets(&cfg)
            .expect("targets should collect")
            .expect("polymarket targets should exist");

        let err = validate_polymarket_startup_results(
            &targets,
            &BTreeSet::from([String::from("present-token")]),
        )
        .expect_err("validation should fail")
        .to_string();

        assert!(err.contains("missing-token.POLYMARKET"), "{err}");
        assert!(err.contains("btc-updown-5m"), "{err}");
    }

    #[test]
    fn startup_validation_accepts_matching_instrument_id() {
        let cfg = test_config(
            vec!["btc-updown-5m"],
            vec!["present-token.POLYMARKET", "also-present.POLYMARKET"],
        );
        let targets = collect_polymarket_startup_validation_targets(&cfg)
            .expect("targets should collect")
            .expect("polymarket targets should exist");

        validate_polymarket_startup_results(
            &targets,
            &BTreeSet::from([String::from("also-present"), String::from("present-token")]),
        )
        .expect("matching instrument ids should pass");
    }

    fn test_config(event_slugs: Vec<&str>, strategy_instrument_ids: Vec<&str>) -> Config {
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
                    event_slugs = event_slugs
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
            rulesets: Vec::new(),
            audit: None,
        }
    }
}
