use super::*;

pub(crate) fn validate_iv_source_clients(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(iv) = root.iv.as_ref() else {
        return errors;
    };

    for profile in &iv.profiles {
        for source in &profile.sources {
            let context = format!(
                "iv.profiles.{}.sources.{}",
                profile.profile_id, source.source_id
            );
            match root.clients.get(source.client_id.as_str()) {
                None => errors.push(format!(
                    "{context}.client_id `{}` does not match any [clients.<id>] block",
                    source.client_id
                )),
                Some(client) if client.data.is_none() => errors.push(format!(
                    "{context}.client_id `{}` must reference a data-capable client (the referenced client has no [data] block)",
                    source.client_id
                )),
                Some(_) => {}
            }
        }
    }

    errors
}

pub(crate) fn validate_realized_volatility_source_clients(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(realized_volatility_surfaces) = root.realized_volatility_surfaces.as_ref() else {
        return errors;
    };

    for (surface_id, surface) in realized_volatility_surfaces {
        for source in surface.sources.iter().filter(|source| source.enabled) {
            let context = format!(
                "realized_volatility_surfaces.{surface_id}.sources.{}",
                source.source_id
            );
            match root.clients.get(source.data_client_id.as_str()) {
                None => errors.push(format!(
                    "{context}.data_client_id `{}` does not match any [clients.<id>] block",
                    source.data_client_id
                )),
                Some(client) if client.data.is_none() => errors.push(format!(
                    "{context}.data_client_id `{}` must reference a data-capable client (no [data] block)",
                    source.data_client_id
                )),
                Some(_) => {}
            }
        }
    }

    errors
}

