use super::*;

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct ResolutionFeedBindingKey {
    provider_id: String,
    resolution_identity: String,
    value_kind: String,
}

#[derive(Debug, Clone)]
struct ResolutionFeedMappingReference {
    key: ResolutionFeedBindingKey,
    context: String,
}

pub(crate) fn validate_https_rest_base_url(
    field_path: &str,
    rest_base_url: &str,
    errors: &mut Vec<String>,
) {
    match url::Url::parse(rest_base_url) {
        Ok(parsed) if parsed.scheme() != "https" => errors.push(format!(
            "{field_path} must use the https scheme (got `{scheme}`); \
             signed credentials must never be sent over an insecure transport",
            scheme = parsed.scheme()
        )),
        Ok(_) => {}
        Err(_) => errors.push(format!("{field_path} must be a valid absolute URL")),
    }
}

/// Resolves a Chainlink Data Streams `report_endpoint_path` against its
/// `rest_base_url`, failing closed against any value that would redirect the
/// credential-bearing report request off the configured endpoint. The HMAC-signed
/// Data Streams credentials travel with this request, so the path must be a rooted
/// absolute path and the joined URL must keep the base scheme, host, port, and
/// userinfo (username/password) and introduce no query or fragment of its own.
/// `url::Url::join` otherwise accepts absolute URLs (`https://other/...`) and
/// scheme-relative/authority paths (`//other/...`, `//user:pass@host/...`) that
/// silently swap the host or inject userinfo while still receiving the signed
/// credentials.
/// Shared by the live-strike client validator, the resolution-oracle gate-provider
/// validator, and the request-URL builder so the endpoint can only ever resolve to
/// the configured host. Returns the safe joined URL so the builder reuses one
/// resolution rather than re-joining.
pub(crate) fn resolve_chainlink_report_endpoint_url(
    rest_base_url: &str,
    report_endpoint_path: &str,
) -> Result<url::Url, String> {
    let base = url::Url::parse(rest_base_url)
        .map_err(|_| "must resolve against a valid absolute base URL".to_string())?;
    // Require a single rooted path. `strip_prefix` enforces the leading slash; a
    // second leading slash or backslash makes the value an authority/scheme-relative
    // reference (`//host`, `/\host`) that `url::Url::join` resolves into
    // host/userinfo/port — including same-host `//user:pass@host` forms that would
    // smuggle credentials into the signed request URL — rather than a path.
    let after_root = match report_endpoint_path.strip_prefix('/') {
        Some(after_root) => after_root,
        None => {
            return Err("must be a rooted absolute path beginning with a single slash".to_string());
        }
    };
    if after_root.starts_with('/') || after_root.starts_with('\\') {
        return Err(
            "must be a single rooted path, not a scheme-relative or authority reference"
                .to_string(),
        );
    }
    let joined = base
        .join(report_endpoint_path)
        .map_err(|_| "must be a path that resolves against the base URL".to_string())?;
    // Authoritative backstop: resolving the path must change only the path. The
    // scheme, host, port, and userinfo must match the base, and the path must
    // introduce no query or fragment of its own.
    if joined.scheme() != base.scheme()
        || joined.host_str() != base.host_str()
        || joined.port_or_known_default() != base.port_or_known_default()
        || joined.username() != base.username()
        || joined.password() != base.password()
        || joined.query().is_some()
        || joined.fragment().is_some()
    {
        return Err(
            "must not redirect off the base URL host, scheme, port, or credentials, or carry a query or fragment"
                .to_string(),
        );
    }
    Ok(joined)
}

/// Validates a Chainlink Data Streams `report_endpoint_path` config field via
/// [`resolve_chainlink_report_endpoint_url`], pushing a field-scoped error on any
/// value that would redirect the signed request off the configured host. A
/// malformed base URL is reported by the `rest_base_url` validator, so this skips
/// that case to avoid double-reporting.
pub(crate) fn validate_chainlink_report_endpoint_path(
    field_path: &str,
    rest_base_url: &str,
    report_endpoint_path: &str,
    errors: &mut Vec<String>,
) {
    if url::Url::parse(rest_base_url).is_err() {
        return;
    }
    if let Err(reason) = resolve_chainlink_report_endpoint_url(rest_base_url, report_endpoint_path)
    {
        errors.push(format!("{field_path} {reason}"));
    }
}

