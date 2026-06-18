//! Tests for the tracked production profile and its runtime-config
//! generation/verification path (issue #768).
//!
//! The load-bearing guarantee: the production config is a tracked artifact that
//! CI loads through the EXACT deployed binary, so it cannot silently drift
//! against the schema the way the former gitignored `config/live.toml` did. The
//! `verify_*_schema_stale_*` test reproduces the 2026-06-18 deploy failure (a
//! stale `nautilus.data_engine.graceful_shutdown_on_error` key) and proves the
//! loader rejects it.

mod support;

use std::path::Path;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_prod_profile::{
        GENERATED_MARKER_PREFIX, ProfileError, generate_live_config, verify_live_config,
    },
};

const PROFILE: &str = "config/prod-btc-5m.toml";
const BTC_STRATEGY: &str = "strategies/binary_oracle_btc.toml";
/// The exact stale key that blocked the 2026-06-18 BTC pilot start at systemd
/// time. The current binary's `NautilusDataEngineBlock` has no such field and is
/// `#[serde(deny_unknown_fields)]`, so any config carrying it must fail to load.
const STALE_DEPLOY_KEY: &str = "graceful_shutdown_on_error";

/// Stage a deployable runtime directory: a copy of every tracked strategy file
/// under `<dir>/strategies/`, so a runtime config written into `<dir>` resolves
/// its relative `strategy_files` exactly as `/opt/bolt-v2/config` does on EC2.
fn stage_runtime_dir(label: &str) -> support::TempCaseDir {
    let dir = support::TempCaseDir::new(label);
    let strategies = dir.path().join("strategies");
    std::fs::create_dir_all(&strategies).expect("staged strategies dir should be created");
    for entry in std::fs::read_dir(support::repo_path("config/strategies"))
        .expect("tracked strategies dir should be readable")
    {
        let path = entry.expect("strategy dir entry should read").path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("toml") {
            let file_name = path.file_name().expect("strategy file should have a name");
            std::fs::copy(&path, strategies.join(file_name)).expect("strategy file should copy");
        }
    }
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).expect("test file should write");
}

// --- Core CI gate: the tracked profile must load against THIS binary ---------

#[test]
fn tracked_prod_profile_loads_against_this_binary() {
    let loaded = load_bolt_v3_config(&support::repo_path(PROFILE))
        .expect("tracked production profile must load against the current binary schema");
    assert!(
        !loaded.config_bundle_checksum.is_empty(),
        "a loaded profile must produce a config bundle checksum"
    );
}

#[test]
fn tracked_prod_profile_is_btc_only_with_loss_rails() {
    let loaded = load_bolt_v3_config(&support::repo_path(PROFILE))
        .expect("tracked production profile must load");

    assert_eq!(
        loaded.root.strategy_files,
        vec![BTC_STRATEGY.to_string()],
        "the BTC 5m pilot profile must select only the BTC strategy"
    );

    let governor = loaded
        .root
        .risk
        .loss_governor
        .as_ref()
        .expect("pilot profile must declare [risk.loss_governor] loss rails");
    assert!(
        governor.max_per_trade_loss.is_some()
            && governor.max_daily_loss.is_some()
            && governor.max_rolling_loss.is_some()
            && governor.max_drawdown.is_some(),
        "pilot loss rails (per-trade/daily/rolling/drawdown) must all be set"
    );
}

#[test]
fn prod_profile_references_only_ssm_secret_paths() {
    // Every credential must be an SSM reference; no inline secret values.
    let text = support::repo_text(PROFILE);
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        let is_secret_field = trimmed.starts_with("private_key")
            || trimmed.starts_with("api_key")
            || trimmed.starts_with("api_secret")
            || trimmed.starts_with("passphrase")
            || trimmed.starts_with("mnemonic")
            || trimmed.starts_with("account_address");
        if is_secret_field {
            assert!(
                trimmed.contains("_ssm_path") || trimmed.contains("_ssm_parameter"),
                "credential field must be an SSM reference, not an inline value: `{trimmed}`"
            );
        }
    }
}

// --- Single source of truth: venue [data] blocks live only in root.toml -------

