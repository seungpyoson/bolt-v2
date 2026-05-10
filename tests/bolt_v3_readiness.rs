mod support;

use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_readiness::{
        BoltV3StartupCheckReport, BoltV3StartupCheckStage, BoltV3StartupCheckStatus,
        BoltV3StartupCheckSubject, run_bolt_v3_startup_check_with,
    },
};

fn statuses_for(
    report: &BoltV3StartupCheckReport,
    stage: BoltV3StartupCheckStage,
) -> Vec<BoltV3StartupCheckStatus> {
    report
        .facts
        .iter()
        .filter(|fact| fact.stage == stage)
        .map(|fact| fact.status)
        .collect()
}

fn skipped_stages(report: &BoltV3StartupCheckReport) -> Vec<BoltV3StartupCheckStage> {
    report
        .facts
        .iter()
        .filter(|fact| fact.status == BoltV3StartupCheckStatus::Skipped)
        .map(|fact| fact.stage)
        .collect()
}

fn report_text(report: &BoltV3StartupCheckReport) -> String {
    format!("{report:#?}")
}

fn assert_no_resolved_secret_values(text: &str) {
    for secret_value in [
        "0x4242424242424242424242424242424242424242424242424242424242424242",
        "polymarket-api-key",
        "YWJj",
        "polymarket-passphrase",
        "binance-api-key",
        "MC4CAQAwBQYDK2VwBCIEIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f",
    ] {
        assert!(
            !text.contains(secret_value),
            "report must not contain resolved secret value {secret_value}: {text}"
        );
    }
}

// This suite covers skip-chain behavior and a deterministic final `builder.build()`
// failure. Direct registration failures are covered by the registration boundary's
// unit tests. A direct readiness-stage builder-construction failure is not
// synthesized here because current Bolt-v3 config can only produce NT's Live
// environment; the NT Backtest rejection is covered in the live-node boundary tests.

#[test]
fn startup_check_reports_success_facts_without_connecting() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    let report = run_bolt_v3_startup_check_with(&loaded, |_| false, support::fake_bolt_v3_resolver);

    for stage in [
        BoltV3StartupCheckStage::ForbiddenCredentialEnv,
        BoltV3StartupCheckStage::SecretResolution,
        BoltV3StartupCheckStage::AdapterMapping,
        BoltV3StartupCheckStage::LiveNodeBuilder,
        BoltV3StartupCheckStage::ClientRegistration,
        BoltV3StartupCheckStage::LiveNodeBuild,
    ] {
        let statuses = statuses_for(&report, stage);
        assert!(
            statuses.contains(&BoltV3StartupCheckStatus::Satisfied),
            "{stage:?} should have a satisfied fact in {report:#?}"
        );
    }

    assert!(
        report
            .facts
            .iter()
            .all(|fact| fact.status != BoltV3StartupCheckStatus::Failed),
        "success fixture should not emit failed facts: {report:#?}"
    );
    assert!(
        report
            .facts
            .iter()
            .all(|fact| fact.status != BoltV3StartupCheckStatus::Skipped),
        "success fixture should not emit skipped facts: {report:#?}"
    );

    let polymarket = report
        .facts
        .iter()
        .find(|fact| {
            fact.stage == BoltV3StartupCheckStage::ClientRegistration
                && fact.detail.contains("polymarket_main")
        })
        .expect("polymarket_main registration fact should exist");
    assert!(polymarket.detail.contains("data=true"));
    assert!(polymarket.detail.contains("execution=true"));

    let binance = report
        .facts
        .iter()
        .find(|fact| {
            fact.stage == BoltV3StartupCheckStage::ClientRegistration
                && fact.detail.contains("binance_reference")
        })
        .expect("binance_reference registration fact should exist");
    assert!(binance.detail.contains("data=true"));
    assert!(binance.detail.contains("execution=false"));

    assert_no_resolved_secret_values(&report_text(&report));
}

#[test]
fn startup_check_reports_empty_adapter_instance_stages_as_satisfied_root_facts() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let empty_loaded = LoadedBoltV3Config {
        root_path: loaded.root_path.clone(),
        root: BoltV3RootConfig {
            adapter_instances: BTreeMap::new(),
            ..loaded.root
        },
        strategies: Vec::new(),
    };
    let resolver = |_region: &str, _path: &str| -> Result<String, &'static str> {
        Err("resolver must not be called when no adapter instances are configured")
    };

    let report = run_bolt_v3_startup_check_with(&empty_loaded, |_| false, resolver);

    for stage in [
        BoltV3StartupCheckStage::ForbiddenCredentialEnv,
        BoltV3StartupCheckStage::SecretResolution,
        BoltV3StartupCheckStage::AdapterMapping,
        BoltV3StartupCheckStage::LiveNodeBuilder,
        BoltV3StartupCheckStage::ClientRegistration,
        BoltV3StartupCheckStage::LiveNodeBuild,
    ] {
        assert_eq!(
            statuses_for(&report, stage),
            vec![BoltV3StartupCheckStatus::Satisfied],
            "{stage:?} should be explicitly satisfied at root level for empty adapter-instance configs: {report:#?}"
        );
    }
}