pub(super) fn validate_realized_volatility_surfaces(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(realized_volatility_surfaces) = root.realized_volatility_surfaces.as_ref() else {
        return errors;
    };

    for (surface_id, surface) in realized_volatility_surfaces {
        let context = format!("realized_volatility_surfaces.{surface_id}");
        if surface_id.trim().is_empty() {
            errors.push("realized_volatility_surfaces contains an empty surface id".to_string());
        }
        if surface.canonical_base_asset.trim().is_empty() {
            errors.push(format!("{context}.canonical_base_asset must be non-empty"));
        }
        if surface.canonical_base_asset != surface.canonical_base_asset.trim() {
            errors.push(format!(
                "{context}.{} must not contain surrounding whitespace",
                stringify!(canonical_base_asset),
            ));
        }
        if surface.canonical_quote_asset.trim().is_empty() {
            errors.push(format!("{context}.canonical_quote_asset must be non-empty"));
        }
        if surface.canonical_quote_asset != surface.canonical_quote_asset.trim() {
            errors.push(format!(
                "{context}.{} must not contain surrounding whitespace",
                stringify!(canonical_quote_asset),
            ));
        }
        if surface.sources.is_empty() {
            errors.push(format!(
                "{context}.sources must contain at least one source"
            ));
        }

        let policy = &surface.policy;
        for (field, value) in [
            (stringify!(window_ms), policy.window_ms),
            (
                stringify!(sampling_interval_ms),
                policy.sampling_interval_ms,
            ),
            (
                stringify!(min_ready_sources),
                policy.min_ready_sources as u64,
            ),
            (stringify!(max_source_age_ms), policy.max_source_age_ms),
            (
                stringify!(max_inter_sample_gap_ms),
                policy.max_inter_sample_gap_ms,
            ),
        ] {
            if value == u64::MIN {
                errors.push(format!(
                    "{context}.policy.{field} must be a positive integer"
                ));
            }
        }
        if policy.window_ms < policy.sampling_interval_ms {
            errors.push(format!(
                "{context}.policy.{} {} must be greater than or equal to policy.{} {}",
                stringify!(window_ms),
                policy.window_ms,
                stringify!(sampling_interval_ms),
                policy.sampling_interval_ms,
            ));
        }
        if !is_positive_finite(policy.min_coverage_ratio) || policy.min_coverage_ratio > UNIT_F64 {
            errors.push(format!(
                "{context}.policy.min_coverage_ratio must be finite and in (0, 1]"
            ));
        }
        if !policy.max_cross_source_dispersion.is_finite()
            || policy.max_cross_source_dispersion < ZERO_F64
        {
            errors.push(format!(
                "{context}.policy.max_cross_source_dispersion must be finite and non-negative"
            ));
        }
        if !is_positive_finite(policy.seconds_per_annum) {
            errors.push(format!(
                "{context}.policy.seconds_per_annum must be positive finite"
            ));
        }
        if !policy.upper_quantile.is_finite()
            || !(HALF_F64..=UNIT_F64).contains(&policy.upper_quantile)
        {
            errors.push(format!(
                "{context}.policy.upper_quantile must be finite and in [0.5, 1.0]"
            ));
        }
        if matches!(
            policy.aggregation,
            RealizedVolatilityAggregationBlock::TrimmedMean
        ) {
            match policy.trim_fraction {
                Some(trim_fraction)
                    if trim_fraction.is_finite()
                        && (ZERO_F64..HALF_F64).contains(&trim_fraction) => {}
                _ => errors.push(format!(
                    "{context}.policy.trim_fraction must be finite and in [0, 0.5) for trimmed_mean aggregation"
                )),
            }
        }
        if matches!(
            policy.aggregation,
            RealizedVolatilityAggregationBlock::MedianWithUpperQuantileGuard
        ) {
            match policy.guard_weight {
                Some(guard_weight)
                    if guard_weight.is_finite()
                        && (ZERO_F64..=UNIT_F64).contains(&guard_weight) => {}
                _ => errors.push(format!(
                    "{context}.policy.guard_weight must be finite and in [0, 1] for median_with_upper_quantile_guard aggregation"
                )),
            }
        }
        if let Some(estimator) = surface.estimator.as_ref() {
            if estimator.noise_robust_method.is_none() {
                errors.push(format!(
                    "{context}.estimator.noise_robust_method must be set when estimator is configured"
                ));
            }
            if estimator.jump_policy.is_none() {
                errors.push(format!(
                    "{context}.estimator.jump_policy must be set when estimator is configured"
                ));
            }
            if estimator.pricing_component.is_none() {
                errors.push(format!(
                    "{context}.estimator.pricing_component must be set when estimator is configured"
                ));
            }
            if matches!(
                estimator.noise_robust_method,
                Some(RealizedVolatilityNoiseMethodBlock::Subsampled)
            ) {
                let subsamples = estimator
                    .subsamples
                    .unwrap_or(MISSING_REALIZED_VOLATILITY_SUBSAMPLE_COUNT);
                let min_ready_subsamples = estimator
                    .min_ready_subsamples
                    .unwrap_or(MISSING_REALIZED_VOLATILITY_SUBSAMPLE_COUNT);
                if subsamples == 0 || min_ready_subsamples == 0 {
                    errors.push(format!(
                        "{context}.estimator.subsamples and min_ready_subsamples must be positive for subsampled RV"
                    ));
                }
                if min_ready_subsamples > subsamples {
                    errors.push(format!(
                        "{context}.estimator.min_ready_subsamples must be less than or equal to subsamples"
                    ));
                }
                if subsamples as u64 > policy.sampling_interval_ms {
                    errors.push(format!(
                        "{context}.estimator.subsamples must not exceed policy.sampling_interval_ms unless collision semantics are explicitly supported"
                    ));
                }
            }
            if matches!(
                estimator.noise_robust_method,
                Some(RealizedVolatilityNoiseMethodBlock::CoarserGrid)
            ) && estimator.coarse_sampling_interval_ms.is_none()
            {
                errors.push(format!(
                    "{context}.estimator.coarse_sampling_interval_ms must be set for coarser_grid RV"
                ));
            }
            if matches!(
                estimator.noise_robust_method,
                Some(RealizedVolatilityNoiseMethodBlock::CoarserGrid)
            ) && estimator.coarser_grid_policy.is_none()
            {
                errors.push(format!(
                    "{context}.estimator.coarser_grid_policy must be set for coarser_grid RV"
                ));
            }
            if matches!(
                estimator.noise_robust_method,
                Some(RealizedVolatilityNoiseMethodBlock::CoarserGrid)
            ) && estimator
                .coarse_sampling_interval_ms
                .is_some_and(|interval| interval <= policy.sampling_interval_ms)
            {
                errors.push(format!(
                    "{context}.estimator.coarse_sampling_interval_ms must be greater than policy.sampling_interval_ms"
                ));
            }
            if matches!(
                estimator.pricing_component,
                Some(RealizedVolatilityPricingComponentBlock::NoiseRobust)
            ) && !matches!(
                estimator.noise_robust_method,
                Some(RealizedVolatilityNoiseMethodBlock::CoarserGrid)
                    | Some(RealizedVolatilityNoiseMethodBlock::Subsampled)
            ) {
                errors.push(format!(
                    "{context}.estimator.pricing_component noise_robust requires noise_robust_method other than none"
                ));
            }
            if matches!(
                estimator.pricing_component,
                Some(RealizedVolatilityPricingComponentBlock::Forecast)
            ) {
                errors.push(format!(
                    "{context}.estimator.pricing_component forecast is not enabled in this implementation slice"
                ));
            }
            if matches!(
                estimator.pricing_component,
                Some(RealizedVolatilityPricingComponentBlock::Continuous)
            ) && !matches!(
                estimator.jump_policy,
                Some(RealizedVolatilityJumpPolicyBlock::Separate)
            ) {
                errors.push(format!(
                    "{context}.estimator.pricing_component continuous requires jump_policy separate"
                ));
            }
        }

        let mut seen_source_ids = BTreeSet::new();
        let mut seen_source_instrument_clients: BTreeMap<String, (String, String)> =
            BTreeMap::new();
        let mut enabled_quorum_sources = 0usize;
        let mut quorum_source_contract: Option<(
            RealizedVolatilitySourceClassBlock,
            RealizedVolatilitySampleKindBlock,
            String,
        )> = None;
        for (index, source) in surface.sources.iter().enumerate() {
            let source_context = format!("{context}.sources[{index}]");
            if source.source_id.trim().is_empty() {
                errors.push(format!("{source_context}.source_id must be non-empty"));
            } else if !seen_source_ids.insert(source.source_id.as_str()) {
                errors.push(format!(
                    "{source_context}.source_id duplicate source_id `{}`",
                    source.source_id
                ));
            }

            if source.canonical_base_asset.trim().is_empty() {
                errors.push(format!(
                    "{source_context}.{} must be non-empty",
                    stringify!(canonical_base_asset),
                ));
            }
            if source.canonical_base_asset != source.canonical_base_asset.trim() {
                errors.push(format!(
                    "{source_context}.{} must not contain surrounding whitespace",
                    stringify!(canonical_base_asset),
                ));
            }
            if source.canonical_base_asset != surface.canonical_base_asset {
                errors.push(format!(
                    "{source_context}.{} `{}` must match {context}.{} `{}`",
                    stringify!(canonical_base_asset),
                    source.canonical_base_asset,
                    stringify!(canonical_base_asset),
                    surface.canonical_base_asset,
                ));
            }
            let instrument_key = source.instrument_id.to_string();
            let data_client_id = source.data_client_id.to_string();
            match seen_source_instrument_clients.get(&instrument_key) {
                Some((existing_data_client_id, existing_context))
                    if existing_data_client_id != &data_client_id =>
                {
                    errors.push(format!(
                        "{source_context}.instrument_id `{}` with data_client_id `{data_client_id}` is also used by {existing_context} with distinct data_client_id `{existing_data_client_id}`; realized_volatility_surfaces source events do not carry data_client_id, so same-instrument RV sources must share one data client",
                        source.instrument_id,
                    ));
                }
                Some(_) => {}
                None => {
                    seen_source_instrument_clients
                        .insert(instrument_key, (data_client_id, source_context.clone()));
                }
            }

            if source.canonical_quote_asset.trim().is_empty() {
                errors.push(format!(
                    "{source_context}.{} must be non-empty",
                    stringify!(canonical_quote_asset),
                ));
            }
            if source.canonical_quote_asset != source.canonical_quote_asset.trim() {
                errors.push(format!(
                    "{source_context}.{} must not contain surrounding whitespace",
                    stringify!(canonical_quote_asset),
                ));
            }
            if source.canonical_quote_asset != surface.canonical_quote_asset {
                errors.push(format!(
                    "{source_context}.{} `{}` must match {context}.{} `{}`",
                    stringify!(canonical_quote_asset),
                    source.canonical_quote_asset,
                    stringify!(canonical_quote_asset),
                    surface.canonical_quote_asset,
                ));
            }
            if !realized_volatility_source_pair_supported(source.source_class, source.sample_kind) {
                errors.push(format!(
                    "{source_context}.{} {:?} with {} {:?} is not supported by the taker realized-volatility router",
                    stringify!(source_class),
                    source.source_class,
                    stringify!(sample_kind),
                    source.sample_kind,
                ));
            }
            if source.enabled && source.counts_toward_quorum {
                enabled_quorum_sources += 1;
                match quorum_source_contract.as_ref() {
                    Some((source_class, sample_kind, existing_context))
                        if source.source_class != *source_class
                            || source.sample_kind != *sample_kind =>
                    {
                        errors.push(format!(
                            "{source_context}.source_class/sample_kind {:?}/{:?} must match enabled quorum source contract {:?}/{:?} established by {existing_context}",
                            source.source_class,
                            source.sample_kind,
                            source_class,
                            sample_kind,
                        ));
                    }
                    Some(_) => {}
                    None => {
                        quorum_source_contract = Some((
                            source.source_class,
                            source.sample_kind,
                            source_context.clone(),
                        ));
                    }
                }
            }
        }

        if policy.min_ready_sources > enabled_quorum_sources {
            errors.push(format!(
                "{context}.policy.min_ready_sources {} exceeds enabled quorum source count {}",
                policy.min_ready_sources, enabled_quorum_sources
            ));
        }
    }

    errors
}

fn realized_volatility_source_pair_supported(
    source_class: RealizedVolatilitySourceClassBlock,
    sample_kind: RealizedVolatilitySampleKindBlock,
) -> bool {
    matches!(
        (source_class, sample_kind),
        (
            RealizedVolatilitySourceClassBlock::SpotQuote,
            RealizedVolatilitySampleKindBlock::Midpoint,
        ) | (
            RealizedVolatilitySourceClassBlock::Trade,
            RealizedVolatilitySampleKindBlock::Trade,
        ) | (
            RealizedVolatilitySourceClassBlock::Index,
            RealizedVolatilitySampleKindBlock::Index,
        )
    )
}
