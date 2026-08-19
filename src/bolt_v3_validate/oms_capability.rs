use nautilus_model::enums::OmsType;

use crate::{
    bolt_v3_config::{BoltV3RootConfig, LoadedStrategy},
    bolt_v3_providers,
};

pub(super) fn validate_oms_venue_position_identity_capabilities(
    root: &BoltV3RootConfig,
    strategies: &[LoadedStrategy],
) -> Vec<String> {
    validate_with_capability_lookup(root, strategies, |provider_key| {
        bolt_v3_providers::binding_for_provider_key(provider_key)
            .and_then(|binding| binding.venue_position_identity)
    })
}

fn validate_with_capability_lookup(
    root: &BoltV3RootConfig,
    strategies: &[LoadedStrategy],
    capability_for_provider: impl Fn(&str) -> Option<bool>,
) -> Vec<String> {
    if !strategies
        .iter()
        .any(|strategy| strategy.config.oms_type == OmsType::Hedging)
    {
        return Vec::new();
    }

    root.clients
        .iter()
        .filter(|(_, client)| client.execution.is_some())
        .filter_map(|(client_key, client)| {
            let capability = capability_for_provider(client.venue.as_str());
            match capability {
                Some(true) => None,
                Some(false) => Some(format!(
                    "clients.{client_key}.execution declares capability `venue_position_identity=not_reported`, which cannot support oms_type=Hedging"
                )),
                None => Some(format!(
                    "clients.{client_key}.execution has no declared `venue_position_identity` capability for oms_type=Hedging"
                )),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use nautilus_model::{identifiers::ClientId, identifiers::Venue};

    use super::*;
    use crate::bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, ClientBlock};

    fn execution_client(venue: &str) -> ClientBlock {
        ClientBlock {
            venue: Venue::from(venue),
            data: None,
            execution: Some(toml::Value::Table(toml::map::Map::new())),
            secrets: None,
            readiness_probe: None,
        }
    }

    #[test]
    fn hedging_rejects_an_incompatible_unselected_execution_client() {
        let mut root: BoltV3RootConfig =
            toml::from_str(include_str!("../../tests/fixtures/bolt_v3/root.toml"))
                .expect("root fixture should parse");
        root.clients = BTreeMap::from([
            (
                "compatible_selected".to_string(),
                execution_client("CAPABILITY_REPORTED"),
            ),
            (
                "incompatible_unselected".to_string(),
                execution_client("CAPABILITY_NOT_REPORTED"),
            ),
        ]);

        let mut strategy: BoltV3StrategyConfig = toml::from_str(include_str!(
            "../../tests/fixtures/bolt_v3/strategies/binary_oracle.toml"
        ))
        .expect("strategy fixture should parse");
        strategy.oms_type = OmsType::Hedging;
        strategy.execution_client_id = ClientId::from("compatible_selected");
        let strategies = [LoadedStrategy {
            config_path: "strategies/binary_oracle.toml".into(),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];

        let errors =
            validate_with_capability_lookup(
                &root,
                &strategies,
                |provider_key| match provider_key {
                    "CAPABILITY_REPORTED" => Some(true),
                    "CAPABILITY_NOT_REPORTED" => Some(false),
                    _ => None,
                },
            );

        assert_eq!(errors.len(), 1, "only the incompatible client should fail");
        assert!(errors[0].contains("clients.incompatible_unselected.execution"));
        assert!(errors[0].contains("venue_position_identity=not_reported"));
        assert!(!errors[0].contains("compatible_selected"));
    }
}