#[test]
fn startup_check_reports_forbidden_env_failure_and_skips_downstream() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    let report = run_bolt_v3_startup_check_with(
        &loaded,
        |var| var == "POLYMARKET_PK",
        support::fake_bolt_v3_resolver,
    );

    assert_eq!(
        statuses_for(&report, BoltV3StartupCheckStage::ForbiddenCredentialEnv),
        vec![BoltV3StartupCheckStatus::Failed],
        "{report:#?}"
    );
    assert!(
        report.facts.iter().any(|fact| {
            fact.stage == BoltV3StartupCheckStage::ForbiddenCredentialEnv
                && fact.subject
                    == BoltV3StartupCheckSubject::AdapterInstance("polymarket_main".to_string())
        }),
        "forbidden env failures should be adapter-instance-keyed: {report:#?}"
    );
    assert!(
        report
            .facts
            .iter()
            .filter(|fact| {
                fact.stage == BoltV3StartupCheckStage::ForbiddenCredentialEnv
                    && fact.status == BoltV3StartupCheckStatus::Failed
            })
            .all(|fact| matches!(fact.subject, BoltV3StartupCheckSubject::AdapterInstance(_))),
        "all forbidden env failures should be adapter-instance-keyed: {report:#?}"
    );
    assert_eq!(
        skipped_stages(&report),
        vec![
            BoltV3StartupCheckStage::SecretResolution,
            BoltV3StartupCheckStage::AdapterMapping,
            BoltV3StartupCheckStage::LiveNodeBuilder,
            BoltV3StartupCheckStage::ClientRegistration,
            BoltV3StartupCheckStage::LiveNodeBuild,
        ],
        "{report:#?}"
    );
}

#[test]
fn startup_check_reports_secret_resolution_failure_and_skips_downstream() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let bad_resolver = |region: &str, path: &str| -> Result<String, &'static str> {
        if path == "/bolt/polymarket_main/private_key" {
            Err("simulated SSM permissions denied for private key")
        } else {
            support::fake_bolt_v3_resolver(region, path)
        }
    };

    let report = run_bolt_v3_startup_check_with(&loaded, |_| false, bad_resolver);

    assert_eq!(
        statuses_for(&report, BoltV3StartupCheckStage::SecretResolution),
        vec![BoltV3StartupCheckStatus::Failed],
        "{report:#?}"
    );
    let text = report_text(&report);
    assert!(text.contains("adapter_instances.polymarket_main.secrets.private_key"));
    assert!(text.contains("/bolt/polymarket_main/private_key"));
    assert_no_resolved_secret_values(&text);
    assert_eq!(
        skipped_stages(&report),
        vec![
            BoltV3StartupCheckStage::AdapterMapping,
            BoltV3StartupCheckStage::LiveNodeBuilder,
            BoltV3StartupCheckStage::ClientRegistration,
            BoltV3StartupCheckStage::LiveNodeBuild,
        ],
        "{report:#?}"
    );
}

#[test]
fn startup_check_reports_adapter_mapping_failure_and_redacts_resolved_secrets() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded
        .root
        .adapter_instances
        .get_mut("polymarket_main")
        .expect("fixture polymarket_main adapter instance should exist")
        .data
        .as_mut()
        .expect("fixture polymarket_main data block should exist")
        .as_table_mut()
        .expect("fixture data block should be a TOML table")
        .insert(
            "subscribe_new_markets".to_string(),
            toml::Value::Boolean(true),
        );

    let report = run_bolt_v3_startup_check_with(&loaded, |_| false, support::fake_bolt_v3_resolver);

    assert_eq!(
        statuses_for(&report, BoltV3StartupCheckStage::AdapterMapping),
        vec![BoltV3StartupCheckStatus::Failed],
        "{report:#?}"
    );
    assert!(
        report.facts.iter().any(|fact| {
            fact.stage == BoltV3StartupCheckStage::AdapterMapping
                && fact.subject
                    == BoltV3StartupCheckSubject::AdapterInstance("polymarket_main".to_string())
        }),
        "adapter mapping failures must be adapter-instance-keyed: {report:#?}"
    );
    assert_eq!(
        skipped_stages(&report),
        vec![
            BoltV3StartupCheckStage::LiveNodeBuilder,
            BoltV3StartupCheckStage::ClientRegistration,
            BoltV3StartupCheckStage::LiveNodeBuild,
        ],
        "{report:#?}"
    );

    assert_no_resolved_secret_values(&report_text(&report));
}

