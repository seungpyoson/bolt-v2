use super::*;

pub(super) fn validate_gate_providers(
    providers: &BTreeMap<String, GateProviderBlock>,
    clients: &BTreeMap<String, ClientBlock>,
) -> Vec<String> {
    let mut errors = Vec::new();

    for (provider_id, provider) in providers {
        let context = format!("gate_providers.{provider_id}");
        let provider_kind = match provider.provider_kind.as_deref() {
            Some(value) if GATE_PROVIDER_KINDS.contains(&value) => Some(value),
            Some(value) => {
                errors.push(format!(
                    "{context}.provider_kind `{value}` is unregistered; supported gate provider kinds are {GATE_PROVIDER_KINDS:?}"
                ));
                None
            }
            None => {
                errors.push(format!("{context}.provider_kind is required"));
                None
            }
        };

        if matches!(provider_kind, Some(kind) if kind == TEST_DOUBLE_PROVIDER_KIND) {
            errors.push(format!(
                "{context}.provider_kind `test_double` is test-only and is not allowed in live/local operator TOML"
            ));
        }

        match &provider.capabilities {
            Some(capabilities) if capabilities.is_empty() => {
                errors.push(format!(
                    "{context}.capabilities must contain one or more semantic capabilities"
                ));
            }
            Some(capabilities) => {
                for capability in capabilities {
                    if !GATE_PROVIDER_CAPABILITIES.contains(&capability.as_str()) {
                        errors.push(format!(
                            "{context}.capabilities contains unregistered capability `{capability}`; supported capabilities are {GATE_PROVIDER_CAPABILITIES:?}"
                        ));
                    }
                }
            }
            None => errors.push(format!(
                "{context}.capabilities must contain one or more semantic capabilities"
            )),
        }

        match &provider.freshness {
            Some(freshness) => errors.extend(validate_gate_provider_freshness(
                &format!("{context}.freshness"),
                freshness,
            )),
            None => errors.push(format!("{context}.freshness is required")),
        }

        if let Some(client_id) = &provider.client_id
            && !clients.contains_key(client_id.as_str())
        {
            errors.push(format!(
                "{context}.client_id `{client_id}` does not match any [clients.<id>] block"
            ));
        }

        if let Some(kind) = provider_kind {
            let expected_table = format!("[{context}.{kind}]");
            match provider.provider_config.get(kind) {
                Some(value) if value.as_table().is_some() => {}
                _ => errors.push(format!(
                    "{context} with provider_kind `{kind}` must define exactly one matching provider-specific subtable {expected_table}"
                )),
            }
            if provider.provider_config.len() != 1 {
                errors.push(format!(
                    "{context} with provider_kind `{kind}` must define exactly one provider-specific subtable; expected {expected_table}"
                ));
            }
            for table_name in provider.provider_config.keys() {
                if table_name != kind {
                    errors.push(format!(
                        "{context} has provider-specific subtable [gate_providers.{provider_id}.{table_name}] but provider_kind `{kind}` requires {expected_table}"
                    ));
                }
            }
            if kind == CHAINLINK_DATA_STREAMS_PROVIDER_KIND
                && let Some(table) = provider
                    .provider_config
                    .get(kind)
                    .and_then(toml::Value::as_table)
            {
                errors.extend(validate_chainlink_data_streams_gate_provider(
                    &context, table,
                ));
            }
        }

        for (table_name, value) in &provider.provider_config {
            if let Some(table) = value.as_table()
                && let Some(parameter) = table.get(SSM_CREDENTIAL_PARAMETER_FIELD)
            {
                match parameter.as_str() {
                    Some(path) => errors.extend(validate_gate_provider_ssm_parameter_path(
                        provider_id,
                        table_name,
                        SSM_CREDENTIAL_PARAMETER_FIELD,
                        path,
                    )),
                    None => errors.push(format!(
                        "gate_providers.{provider_id}.{table_name}.ssm_credential_parameter must be a string SSM path"
                    )),
                }
            }
        }
    }

    errors
}

