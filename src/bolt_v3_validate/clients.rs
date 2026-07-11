use super::*;

pub(super) fn validate_aws_block(block: &AwsBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if block.region.trim().is_empty() {
        errors.push("aws.region must be a non-empty string".to_string());
    }
    errors
}

pub(super) fn validate_clients_block(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let clients = &root.clients;
    if clients.is_empty() {
        errors.push("clients must define at least one client block".to_string());
        return errors;
    }
    for (key, client) in clients {
        errors.extend(validate_root_owned_chainlink_feed_catalog(
            root, key, client,
        ));
        let validation_client = client_with_root_chainlink_feed_catalog(root, client);
        let client = validation_client.as_ref().unwrap_or(client);
        errors.extend(crate::bolt_v3_providers::validate_client_block(key, client));
        errors.extend(validate_reference_reconnect_timeout_exceeds_startup_bound(
            root, key, client,
        ));
        errors.extend(validate_client_readiness_probe(key, client));
    }
    errors.extend(validate_unique_client_readiness_probe_instruments(clients));
    errors
}

#[derive(Debug)]
enum ReferenceReconnectStartupBudgetValidationError {
    BudgetResolution(crate::bolt_v3_providers::NtReconnectBudgetResolutionError),
    NautilusStartupBoundOverflow(crate::bolt_v3_config::NautilusStartupBoundOverflow),
    NautilusStartupBoundMillisecondsOverflow {
        startup_bound_secs: u64,
    },
    ReferenceReconnectTimeoutNotAboveStartupBound {
        client_key: String,
        provider_key: &'static str,
        reconnect_timeout_ms: u64,
        startup_bound_ms: u64,
    },
}

impl std::fmt::Display for ReferenceReconnectStartupBudgetValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BudgetResolution(error) => write!(f, "{error}"),
            Self::NautilusStartupBoundOverflow(error) => {
                write!(f, "error_variant=NautilusStartupBoundOverflow {error}")
            }
            Self::NautilusStartupBoundMillisecondsOverflow { startup_bound_secs } => write!(
                f,
                "error_variant=NautilusStartupBoundMillisecondsOverflow nautilus startup bound does not fit reconnect_timeout_ms; startup_bound_secs={startup_bound_secs}"
            ),
            Self::ReferenceReconnectTimeoutNotAboveStartupBound {
                client_key,
                provider_key,
                reconnect_timeout_ms,
                startup_bound_ms,
            } => write!(
                f,
                "error_variant=ReferenceReconnectTimeoutNotAboveStartupBound clients.{client_key}.data.reconnect_timeout_ms must be greater than nautilus startup bound because NT uses reconnect_timeout as the initial-connect budget; provider={provider_key} reconnect_timeout_ms={reconnect_timeout_ms} startup_bound_ms={startup_bound_ms}"
            ),
        }
    }
}

fn validate_reference_reconnect_timeout_exceeds_startup_bound(
    root: &BoltV3RootConfig,
    key: &str,
    client: &ClientBlock,
) -> Vec<String> {
    let (provider_key, reconnect_timeout_ms) = match crate::bolt_v3_providers::nt_reconnect_budget(
        client.venue.as_str(),
        client.data.as_ref(),
    ) {
        Ok(crate::bolt_v3_providers::NtReconnectBudget::NotApplicable) => return Vec::new(),
        Ok(crate::bolt_v3_providers::NtReconnectBudget::Required {
            provider_key,
            reconnect_timeout_ms,
        }) => (provider_key, reconnect_timeout_ms),
        Err(error) => {
            return vec![
                ReferenceReconnectStartupBudgetValidationError::BudgetResolution(error).to_string(),
            ];
        }
    };
    let startup_bound_secs =
        match crate::bolt_v3_config::nautilus_startup_bound_secs(&root.nautilus) {
            Ok(startup_bound_secs) => startup_bound_secs,
            Err(error) => {
                return vec![
                    ReferenceReconnectStartupBudgetValidationError::NautilusStartupBoundOverflow(
                        error,
                    )
                    .to_string(),
                ];
            }
        };
    let startup_bound_ms = match u64::try_from(
        std::time::Duration::from_secs(startup_bound_secs).as_millis(),
    ) {
        Ok(startup_bound_ms) => startup_bound_ms,
        Err(_) => {
            return vec![
                    ReferenceReconnectStartupBudgetValidationError::NautilusStartupBoundMillisecondsOverflow {
                        startup_bound_secs,
                    }
                    .to_string(),
                ];
        }
    };

    if reconnect_timeout_ms > startup_bound_ms {
        return Vec::new();
    }

    vec![
        ReferenceReconnectStartupBudgetValidationError::ReferenceReconnectTimeoutNotAboveStartupBound {
            client_key: key.to_string(),
            provider_key,
            reconnect_timeout_ms,
            startup_bound_ms,
        }
        .to_string(),
    ]
}