#[test]
fn prod_profile_shares_root_client_data_blocks() {
    let root: toml::Value =
        toml::from_str(&support::repo_text("config/root.toml")).expect("root.toml parses");
    let profile: toml::Value =
        toml::from_str(&support::repo_text(PROFILE)).expect("profile parses");

    let root_clients = root
        .get("clients")
        .and_then(toml::Value::as_table)
        .expect("root has [clients]");
    let profile_clients = profile
        .get("clients")
        .and_then(toml::Value::as_table)
        .expect("profile has [clients]");

    for (name, profile_client) in profile_clients {
        let root_client = root_clients.get(name).unwrap_or_else(|| {
            panic!("profile client `{name}` must also exist in root.toml (single source for venue blocks)")
        });
        assert_eq!(
            profile_client.get("data"),
            root_client.get("data"),
            "profile client `{name}` [data] block must match root.toml — venue connection \
             details are single-sourced in root; update root.toml, not the profile, when a venue changes"
        );
    }
}

// --- Generation is deterministic and provenance-stamped ----------------------

#[test]
fn generate_live_config_is_reproducible() {
    let first = generate_live_config(&support::repo_path(PROFILE)).expect("generate succeeds");
    let second = generate_live_config(&support::repo_path(PROFILE)).expect("generate succeeds");
    assert_eq!(
        first.text, second.text,
        "generation must be deterministic so the deployed artifact is reproducible from the profile"
    );
    assert_eq!(first.profile_bundle_sha256, second.profile_bundle_sha256);
}

#[test]
fn generated_live_config_carries_provenance_and_loads() {
    let dir = stage_runtime_dir("prod-profile-provenance");
    let generated = generate_live_config(&support::repo_path(PROFILE)).expect("generate succeeds");

    assert!(
        generated.text.starts_with(GENERATED_MARKER_PREFIX),
        "generated runtime config must begin with the provenance marker"
    );
    assert!(
        generated.text.contains(&generated.profile_bundle_sha256),
        "provenance header must record the profile bundle checksum"
    );

    let deployed = dir.path().join("live.toml");
    write(&deployed, &generated.text);
    load_bolt_v3_config(&deployed)
        .expect("generated runtime config (header is comments) must still load against the binary");
}

// --- Verification: byte-equality AND independent load ------------------------

#[test]
fn verify_passes_for_freshly_generated_deployed_config() {
    let dir = stage_runtime_dir("prod-profile-verify-ok");
    let generated = generate_live_config(&support::repo_path(PROFILE)).expect("generate succeeds");
    let deployed = dir.path().join("live.toml");
    write(&deployed, &generated.text);

    let verification = verify_live_config(&support::repo_path(PROFILE), &deployed)
        .expect("a freshly generated deployed config must verify");
    assert!(verification.matches_profile);
    assert!(verification.loads_against_binary);
    assert_eq!(
        verification.invariants.strategy_files,
        vec![BTC_STRATEGY.to_string()]
    );
}

#[test]
fn verify_rejects_tampered_deployed_body() {
    let dir = stage_runtime_dir("prod-profile-tampered");
    let generated = generate_live_config(&support::repo_path(PROFILE)).expect("generate succeeds");
    let deployed = dir.path().join("live.toml");
    // Flip a real value (sizing notional) — still valid TOML, but not what the
    // approved profile generates.
    let tampered = generated
        .text
        .replace("default_max_notional_per_order", "default_max_notional_per_order # tampered");
    assert_ne!(tampered, generated.text, "the tamper must change the bytes");
    write(&deployed, &tampered);

    let error = verify_live_config(&support::repo_path(PROFILE), &deployed)
        .expect_err("a hand-edited deployed config must fail verification");
    assert!(
        matches!(error, ProfileError::Mismatch(_)),
        "tampered deployed config must be rejected as a profile mismatch, got: {error}"
    );
}