fn validate_chainlink_data_streams_gate_provider(
    context: &str,
    table: &toml::map::Map<String, toml::Value>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let feed_bindings_context =
        format!("{context}.chainlink_data_streams.{CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD}");

    for field in table.keys() {
        let is_old_provider_level_feed_field =
            CHAINLINK_DATA_STREAMS_OLD_PROVIDER_LEVEL_FEED_FIELDS.contains(&field.as_str());
        if !CHAINLINK_DATA_STREAMS_PROVIDER_FIELDS.contains(&field.as_str())
            && !is_old_provider_level_feed_field
        {
            errors.push(format!(
                "{context}.chainlink_data_streams.{field} is not a supported chainlink_data_streams provider field"
            ));
        }
    }

    if table.contains_key(CHAINLINK_DATA_STREAMS_OLD_SSM_CREDENTIAL_PARAMETER_FIELD) {
        errors.push(format!(
            "{context}.chainlink_data_streams.{CHAINLINK_DATA_STREAMS_OLD_SSM_CREDENTIAL_PARAMETER_FIELD} must be replaced by {CHAINLINK_DATA_STREAMS_API_KEY_SSM_PARAMETER_FIELD} and {CHAINLINK_DATA_STREAMS_API_SECRET_SSM_PARAMETER_FIELD}"
        ));
    }
    required_string_field(
        table,
        &format!("{context}.chainlink_data_streams"),
        CHAINLINK_DATA_STREAMS_ENDPOINT_ID_FIELD,
        &mut errors,
    );
    let rest_base_url = required_string_field(
        table,
        &format!("{context}.chainlink_data_streams"),
        CHAINLINK_DATA_STREAMS_REST_BASE_URL_FIELD,
        &mut errors,
    );
    if let Some(rest_base_url) = rest_base_url {
        validate_https_rest_base_url(
            &format!(
                "{context}.chainlink_data_streams.{CHAINLINK_DATA_STREAMS_REST_BASE_URL_FIELD}"
            ),
            rest_base_url,
            &mut errors,
        );
    }
    if let Some(report_endpoint_path) = required_string_field(
        table,
        &format!("{context}.chainlink_data_streams"),
        CHAINLINK_DATA_STREAMS_REPORT_ENDPOINT_PATH_FIELD,
        &mut errors,
    ) && let Some(rest_base_url) = rest_base_url
    {
        validate_chainlink_report_endpoint_path(
            &format!(
                "{context}.chainlink_data_streams.{CHAINLINK_DATA_STREAMS_REPORT_ENDPOINT_PATH_FIELD}"
            ),
            rest_base_url,
            report_endpoint_path,
            &mut errors,
        );
    }
    required_positive_integer_field(
        table,
        &format!("{context}.chainlink_data_streams"),
        CHAINLINK_DATA_STREAMS_HTTP_TIMEOUT_SECS_FIELD,
        &mut errors,
    );
    errors.extend(validate_chainlink_data_streams_ssm_parameter_field(
        context,
        table,
        CHAINLINK_DATA_STREAMS_API_KEY_SSM_PARAMETER_FIELD,
    ));
    errors.extend(validate_chainlink_data_streams_ssm_parameter_field(
        context,
        table,
        CHAINLINK_DATA_STREAMS_API_SECRET_SSM_PARAMETER_FIELD,
    ));

    for field in CHAINLINK_DATA_STREAMS_OLD_PROVIDER_LEVEL_FEED_FIELDS {
        if table.contains_key(*field) {
            errors.push(format!(
                "{context}.chainlink_data_streams.{field} must move under [[{feed_bindings_context}]]"
            ));
        }
    }

    let Some(feed_bindings) = table
        .get(CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD)
        .and_then(toml::Value::as_array)
        .filter(|bindings| !bindings.is_empty())
    else {
        errors.push(format!(
            "{feed_bindings_context} must contain one or more resolution feed bindings"
        ));
        return errors;
    };

    let mut seen = HashSet::new();
    for (index, binding_value) in feed_bindings.iter().enumerate() {
        let binding_context = format!("{feed_bindings_context}[{index}]");
        let Some(binding) = binding_value.as_table() else {
            errors.push(format!("{binding_context} must be a TOML table"));
            continue;
        };
        let resolution_identity = required_string_field(
            binding,
            &binding_context,
            CHAINLINK_DATA_STREAMS_RESOLUTION_IDENTITY_FIELD,
            &mut errors,
        );
        let value_kind = required_string_field(
            binding,
            &binding_context,
            CHAINLINK_DATA_STREAMS_VALUE_KIND_FIELD,
            &mut errors,
        );
        if let Some(value_kind) = value_kind
            && value_kind != PRICE_GATE_VALUE_KIND
        {
            errors.push(format!(
                "{binding_context}.value_kind `{value_kind}` is not supported for chainlink_data_streams price reports"
            ));
        }
        if let (Some(resolution_identity), Some(value_kind)) = (resolution_identity, value_kind)
            && !seen.insert((resolution_identity.to_string(), value_kind.to_string()))
        {
            errors.push(format!(
                "{binding_context} duplicates resolution_identity `{resolution_identity}` and value_kind `{value_kind}`"
            ));
        }
        if let Some(feed_id) = required_string_field(
            binding,
            &binding_context,
            CHAINLINK_DATA_STREAMS_FEED_ID_FIELD,
            &mut errors,
        ) && !is_lowercase_chainlink_feed_id(feed_id)
        {
            errors.push(format!(
                "{binding_context}.feed_id must be a lowercase chainlink_data_streams feed id"
            ));
        }
        required_positive_integer_field(
            binding,
            &binding_context,
            CHAINLINK_DATA_STREAMS_REPORT_SCHEMA_VERSION_FIELD,
            &mut errors,
        );
        required_positive_integer_field(
            binding,
            &binding_context,
            CHAINLINK_DATA_STREAMS_REPORT_DECIMAL_SCALE_FIELD,
            &mut errors,
        );
    }

    errors
}