#[test]
fn startup_check_reports_live_node_build_failure_after_registration() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded
        .root
        .nautilus
        .data_engine
        .time_bars_origins
        .insert("INVALID".to_string(), 1);

    let report = run_bolt_v3_startup_check_with(&loaded, |_| false, support::fake_bolt_v3_resolver);

    assert!(
        statuses_for(&report, BoltV3StartupCheckStage::ClientRegistration)
            .contains(&BoltV3StartupCheckStatus::Satisfied),
        "client registration should remain satisfied before final build failure: {report:#?}"
    );
    assert_eq!(
        statuses_for(&report, BoltV3StartupCheckStage::LiveNodeBuild),
        vec![BoltV3StartupCheckStatus::Failed],
        "{report:#?}"
    );
    assert!(
        report
            .facts
            .iter()
            .all(|fact| fact.status != BoltV3StartupCheckStatus::Skipped),
        "final build failure should not retroactively skip earlier stages: {report:#?}"
    );
    assert_no_resolved_secret_values(&report_text(&report));
}

#[test]
fn startup_check_source_does_not_expose_launch_booleans() {
    let src = source_without_comments_and_strings(include_str!("../src/bolt_v3_readiness.rs"));
    let tokens = identifier_tokens(&src);
    for forbidden in ["entry_ready", "can_trade", "tradable", "ready"] {
        assert!(
            !tokens.iter().any(|token| token == forbidden),
            "src/bolt_v3_readiness.rs must not expose launch boolean token `{forbidden}`"
        );
    }
}

#[test]
fn startup_check_source_does_not_call_trading_apis_directly() {
    let src = source_without_comments_and_strings(include_str!("../src/bolt_v3_readiness.rs"));
    let tokens = identifier_tokens(&src);
    for forbidden in [
        "connect_bolt_v3_clients",
        "disconnect_bolt_v3_clients",
        "submit_order",
        "runner",
        "run_trader",
    ] {
        assert!(
            !tokens.iter().any(|token| token == forbidden),
            "src/bolt_v3_readiness.rs must not contain trade-path token `{forbidden}`"
        );
    }
    assert!(
        !tokens.iter().any(|token| token.starts_with("subscribe_")),
        "src/bolt_v3_readiness.rs must not contain subscribe_* APIs"
    );
}

#[test]
fn source_fence_helper_does_not_treat_lifetimes_as_char_literals() {
    let src = "fn push<'a>(value: &'a str) {}\nfn after() { connect_bolt_v3_clients(); }\n";
    let stripped = source_without_comments_and_strings(src);
    let tokens = identifier_tokens(&stripped);

    assert!(
        tokens
            .iter()
            .any(|token| token == "connect_bolt_v3_clients"),
        "lifetime annotations must not hide later code tokens: {stripped}"
    );
}

#[test]
fn source_fence_helper_blanks_raw_strings_without_corrupting_later_tokens() {
    let src = "const FIXTURE: &[u8] = br##\"runner 'static subscribe_markets\"##;\nfn after() { submit_order(); }\n";
    let stripped = source_without_comments_and_strings(src);
    let tokens = identifier_tokens(&stripped);

    assert!(
        !tokens.iter().any(|token| token == "runner"),
        "raw string body must be blanked before token scanning: {stripped}"
    );
    assert!(
        !tokens.iter().any(|token| token == "subscribe_markets"),
        "raw string body must be blanked before token scanning: {stripped}"
    );
    assert!(
        tokens.iter().any(|token| token == "submit_order"),
        "raw strings must not corrupt token scanning after the literal: {stripped}"
    );
}

#[test]
fn source_fence_helper_blanks_bare_unicode_char_escapes() {
    let src = "const CH: char = '\\u0072';\nfn after() { run_trader(); }\n";
    let stripped = source_without_comments_and_strings(src);
    let tokens = identifier_tokens(&stripped);

    assert!(
        !tokens.iter().any(|token| token == "u0072"),
        "bare unicode char escape must be blanked before token scanning: {stripped}"
    );
    assert!(
        tokens.iter().any(|token| token == "run_trader"),
        "bare unicode char escapes must not corrupt later token scanning: {stripped}"
    );
}

