use super::*;

pub(super) fn validate_reference_current_price(
    context: &str,
    root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
) -> Vec<String> {
    let Some(reference_current_price) = &strategy.reference_current_price else {
        return Vec::new();
    };

    let mut errors = Vec::new();
    let configured: BTreeSet<&str> = reference_current_price
        .source_order
        .iter()
        .map(String::as_str)
        .collect();
    let declared: BTreeSet<&str> = reference_current_price
        .sources
        .keys()
        .map(String::as_str)
        .collect();

    if reference_current_price.asset.is_empty()
        || !reference_current_price
            .asset
            .chars()
            .all(|char| char.is_ascii_uppercase() || char.is_ascii_digit() || char == '_')
    {
        errors.push(format!(
            "{context}: reference_current_price.asset must be a normalized non-empty uppercase ASCII asset symbol containing only letters, digits, and underscores"
        ));
    }
    if let Ok(target) =
        crate::bolt_v3_market_families::target_runtime_fields_from_target(&strategy.target)
        && reference_current_price.asset != target.underlying_asset
    {
        errors.push(format!(
            "{context}: reference_current_price.asset `{}` must match target.underlying_asset `{}`",
            reference_current_price.asset, target.underlying_asset,
        ));
    }

    if reference_current_price.source_order.is_empty() {
        errors.push(format!(
            "{context}: reference_current_price.sources must be non-empty"
        ));
    }

    let mut seen_sources = HashSet::new();
    for source_id in &reference_current_price.source_order {
        if !seen_sources.insert(source_id.as_str()) {
            errors.push(format!(
                "{context}: reference_current_price.sources contains duplicate source key `{source_id}`"
            ));
        }
    }

    if reference_current_price.min_valid_sources == 0 {
        errors.push(format!(
            "{context}: reference_current_price.min_valid_sources must be at least 1"
        ));
    }

    let enabled_source_count = reference_current_price
        .source_order
        .iter()
        .filter(|source_id| {
            reference_current_price
                .sources
                .get(source_id.as_str())
                .is_some_and(|source| source.enabled)
        })
        .count();
    if reference_current_price.min_valid_sources > enabled_source_count {
        errors.push(format!(
            "{context}: reference_current_price.min_valid_sources {} exceeds enabled source count {}",
            reference_current_price.min_valid_sources, enabled_source_count
        ));
    }

    if reference_current_price.max_source_age_ms == 0 {
        errors.push(format!(
            "{context}: reference_current_price.max_source_age_ms must be positive"
        ));
    }

    if reference_current_price.max_source_drift_bps == 0 {
        errors.push(format!(
            "{context}: reference_current_price.max_source_drift_bps must be positive"
        ));
    }

    for source_id in configured.difference(&declared) {
        errors.push(format!(
            "{context}: reference_current_price.sources contains `{source_id}` but missing [reference_current_price.source.{source_id}]"
        ));
    }

    for source_id in declared.difference(&configured) {
        errors.push(format!(
            "{context}: [reference_current_price.source.{source_id}] is declared but not listed in reference_current_price.sources"
        ));
    }

    let mut valid_enabled_sources = enabled_source_count;
    let mut physical_source_keys: BTreeMap<(String, String, String), &str> = BTreeMap::new();
    for source_id in &reference_current_price.source_order {
        let Some(source) = reference_current_price.sources.get(source_id.as_str()) else {
            continue;
        };
        if !source.enabled {
            continue;
        }
        let Some(provider_metadata) = reference_price_provider_metadata(source.provider.as_str())
        else {
            continue;
        };
        let identifier = match provider_metadata.identifier_kind {
            ReferencePriceIdentifierKind::InstrumentId => source.instrument_id.as_deref(),
            ReferencePriceIdentifierKind::Symbol => source.symbol.as_deref(),
        };
        let Some(identifier) = identifier.filter(|value| !reference_price_field_is_blank(value))
        else {
            continue;
        };
        let key = (
            source.provider.as_str().to_string(),
            source.client_id.to_string(),
            identifier.to_string(),
        );
        if let Some(existing_source_id) = physical_source_keys.insert(key, source_id.as_str()) {
            errors.push(format!(
                "{context}: reference_current_price.source.{source_id} uses the same physical reference feed as reference_current_price.source.{existing_source_id}: provider `{}`, client_id `{}`, identifier `{identifier}`",
                source.provider.as_str(),
                source.client_id,
            ));
        }
    }

    for (source_id, source) in &reference_current_price.sources {
        let provider_metadata = reference_price_provider_metadata(source.provider.as_str());
        match root.clients.get(source.client_id.as_str()) {
            None => errors.push(format!(
                "{context}: reference_current_price.source.{source_id}.client_id `{}` does not match any [clients.<id>] block",
                source.client_id
            )),
            Some(client) => {
                if let Some(provider_metadata) = provider_metadata
                    && client.venue.as_str() != provider_metadata.client_venue_key
                {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.client_id `{}` must reference a {} client for provider `{}`; got `{}`",
                        source.client_id,
                        provider_metadata.client_venue_key,
                        provider_metadata.provider_key,
                        client.venue.as_str()
                    ));
                }
                if client.data.is_none() {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.client_id `{}` must reference a data-capable client",
                        source.client_id
                    ));
                }
            }
        }
        if source.required && !source.enabled {
            errors.push(format!(
                "{context}: reference_current_price.source.{source_id} is required but disabled"
            ));
        }

        let Some(provider_metadata) = provider_metadata else {
            errors.push(format!(
                "{context}: reference_current_price.source.{source_id}.provider `{}` is unsupported",
                source.provider.as_str()
            ));
            continue;
        };

        match provider_metadata.identifier_kind {
            ReferencePriceIdentifierKind::InstrumentId => {
                let provider_key = source.provider.as_str();
                if source
                    .instrument_id
                    .as_deref()
                    .is_none_or(reference_price_field_is_blank)
                {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.instrument_id is required for provider `{provider_key}`"
                    ));
                }
                if source.symbol.is_some() {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.symbol is unsupported for provider `{provider_key}`"
                    ));
                }
                if let Some(instrument_id) = source.instrument_id.as_deref()
                    && !reference_price_identifier_matches_asset(
                        instrument_id,
                        &reference_current_price.asset,
                    )
                {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.instrument_id `{instrument_id}` must map to reference_current_price.asset `{}`",
                        reference_current_price.asset
                    ));
                }
                if let Some(instrument_id) = source.instrument_id.as_deref() {
                    match reference_price_provider_identifier_is_configured(
                        root,
                        source.provider.as_str(),
                        instrument_id,
                    ) {
                        Ok(true) => {}
                        Ok(false) => errors.push(format!(
                            "{context}: reference_current_price.source.{source_id}.instrument_id `{instrument_id}` is not present in provider catalog for provider `{provider_key}`"
                        )),
                        Err(message) => errors.push(format!(
                            "{context}: reference_current_price.source.{source_id}.instrument_id `{instrument_id}` could not be checked against provider catalog: {message}"
                        )),
                    }
                }
            }
            ReferencePriceIdentifierKind::Symbol => {
                let provider_key = source.provider.as_str();
                if source
                    .symbol
                    .as_deref()
                    .is_none_or(reference_price_field_is_blank)
                {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.symbol is required for provider `{provider_key}`"
                    ));
                }
                if source.instrument_id.is_some() {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.instrument_id is unsupported for provider `{provider_key}`"
                    ));
                }
                if let Some(symbol) = source.symbol.as_deref()
                    && !reference_price_identifier_matches_asset(
                        symbol,
                        &reference_current_price.asset,
                    )
                {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.symbol `{symbol}` must map to reference_current_price.asset `{}`",
                        reference_current_price.asset
                    ));
                }
            }
        }

        let unsupported_asset = source.enabled
            && reference_price_source_is_unsupported(reference_current_price, source);
        if unsupported_asset && configured.contains(source_id.as_str()) {
            valid_enabled_sources = valid_enabled_sources.saturating_sub(1);
        }
        if unsupported_asset && (source.required || !configured.contains(source_id.as_str())) {
            errors.push(format!(
                "{context}: reference_current_price.source.{source_id} {} asset `{}` is unsupported",
                source.provider.as_str(),
                reference_current_price.asset
            ));
        }
    }

    if reference_current_price.min_valid_sources > valid_enabled_sources {
        errors.push(format!(
            "{context}: reference_current_price.min_valid_sources {} cannot be met by {} enabled supported source(s)",
            reference_current_price.min_valid_sources, valid_enabled_sources
        ));
    }

    errors
}

fn reference_price_field_is_blank(value: &str) -> bool {
    value.trim().is_empty() || value.trim() != value
}

fn reference_price_identifier_matches_asset(identifier: &str, asset: &str) -> bool {
    identifier
        .split(['-', '.', '/'])
        .next()
        .is_some_and(|prefix| prefix == asset)
}