fn validate_client_readiness_probe(key: &str, client: &ClientBlock) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(readiness_probe) = &client.readiness_probe else {
        return errors;
    };
    if client.data.is_none() {
        errors.push(format!(
            "clients.{key}.readiness_probe requires the same client to declare a [data] block"
        ));
    }
    // A trade chunk-count probe walks the venue's full instrument universe in
    // chunks until `m` distinct markets trade; it has no fixed sample, so it
    // owns a distinct config surface (chunk_size + window, no sampling knobs).
    let is_trade_volume_probe = matches!(
        readiness_probe.market_data_kind,
        crate::bolt_v3_config::DataClientReadinessProbeMarketDataKind::Trade
    ) && matches!(
        readiness_probe.quote_target_source,
        DataClientReadinessProbeQuoteTargetSource::MetadataResponse
    );
    match readiness_probe.market_data_kind {
        crate::bolt_v3_config::DataClientReadinessProbeMarketDataKind::Quote => {
            if readiness_probe.book_type.is_some() {
                errors.push(format!(
                    "clients.{key}.readiness_probe.book_type is only valid when market_data_kind = \"book\""
                ));
            }
        }
        crate::bolt_v3_config::DataClientReadinessProbeMarketDataKind::Book => {
            if readiness_probe.book_type.is_none() {
                errors.push(format!(
                    "clients.{key}.readiness_probe.book_type must be configured when market_data_kind = \"book\""
                ));
            }
        }
        crate::bolt_v3_config::DataClientReadinessProbeMarketDataKind::Trade => {
            if readiness_probe.book_type.is_some() {
                errors.push(format!(
                    "clients.{key}.readiness_probe.book_type is only valid when market_data_kind = \"book\""
                ));
            }
        }
    }
    match readiness_probe.quote_target_source {
        DataClientReadinessProbeQuoteTargetSource::Configured => {
            if readiness_probe
                .quote_targets
                .as_ref()
                .is_none_or(|quote_targets| quote_targets.is_empty())
            {
                errors.push(format!(
                    "clients.{key}.readiness_probe.quote_targets must define at least one configured quote target when quote_target_source = \"configured\""
                ));
            }
            if readiness_probe.max_metadata_quote_targets.is_some() {
                errors.push(format!(
                    "clients.{key}.readiness_probe.max_metadata_quote_targets is only valid when quote_target_source = \"metadata_response\""
                ));
            }
            if readiness_probe.allow_metadata_target_sampling.is_some() {
                errors.push(format!(
                    "clients.{key}.readiness_probe.allow_metadata_target_sampling is only valid when quote_target_source = \"metadata_response\""
                ));
            }
        }
        DataClientReadinessProbeQuoteTargetSource::MetadataResponse => {
            if readiness_probe.quote_targets.is_some() {
                errors.push(format!(
                    "clients.{key}.readiness_probe cannot combine quote_target_source = \"metadata_response\" with readiness_probe.quote_targets"
                ));
            }
            if is_trade_volume_probe {
                // Chunk-count mode subscribes the whole universe in chunks of
                // chunk_size until `m` (min_observed_targets) distinct markets
                // trade. There is no fixed sample, so the sampling knobs are
                // rejected and the chunk knobs are required instead.
                if readiness_probe.max_metadata_quote_targets.is_some() {
                    errors.push(format!(
                        "clients.{key}.readiness_probe.max_metadata_quote_targets is not valid for a trade chunk-count probe; configure chunk_size instead"
                    ));
                }
                if readiness_probe.allow_metadata_target_sampling.is_some() {
                    errors.push(format!(
                        "clients.{key}.readiness_probe.allow_metadata_target_sampling is not valid for a trade chunk-count probe"
                    ));
                }
                match readiness_probe.chunk_size {
                    Some(chunk_size) if chunk_size > 0 => {}
                    _ => {
                        errors.push(format!(
                            "clients.{key}.readiness_probe.chunk_size must be a positive integer when market_data_kind = \"trade\" and quote_target_source = \"metadata_response\""
                        ));
                    }
                };
                match readiness_probe.chunk_observation_window_seconds {
                    Some(window) if window > 0 => {}
                    _ => {
                        errors.push(format!(
                            "clients.{key}.readiness_probe.chunk_observation_window_seconds must be a positive integer when market_data_kind = \"trade\" and quote_target_source = \"metadata_response\""
                        ));
                    }
                };
                match readiness_probe.min_observed_targets {
                    Some(min_observed_targets) if min_observed_targets > 0 => {}
                    _ => {
                        errors.push(format!(
                            "clients.{key}.readiness_probe.min_observed_targets must be a positive integer when market_data_kind = \"trade\" and quote_target_source = \"metadata_response\""
                        ));
                    }
                };
            } else {
                match readiness_probe.max_metadata_quote_targets {
                    Some(max_metadata_quote_targets) if max_metadata_quote_targets > 0 => {}
                    _ => {
                        errors.push(format!(
                            "clients.{key}.readiness_probe.max_metadata_quote_targets must be a positive integer when quote_target_source = \"metadata_response\""
                        ));
                    }
                };
                if readiness_probe.allow_metadata_target_sampling.is_none() {
                    errors.push(format!(
                        "clients.{key}.readiness_probe.allow_metadata_target_sampling must be explicitly configured when quote_target_source = \"metadata_response\""
                    ));
                }
            }
        }
    }
    if !is_trade_volume_probe {
        if readiness_probe.chunk_size.is_some() {
            errors.push(format!(
                "clients.{key}.readiness_probe.chunk_size is only valid when market_data_kind = \"trade\" and quote_target_source = \"metadata_response\""
            ));
        }
        if readiness_probe.chunk_observation_window_seconds.is_some() {
            errors.push(format!(
                "clients.{key}.readiness_probe.chunk_observation_window_seconds is only valid when market_data_kind = \"trade\" and quote_target_source = \"metadata_response\""
            ));
        }
    }
    if let Some(quote_targets) = &readiness_probe.quote_targets {
        for (target_id, target) in quote_targets {
            if target_id.trim().is_empty() || target_id.trim() != target_id {
                errors.push(format!(
                    "clients.{key}.readiness_probe.quote_targets target id must be non-empty without surrounding whitespace"
                ));
            }
            if target.instrument_id.venue.as_str() != client.venue.as_str() {
                errors.push(format!(
                    "clients.{key}.readiness_probe.quote_targets.{target_id}.instrument_id venue `{}` must match clients.{key}.venue `{}`",
                    target.instrument_id.venue,
                    client.venue
                ));
            }
        }
    }
    errors
}