#[test]
fn verify_rejects_schema_stale_deployed_config() {
    // Reproduce the 2026-06-18 failure mode: a deployed config carrying a stale
    // key the binary no longer accepts. It must be rejected, not started.
    let dir = stage_runtime_dir("prod-profile-stale");
    let generated = generate_live_config(&support::repo_path(PROFILE)).expect("generate succeeds");
    let stale = generated.text.replace(
        "[nautilus.data_engine]\n",
        &format!("[nautilus.data_engine]\n{STALE_DEPLOY_KEY} = true\n"),
    );
    assert!(
        stale.contains(STALE_DEPLOY_KEY),
        "the stale key must have been injected"
    );
    let deployed = dir.path().join("live.toml");
    write(&deployed, &stale);

    let error = verify_live_config(&support::repo_path(PROFILE), &deployed)
        .expect_err("a schema-stale deployed config must fail verification");
    assert!(
        matches!(error, ProfileError::Mismatch(_) | ProfileError::Load { .. }),
        "schema-stale deployed config must be rejected, got: {error}"
    );
}

#[test]
fn binary_rejects_the_2026_06_18_stale_key_directly() {
    // The foundational guarantee #768 relies on: the loader itself rejects the
    // exact stale key, independent of generate/verify.
    let dir = stage_runtime_dir("prod-profile-stale-load");
    let profile_text = support::repo_text(PROFILE);
    let stale = profile_text.replace(
        "[nautilus.data_engine]\n",
        &format!("[nautilus.data_engine]\n{STALE_DEPLOY_KEY} = true\n"),
    );
    let stale_path = dir.path().join("live.toml");
    write(&stale_path, &stale);

    let error = load_bolt_v3_config(&stale_path)
        .expect_err("a config with the stale 2026-06-18 key must fail to load");
    let message = error.to_string();
    assert!(
        message.contains(STALE_DEPLOY_KEY),
        "the load error must name the unknown stale key, got: {message}"
    );
}

// --- Generation fails closed on bad/insufficient profiles ---------------------

#[test]
fn generate_rejects_profile_with_unknown_field() {
    let dir = stage_runtime_dir("prod-profile-unknown-field");
    let profile_text = support::repo_text(PROFILE);
    let invalid = profile_text.replace(
        "schema_version = 1\n",
        "schema_version = 1\nbogus_unknown_top_level_key = 1\n",
    );
    let invalid_path = dir.path().join("prod-bad.toml");
    write(&invalid_path, &invalid);

    let error = generate_live_config(&invalid_path)
        .expect_err("an unknown field must fail generation closed");
    assert!(
        matches!(error, ProfileError::Load { .. }),
        "unknown-field profile must fail at load, got: {error}"
    );
}

#[test]
fn generate_rejects_profile_without_loss_rails() {
    let dir = stage_runtime_dir("prod-profile-no-rails");
    let profile_text = support::repo_text(PROFILE);
    let without_rails = without_loss_governor(&profile_text);
    assert!(
        !without_rails.contains("[risk.loss_governor]"),
        "the loss_governor block must have been removed for this negative case"
    );
    let path = dir.path().join("prod-no-rails.toml");
    write(&path, &without_rails);

    let error =
        generate_live_config(&path).expect_err("a profile without loss rails must fail closed");
    match error {
        ProfileError::Invariant(message) => assert!(
            message.contains("loss_governor"),
            "invariant error must point at the missing loss rails, got: {message}"
        ),
        other => panic!("expected an invariant error, got: {other}"),
    }
}

#[test]
fn generate_rejects_an_already_generated_file() {
    let dir = stage_runtime_dir("prod-profile-double-generate");
    let generated = generate_live_config(&support::repo_path(PROFILE)).expect("generate succeeds");
    let already = dir.path().join("live.toml");
    write(&already, &generated.text);

    let error = generate_live_config(&already)
        .expect_err("generating from an already-generated file must fail closed");
    assert!(
        matches!(error, ProfileError::AlreadyGenerated(_)),
        "must refuse to double-stamp a generated config, got: {error}"
    );
}

/// Drop the `[risk.loss_governor]` table (header + its key lines) up to the next
/// section header, leaving an otherwise-valid profile with no loss rails.
fn without_loss_governor(profile_text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut skipping = false;
    for line in profile_text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("[risk.loss_governor]") {
            skipping = true;
            continue;
        }
        if skipping {
            if trimmed.starts_with('[') {
                skipping = false;
            } else {
                continue;
            }
        }
        out.push(line);
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}