fn identifier_tokens(src: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in src.chars() {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn source_without_comments_and_strings(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut index = 0;

    while index < src.len() {
        let rest = &src[index..];

        if rest.starts_with("//") {
            let end = rest.find('\n').map_or(src.len(), |offset| index + offset);
            push_blanked_source(&mut out, &src[index..end]);
            if end < src.len() {
                out.push('\n');
                index = end + 1;
            } else {
                index = end;
            }
            continue;
        }

        if rest.starts_with("/*") {
            let end = rest
                .find("*/")
                .map_or(src.len(), |offset| index + offset + 2);
            push_blanked_source(&mut out, &src[index..end]);
            index = end;
            continue;
        }

        if let Some(end) = rust_raw_string_literal_end(src, index) {
            push_blanked_source(&mut out, &src[index..end]);
            index = end;
            continue;
        }

        if let Some(end) = rust_string_literal_end(src, index) {
            push_blanked_source(&mut out, &src[index..end]);
            index = end;
            continue;
        }

        if let Some(end) = rust_byte_char_literal_end(src, index) {
            push_blanked_source(&mut out, &src[index..end]);
            index = end;
            continue;
        }

        if let Some(end) = rust_char_literal_end(src, index) {
            push_blanked_source(&mut out, &src[index..end]);
            index = end;
            continue;
        }

        let ch = rest
            .chars()
            .next()
            .expect("index should be inside a valid UTF-8 source boundary");
        out.push(ch);
        index += ch.len_utf8();
    }

    out
}

fn push_blanked_source(out: &mut String, source: &str) {
    out.extend(source.chars().map(|ch| if ch == '\n' { '\n' } else { ' ' }));
}

fn rust_raw_string_literal_end(src: &str, start: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut index = start;

    if src[index..].starts_with("br") {
        index += 2;
    } else if src[index..].starts_with('r') {
        index += 1;
    } else {
        return None;
    }

    let hash_start = index;
    while bytes.get(index) == Some(&b'#') {
        index += 1;
    }
    if bytes.get(index) != Some(&b'"') {
        return None;
    }

    let terminator = format!("\"{}", &src[hash_start..index]);
    index += 1;
    Some(
        src[index..]
            .find(&terminator)
            .map_or(src.len(), |offset| index + offset + terminator.len()),
    )
}

fn rust_string_literal_end(src: &str, start: usize) -> Option<usize> {
    let mut index = if src[start..].starts_with("b\"") {
        start + 2
    } else if src[start..].starts_with('"') {
        start + 1
    } else {
        return None;
    };
    let mut escaped = false;

    while index < src.len() {
        let ch = src[index..]
            .chars()
            .next()
            .expect("index should be inside a valid UTF-8 source boundary");
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(index + ch.len_utf8());
        }
        index += ch.len_utf8();
    }

    Some(src.len())
}

fn rust_byte_char_literal_end(src: &str, start: usize) -> Option<usize> {
    if src[start..].starts_with("b'") {
        rust_char_literal_end(src, start + 1)
    } else {
        None
    }
}

fn rust_char_literal_end(src: &str, start: usize) -> Option<usize> {
    if !src[start..].starts_with('\'') {
        return None;
    }

    let mut index = start + 1;
    let ch = src[index..].chars().next()?;
    if matches!(ch, '\n' | '\r' | '\'') {
        return None;
    }

    if ch == '\\' {
        index += ch.len_utf8();
        let escape = src[index..].chars().next()?;
        if escape == 'u' && src[index + escape.len_utf8()..].starts_with('{') {
            let hex_start = index + escape.len_utf8() + 1;
            let close = src[hex_start..]
                .find('}')
                .map(|offset| hex_start + offset)?;
            let hex = &src[hex_start..close];
            if !(1..=6).contains(&hex.len()) || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return None;
            }
            index = close + 1;
        } else if escape == 'u' {
            let hex_start = index + escape.len_utf8();
            let hex_end = hex_start + 4;
            let hex = src.get(hex_start..hex_end)?;
            if !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return None;
            }
            index = hex_end;
        } else if escape == 'x' {
            let hex_start = index + escape.len_utf8();
            let hex_end = hex_start + 2;
            let hex = src.get(hex_start..hex_end)?;
            if !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return None;
            }
            index = hex_end;
        } else {
            index += escape.len_utf8();
        }
    } else {
        index += ch.len_utf8();
    }

    if src[index..].starts_with('\'') {
        Some(index + 1)
    } else {
        None
    }
}