fn validate_chainlink_data_streams_ssm_parameter_field(
    context: &str,
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    let field_context = format!("{context}.chainlink_data_streams.{field}");
    let Some(value) = table.get(field) else {
        errors.push(format!("{field_context} must be a string SSM path"));
        return errors;
    };
    let Some(path) = value.as_str() else {
        errors.push(format!("{field_context} must be a string SSM path"));
        return errors;
    };
    let trimmed = path.trim();
    if trimmed.is_empty() {
        errors.push(format!("{field_context} must be a non-empty SSM path"));
    } else {
        if trimmed != path {
            errors.push(format!(
                "{field_context} must not have leading or trailing whitespace"
            ));
        }
        if !trimmed.starts_with('/') {
            errors.push(format!(
                "{field_context} must be an absolute-style SSM parameter path starting with `/`: `{path}`"
            ));
        }
    }
    errors
}

fn required_string_field<'a>(
    table: &'a toml::map::Map<String, toml::Value>,
    context: &str,
    field: &str,
    errors: &mut Vec<String>,
) -> Option<&'a str> {
    match table.get(field).and_then(toml::Value::as_str) {
        Some(value) if !value.trim().is_empty() => Some(value.trim()),
        _ => {
            errors.push(format!("{context}.{field} must be a non-empty string"));
            None
        }
    }
}

fn required_positive_integer_field(
    table: &toml::map::Map<String, toml::Value>,
    context: &str,
    field: &str,
    errors: &mut Vec<String>,
) {
    if table
        .get(field)
        .and_then(toml::Value::as_integer)
        .is_none_or(|value| value <= 0)
    {
        errors.push(format!("{context}.{field} must be a positive integer"));
    }
}

fn is_lowercase_chainlink_feed_id(value: &str) -> bool {
    value.len() == 66
        && value.starts_with("0x")
        && value[2..]
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
}

fn validate_gate_provider_freshness(
    context: &str,
    freshness: &GateProviderFreshnessBlock,
) -> Vec<String> {
    let mut errors = Vec::new();

    match freshness.max_age_ms {
        Some(0) => errors.push(format!("{context}.max_age_ms must be a positive integer")),
        Some(_) => {}
        None => errors.push(format!("{context}.max_age_ms is required")),
    }
    match freshness.max_clock_skew_ms {
        Some(0) => errors.push(format!(
            "{context}.max_clock_skew_ms must be a positive integer"
        )),
        Some(_) => {}
        None => errors.push(format!("{context}.max_clock_skew_ms is required")),
    }
    if let (Some(max_age_ms), Some(max_clock_skew_ms)) =
        (freshness.max_age_ms, freshness.max_clock_skew_ms)
        && max_clock_skew_ms > max_age_ms
    {
        errors.push(format!(
            "{context}.max_clock_skew_ms must be less than or equal to {context}.max_age_ms"
        ));
    }

    errors
}

fn validate_gate_provider_ssm_parameter_path(
    provider_id: &str,
    table_name: &str,
    field: &str,
    value: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    let context = format!("gate_providers.{provider_id}.{table_name}.{field}");
    let trimmed = value.trim();
    if trimmed.is_empty() {
        errors.push(format!("{context} must be a non-empty SSM path"));
    } else {
        if trimmed != value {
            errors.push(format!(
                "{context} must not have leading or trailing whitespace"
            ));
        }
        if !trimmed.starts_with('/') {
            errors.push(format!(
                "{context} must be an absolute-style SSM parameter path starting with `/`: `{value}`"
            ));
        }
    }
    errors
}
