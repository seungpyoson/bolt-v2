mod support;

use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_readiness::{
        BoltV3StartupCheckReport, BoltV3StartupCheckStage, BoltV3StartupCheckStatus,
        run_bolt_v3_startup_check_with,
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
}

#[test]
fn startup_check_reports_empty_venue_stages_as_satisfied_root_facts() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let empty_loaded = LoadedBoltV3Config {
        root_path: loaded.root_path.clone(),
        root: BoltV3RootConfig {
            venues: BTreeMap::new(),
            ..loaded.root
        },
        strategies: Vec::new(),
    };
    let resolver = |_region: &str, _path: &str| -> Result<String, &'static str> {
        Err("resolver must not be called when no venues are configured")
    };

    let report = run_bolt_v3_startup_check_with(&empty_loaded, |_| false, resolver);

    for stage in [
        BoltV3StartupCheckStage::SecretResolution,
        BoltV3StartupCheckStage::AdapterMapping,
        BoltV3StartupCheckStage::ClientRegistration,
    ] {
        assert_eq!(
            statuses_for(&report, stage),
            vec![BoltV3StartupCheckStatus::Satisfied],
            "{stage:?} should be explicitly satisfied at root level for empty venue configs: {report:#?}"
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
    assert!(text.contains("venues.polymarket_main.secrets.private_key"));
    assert!(text.contains("/bolt/polymarket_main/private_key"));
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
        .venues
        .get_mut("polymarket_main")
        .expect("fixture polymarket_main venue should exist")
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
    assert_eq!(
        skipped_stages(&report),
        vec![
            BoltV3StartupCheckStage::LiveNodeBuilder,
            BoltV3StartupCheckStage::ClientRegistration,
            BoltV3StartupCheckStage::LiveNodeBuild,
        ],
        "{report:#?}"
    );

    let text = report_text(&report);
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
fn startup_check_source_remains_no_trade() {
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
    let mut chars = src.chars().peekable();
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut in_string = false;
    let mut in_char = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
                out.push('\n');
            } else {
                out.push(' ');
            }
            continue;
        }
        if in_block_comment {
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block_comment = false;
                out.push(' ');
                out.push(' ');
            } else {
                out.push(if ch == '\n' { '\n' } else { ' ' });
            }
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            out.push(if ch == '\n' { '\n' } else { ' ' });
            continue;
        }
        if in_char {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '\'' {
                in_char = false;
            }
            out.push(if ch == '\n' { '\n' } else { ' ' });
            continue;
        }

        if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            in_line_comment = true;
            out.push(' ');
            out.push(' ');
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            in_block_comment = true;
            out.push(' ');
            out.push(' ');
        } else if ch == '"' {
            in_string = true;
            out.push(' ');
        } else if ch == '\'' {
            in_char = true;
            out.push(' ');
        } else {
            out.push(ch);
        }
    }

    out
}