pub(super) fn validate_root_owned_chainlink_feed_catalog(
    root: &BoltV3RootConfig,
    key: &str,
    client: &ClientBlock,
) -> Vec<String> {
    if !uses_root_owned_chainlink_feed_catalog(client) || client.data.is_none() {
        return Vec::new();
    }
    let has_client_feed_bindings = client
        .data
        .as_ref()
        .and_then(toml::Value::as_table)
        .is_some_and(|data| data.contains_key(CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD));
    let mut errors = Vec::new();
    if has_client_feed_bindings {
        errors.push(format!(
            "chainlink_data_streams.feed_bindings is root-owned; clients.{key}.data.feed_bindings must be removed so feed bindings have one configured path"
        ));
    }
    if root.chainlink_data_streams.is_none() {
        errors.push(format!(
            "chainlink_data_streams.feed_bindings must be configured for clients.{key}; clients.{key}.data.feed_bindings is not supported"
        ));
    }
    errors
}

pub(super) fn uses_root_owned_chainlink_feed_catalog(client: &ClientBlock) -> bool {
    client.venue.as_str() == crate::bolt_v3_providers::RESOLUTION_ORACLE_VENUE_KEY
}

pub(crate) fn client_with_root_chainlink_feed_catalog(
    root: &BoltV3RootConfig,
    client: &ClientBlock,
) -> Option<ClientBlock> {
    let catalog = root.chainlink_data_streams.as_ref()?;
    if client.venue.as_str() != crate::bolt_v3_providers::RESOLUTION_ORACLE_VENUE_KEY {
        return None;
    }
    let data = client.data.as_ref()?.as_table()?;
    if data.contains_key(CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD) {
        return None;
    }

    let mut client = client.clone();
    let data = client.data.as_mut()?.as_table_mut()?;
    data.insert(
        CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD.to_string(),
        toml::Value::Array(catalog.feed_bindings.clone()),
    );
    Some(client)
}

pub(super) fn validate_chainlink_feed_binding_coverage(
    root: &BoltV3RootConfig,
    strategies: &[LoadedStrategy],
) -> Vec<String> {
    let mut errors = Vec::new();
    let target_references = collect_chainlink_target_mapping_references(strategies, &mut errors);
    let target_keys = target_references
        .iter()
        .map(|reference| reference.key.clone())
        .collect::<BTreeSet<_>>();
    let feed_bindings = collect_chainlink_feed_bindings(root);

    for reference in &target_references {
        let binding_count = match feed_bindings.get(&reference.key) {
            Some(contexts) => contexts.len(),
            None => 0,
        };
        match binding_count {
            1 => {}
            0 => errors.push(format!(
                "{}: chainlink_data_streams mapping provider_id `{}` resolution_identity `{}` value_kind `{}` has no matching gate_providers.{}.chainlink_data_streams.feed_bindings entry",
                reference.context,
                reference.key.provider_id,
                reference.key.resolution_identity,
                reference.key.value_kind,
                reference.key.provider_id
            )),
            count => errors.push(format!(
                "{}: chainlink_data_streams mapping provider_id `{}` resolution_identity `{}` value_kind `{}` has {count} matching gate_providers.{}.chainlink_data_streams.feed_bindings entries; expected exactly one",
                reference.context,
                reference.key.provider_id,
                reference.key.resolution_identity,
                reference.key.value_kind,
                reference.key.provider_id
            )),
        }
    }

    for (key, contexts) in &feed_bindings {
        if !target_keys.contains(key) {
            for context in contexts {
                errors.push(format!(
                    "{context} resolution_identity `{}` value_kind `{}` is not referenced by any loaded strategy chainlink_data_streams mapping",
                    key.resolution_identity, key.value_kind
                ));
            }
        }
    }

    errors
}