fn validate_unique_client_readiness_probe_instruments(
    clients: &BTreeMap<String, ClientBlock>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let mut by_instrument: BTreeMap<String, (&str, &str)> = BTreeMap::new();
    for (client_key, client) in clients {
        let Some(readiness_probe) = &client.readiness_probe else {
            continue;
        };
        if let Some(quote_targets) = &readiness_probe.quote_targets {
            for (target_id, target) in quote_targets {
                let instrument_id = target.instrument_id.to_string();
                match by_instrument.get(instrument_id.as_str()) {
                    Some((existing_client_key, existing_target_id))
                        if existing_client_key != client_key =>
                    {
                        errors.push(format!(
                            "clients.{client_key}.readiness_probe.quote_targets.{target_id}.instrument_id `{instrument_id}` is also used by clients.{existing_client_key}.readiness_probe.quote_targets.{existing_target_id}.instrument_id; QuoteTick does not carry data_client_id, so strategy-free data-client quote probe evidence cannot distinguish data clients for the same instrument"
                        ));
                    }
                    None => {
                        by_instrument
                            .insert(instrument_id, (client_key.as_str(), target_id.as_str()));
                    }
                    _ => {}
                }
            }
        }
    }
    errors
}

/// Provider-neutral SSM parameter-path utility shared by the per-
/// provider secret validators in `crate::bolt_v3_providers`. Stays in
/// core because the path-shape rule itself is provider-neutral and is
/// also the gate behind the SSM-only invariant; mirrors the cross-
/// layer call that the archetype binding makes into
/// `parse_decimal_string`.
pub(crate) fn validate_ssm_parameter_path(key: &str, field: &str, value: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        errors.push(format!(
            "clients.{key}.secrets.{field} must be a non-empty SSM path"
        ));
    } else {
        if trimmed != value {
            errors.push(format!(
                "clients.{key}.secrets.{field} must not have leading or trailing whitespace"
            ));
        }
        if !trimmed.starts_with('/') {
            // The Rust AWS SDK accepts both `name`-style and `/name`-style
            // parameter references, but bolt-v3 standardizes on
            // absolute-style hierarchical paths so an SSM resource layout
            // like `/bolt/<venue>/<field>` is the only supported shape and
            // typos that drop the leading slash fail closed at startup.
            errors.push(format!(
            "clients.{key}.secrets.{field} must be an absolute-style SSM parameter path starting with `/`: `{value}`"
        ));
        }
    }
    errors
}