fn collect_chainlink_target_mapping_references(
    strategies: &[LoadedStrategy],
    errors: &mut Vec<String>,
) -> Vec<ResolutionFeedMappingReference> {
    let mut references = Vec::new();

    for loaded in strategies {
        let strategy_context = format!("strategy `{}`", loaded.relative_path);
        let Some(target) = loaded.config.target.as_table() else {
            continue;
        };
        let Some(gate_subscriptions) = target
            .get(TARGET_GATE_SUBSCRIPTIONS_FIELD)
            .and_then(toml::Value::as_table)
        else {
            continue;
        };
        for (role, subscription_value) in gate_subscriptions {
            let Some(subscription) = subscription_value.as_table() else {
                continue;
            };
            let Some(market_mappings) = subscription
                .get(TARGET_MARKET_MAPPINGS_FIELD)
                .and_then(toml::Value::as_array)
            else {
                continue;
            };
            for (index, mapping_value) in market_mappings.iter().enumerate() {
                let Some(mapping) = mapping_value.as_table() else {
                    continue;
                };
                if string_field(mapping, TARGET_RESOLUTION_KIND_FIELD).as_deref()
                    != Some(CHAINLINK_DATA_STREAMS_PROVIDER_KIND)
                {
                    continue;
                }
                let (Some(resolution_identity), Some(value_kind)) = (
                    string_field(mapping, CHAINLINK_DATA_STREAMS_RESOLUTION_IDENTITY_FIELD),
                    string_field(mapping, CHAINLINK_DATA_STREAMS_VALUE_KIND_FIELD),
                ) else {
                    continue;
                };
                let Some(provider_id) = selected_chainlink_provider_id(subscription, mapping)
                else {
                    errors.push(format!(
                        "{strategy_context}: target.{TARGET_GATE_SUBSCRIPTIONS_FIELD}.{role}.{TARGET_MARKET_MAPPINGS_FIELD}[{index}]: chainlink_data_streams mapping resolution_identity `{resolution_identity}` value_kind `{value_kind}` cannot resolve provider_id from mapping provider_id, provider_preference, or a single allowed_provider_ids entry"
                    ));
                    continue;
                };
                references.push(ResolutionFeedMappingReference {
                    key: ResolutionFeedBindingKey {
                        provider_id,
                        resolution_identity,
                        value_kind,
                    },
                    context: format!(
                        "{strategy_context}: target.{TARGET_GATE_SUBSCRIPTIONS_FIELD}.{role}.{TARGET_MARKET_MAPPINGS_FIELD}[{index}]"
                    ),
                });
            }
        }
    }

    references
}

fn selected_chainlink_provider_id(
    subscription: &toml::map::Map<String, toml::Value>,
    mapping: &toml::map::Map<String, toml::Value>,
) -> Option<String> {
    string_field(mapping, TARGET_PROVIDER_ID_FIELD)
        .or_else(|| first_string_array_value(subscription, TARGET_PROVIDER_PREFERENCE_FIELD))
        .or_else(|| single_string_array_value(subscription, TARGET_ALLOWED_PROVIDER_IDS_FIELD))
}

fn collect_chainlink_feed_bindings(
    root: &BoltV3RootConfig,
) -> BTreeMap<ResolutionFeedBindingKey, Vec<String>> {
    let mut bindings: BTreeMap<ResolutionFeedBindingKey, Vec<String>> = BTreeMap::new();
    let Some(gate_providers) = &root.gate_providers else {
        return bindings;
    };

    for (provider_id, provider) in gate_providers {
        if provider.provider_kind.as_deref().map(str::trim)
            != Some(CHAINLINK_DATA_STREAMS_PROVIDER_KIND)
        {
            continue;
        }
        let Some(provider_config) = provider
            .provider_config
            .get(CHAINLINK_DATA_STREAMS_PROVIDER_KIND)
            .and_then(toml::Value::as_table)
        else {
            continue;
        };
        let Some(feed_bindings) = provider_config
            .get(CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD)
            .and_then(toml::Value::as_array)
        else {
            continue;
        };
        for (index, binding_value) in feed_bindings.iter().enumerate() {
            let Some(binding) = binding_value.as_table() else {
                continue;
            };
            let (Some(resolution_identity), Some(value_kind)) = (
                string_field(binding, CHAINLINK_DATA_STREAMS_RESOLUTION_IDENTITY_FIELD),
                string_field(binding, CHAINLINK_DATA_STREAMS_VALUE_KIND_FIELD),
            ) else {
                continue;
            };
            let key = ResolutionFeedBindingKey {
                provider_id: provider_id.clone(),
                resolution_identity,
                value_kind,
            };
            let context = format!(
                "gate_providers.{provider_id}.chainlink_data_streams.{CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD}[{index}]"
            );
            match bindings.entry(key) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().push(context);
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(vec![context]);
                }
            }
        }
    }

    bindings
}
