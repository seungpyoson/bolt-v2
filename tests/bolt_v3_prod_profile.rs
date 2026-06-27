//! Tests for the tracked production overlay and its runtime-config
//! composition/verification path (issue #768).
//!
//! The load-bearing guarantee: the production config is a tracked overlay over a
//! shared base template (`config/root.toml`), composed into a complete config that
//! CI loads through the EXACT deployed binary, so it cannot silently drift against
//! the schema the way the former gitignored `config/live.toml` did. The
//! `*_stale_key_*` tests reproduce the 2026-06-18 deploy failure (a stale
//! `nautilus.data_engine.graceful_shutdown_on_error` key) and prove the loader
//! rejects it.
//!
//! Behavior preservation: `composed_config_matches_frozen_legacy_oracle` proves the
//! base+overlay composition produces the SAME effective config as the frozen
//! pre-refactor standalone profile (`tests/fixtures/legacy_prod_btc_5m_oracle.toml`),
//! so the base/overlay refactor changes nothing the runtime sees.

use crate::support;

use std::{collections::BTreeSet, path::Path};

use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_prod_profile::{
        GENERATED_MARKER_PREFIX, ProdOverlay, ProfileError, ProfileId, generate_live_config,
        live_config_path, profile_overlay_path, verify_live_config,
    },
};

/// Repo-local config root used by generation/verification.
const CONFIG_ROOT: &str = "config";
/// Opaque profile ID for the tracked production overlay.
const PROFILE_ID: &str = "prod-btc-5m";
/// The tracked production OVERLAY — the pilot deltas over the shared base template.
const OVERLAY: &str = "config/profiles/prod-btc-5m.overlay.toml";
/// The shared base template the overlay composes onto.
const BASE: &str = "config/root.toml";
/// Frozen pre-refactor standalone production profile, kept ONLY as a regression
/// oracle for the composition (it is intentionally out of `config/` so deploy
/// tooling cannot pick it up as a config).
const LEGACY_ORACLE: &str = "tests/fixtures/legacy_prod_btc_5m_oracle.toml";
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

/// Stage a FULL on-box config tree mirroring `/opt/bolt-v2/config`: the strategies
/// (via [`stage_runtime_dir`]), a copy of the base `root.toml`, and the overlay
/// under `profiles/`. In this single-tree layout the derived overlay, derived
/// `live.toml`, and strategies all live under one config root.
fn stage_full_config_tree(label: &str) -> support::TempCaseDir {
    let dir = stage_runtime_dir(label);
    std::fs::copy(support::repo_path(BASE), dir.path().join("root.toml"))
        .expect("base root.toml copies into the staged tree");
    let profiles = dir.path().join("profiles");
    std::fs::create_dir_all(&profiles).expect("staged profiles dir creates");
    let overlay = profiles.join("prod-btc-5m.overlay.toml");
    std::fs::copy(support::repo_path(OVERLAY), &overlay)
        .expect("overlay copies into the staged tree");
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).expect("test file should write");
}

/// Load a config TEXT by writing it into a staged runtime dir (so its relative
/// `strategy_files` resolve) and loading it through the production loader.
fn load_text_in_staged_dir(label: &str, text: &str) -> LoadedBoltV3Config {
    let dir = stage_runtime_dir(label);
    let path = dir.path().join("live.toml");
    write(&path, text);
    load_bolt_v3_config(&path).expect("staged config text must load against the binary")
}

fn repo_config_root() -> std::path::PathBuf {
    support::repo_path(CONFIG_ROOT)
}

fn repo_config_text(relative: &str) -> String {
    support::repo_text(&format!("{CONFIG_ROOT}/{relative}"))
}

#[test]
fn profile_id_validator_accepts_only_opaque_ids() {
    let max_len_id = format!("p{}", "a".repeat(62));
    for accepted in [PROFILE_ID, "a", "a123", "profile_1", max_len_id.as_str()] {
        assert!(
            ProfileId::parse(accepted).is_ok(),
            "profile id `{accepted}` should be accepted"
        );
    }

    let too_long_id = format!("p{}", "a".repeat(63));
    for rejected in [
        "",
        ".",
        "..",
        "Prod",
        "a/b",
        "a\\b",
        "a..b",
        "live.local",
        "-leading",
        "_leading",
        "live",
        "local",
        "root",
        "profiles",
        "config/profiles/prod-btc-5m.overlay.toml",
        too_long_id.as_str(),
    ] {
        assert!(
            matches!(
                ProfileId::parse(rejected),
                Err(ProfileError::InvalidProfileId(_))
            ),
            "profile id `{rejected}` should be rejected"
        );
    }
}

#[test]
fn profile_id_derives_overlay_path_under_config_root() {
    let dir = support::TempCaseDir::new("prod-profile-id-derived-path");
    let profile_id = ProfileId::parse(PROFILE_ID).expect("repo profile id is valid");
    assert_eq!(
        profile_overlay_path(dir.path(), &profile_id),
        dir.path()
            .join("profiles")
            .join(format!("{PROFILE_ID}.overlay.toml"))
    );
    assert_eq!(live_config_path(dir.path()), dir.path().join("live.toml"));
}

// --- Core CI gate: composition loads against THIS binary ---------------------

#[test]
fn composed_prod_config_loads_against_this_binary() {
    let generated =
        generate_live_config(&repo_config_root(), PROFILE_ID).expect("composition must succeed");
    let loaded = load_text_in_staged_dir("prod-overlay-loads", &generated.text);
    assert!(
        !loaded.config_bundle_checksum.is_empty(),
        "a loaded composed config must produce a config bundle checksum"
    );
}

#[test]
fn composed_prod_config_is_btc_only_with_loss_rails() {
    let generated =
        generate_live_config(&repo_config_root(), PROFILE_ID).expect("composition must succeed");
    let loaded = load_text_in_staged_dir("prod-overlay-btc-only", &generated.text);

    assert_eq!(
        loaded.root.strategy_files,
        vec![BTC_STRATEGY.to_string()],
        "the BTC 5m pilot must select only the BTC strategy"
    );

    let governor = loaded
        .root
        .risk
        .loss_governor
        .as_ref()
        .expect("pilot config must declare [risk.loss_governor]");
    assert!(
        !governor.enabled,
        "pilot loss governor must be explicitly disabled so submit admission skips loss policy"
    );
    assert!(
        governor.max_per_trade_loss.is_some()
            && governor.max_daily_loss.is_some()
            && governor.max_rolling_loss.is_some()
            && governor.max_drawdown.is_some(),
        "configured loss thresholds (per-trade/daily/rolling/drawdown) must all be set"
    );
}

#[test]
fn composed_prod_config_references_only_ssm_secret_paths() {
    // Every credential must be an SSM reference; no inline secret values. Scan the
    // COMPOSED runtime config (what actually deploys), not just the overlay.
    let generated =
        generate_live_config(&repo_config_root(), PROFILE_ID).expect("composition must succeed");
    for line in generated.text.lines() {
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

// --- Behavior preservation: composition == frozen pre-refactor oracle ---------

#[test]
fn composed_config_matches_frozen_legacy_oracle() {
    // The refactor (standalone 593-line profile → base ⊕ typed overlay) must change
    // NOTHING the runtime sees. Prove the composed config and the frozen pre-refactor
    // standalone profile load to byte-for-byte identical effective `BoltV3RootConfig`s.
    let generated =
        generate_live_config(&repo_config_root(), PROFILE_ID).expect("composition must succeed");
    let composed = load_text_in_staged_dir("prod-overlay-equiv-composed", &generated.text);

    let oracle_text = support::repo_text(LEGACY_ORACLE);
    let oracle = load_text_in_staged_dir("prod-overlay-equiv-oracle", &oracle_text);

    assert_eq!(
        composed.root, oracle.root,
        "base ⊕ overlay must produce the SAME effective config as the frozen pre-refactor \
         standalone profile — the refactor changes the config's shape, not its meaning"
    );
    // Strategy selection + contents are part of the effective config too.
    assert_eq!(
        composed.root.strategy_files, oracle.root.strategy_files,
        "composed and oracle must select the same strategy files"
    );
    assert_eq!(
        composed.strategies.len(),
        oracle.strategies.len(),
        "composed and oracle must reference the same number of strategy files"
    );
    for (composed_strategy, oracle_strategy) in
        composed.strategies.iter().zip(oracle.strategies.iter())
    {
        assert_eq!(
            composed_strategy.config, oracle_strategy.config,
            "composed and oracle strategy `{}` must be the same effective strategy config",
            composed_strategy.relative_path
        );
    }
}

// --- Composition is deterministic and provenance-stamped ---------------------

#[test]
fn generate_live_config_is_reproducible() {
    let first = generate_live_config(&repo_config_root(), PROFILE_ID).expect("generate succeeds");
    let second = generate_live_config(&repo_config_root(), PROFILE_ID).expect("generate succeeds");
    assert_eq!(
        first.text, second.text,
        "composition must be deterministic so the deployed artifact is reproducible from \
         overlay ⊕ base byte-for-byte"
    );
    assert_eq!(first.profile_bundle_sha256, second.profile_bundle_sha256);
}

/// `toml::to_string` emits a table's inline entries (`key = ...`, including empty tables
/// `{}` and inline arrays) before its header sections (`[key]` / `[[key]]`) — the TOML
/// format requires inline keys to precede sub-table headers. `sort_toml_value` sorts each
/// group, so a deterministic emission has, per table, the inline keys in sorted order then
/// the section keys in sorted order.
fn is_section_value(value: &toml::Value) -> bool {
    match value {
        // An EMPTY table serializes inline as `{}`, so it is not a header section.
        toml::Value::Table(table) => !table.is_empty(),
        // A non-empty array whose elements are all tables serializes as `[[key]]` sections;
        // anything else (scalars, empty array, mixed) serializes inline.
        toml::Value::Array(items) => {
            !items.is_empty()
                && items
                    .iter()
                    .all(|item| matches!(item, toml::Value::Table(_)))
        }
        _ => false,
    }
}

/// Assert the deterministic key order `sort_toml_value` + TOML serialization guarantee:
/// within every table the inline keys are sorted and the section keys are sorted. A
/// randomized-map iteration (e.g. a `HashMap`) leaking into the compose path would break
/// one of these sorted runs even though a same-process double-generate — which shares one
/// process's iteration order — would not.
fn assert_deterministic_key_order(value: &toml::Value, path: &str) {
    match value {
        toml::Value::Table(table) => {
            // `for (key, child) in table` binds `child` as `&Value` unambiguously (avoids
            // the match-ergonomics surprise of `.filter(|(_, child)| ..)`, which would bind
            // `&&Value`). Partition by how `toml::to_string` emits each entry.
            let mut inline: Vec<&str> = Vec::new();
            let mut sections: Vec<&str> = Vec::new();
            for (key, child) in table {
                if is_section_value(child) {
                    sections.push(key.as_str());
                } else {
                    inline.push(key.as_str());
                }
            }
            let mut inline_sorted = inline.clone();
            inline_sorted.sort_unstable();
            assert_eq!(
                inline, inline_sorted,
                "inline keys of table `{path}` must serialize in sorted order (deterministic composition)"
            );
            let mut sections_sorted = sections.clone();
            sections_sorted.sort_unstable();
            assert_eq!(
                sections, sections_sorted,
                "section keys of table `{path}` must serialize in sorted order (deterministic composition)"
            );
            for (key, child) in table {
                assert_deterministic_key_order(child, &format!("{path}.{key}"));
            }
        }
        toml::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                assert_deterministic_key_order(item, &format!("{path}[{index}]"));
            }
        }
        _ => {}
    }
}

#[test]
fn generated_live_config_is_fully_key_ordered_for_cross_process_determinism() {
    // The verify gate proves a deployed config (composed on one box at deploy time) is
    // byte-identical to a re-composition (on another box at verify time). That
    // cross-process byte-parity holds only if composition is order-deterministic.
    // `generate_live_config_is_reproducible` checks two SAME-process calls agree, which
    // cannot catch a randomized map iteration that differs across processes. Assert the
    // stronger structural invariant that guarantees determinism: every table in the
    // emitted config has sorted inline keys and sorted section keys.
    let generated =
        generate_live_config(&repo_config_root(), PROFILE_ID).expect("generate succeeds");
    // The provenance header is TOML comments, so the whole artifact parses as TOML.
    let parsed: toml::Value =
        toml::from_str(&generated.text).expect("generated runtime config must parse as TOML");
    assert_deterministic_key_order(&parsed, "root");
}

#[test]
fn generated_live_config_carries_provenance_and_loads() {
    let dir = stage_runtime_dir("prod-overlay-provenance");
    let generated =
        generate_live_config(&repo_config_root(), PROFILE_ID).expect("generate succeeds");

    assert!(
        generated.text.starts_with(GENERATED_MARKER_PREFIX),
        "generated runtime config must begin with the provenance marker"
    );
    assert!(
        generated.text.contains(&generated.profile_bundle_sha256),
        "provenance header must record the composed config bundle checksum"
    );

    let deployed = dir.path().join("live.toml");
    write(&deployed, &generated.text);
    let loaded = load_bolt_v3_config(&deployed)
        .expect("generated runtime config (header is comments) must still load against the binary");
    assert_eq!(
        loaded.config_bundle_checksum, generated.profile_bundle_sha256,
        "loading a generated live.toml must preserve the body+strategy checksum recorded in its provenance header"
    );
}

#[test]
fn generate_live_config_accepts_relative_config_root() {
    let generated = generate_live_config(Path::new(CONFIG_ROOT), PROFILE_ID)
        .expect("relative config root works");
    assert!(
        generated.text.starts_with(GENERATED_MARKER_PREFIX),
        "relative config-root generation must still produce a generated runtime config"
    );
}

// --- Verification: byte-equality AND independent load ------------------------

#[test]
fn verify_passes_for_freshly_generated_deployed_config() {
    let dir = stage_full_config_tree("prod-overlay-verify-ok");
    let generated = generate_live_config(dir.path(), PROFILE_ID).expect("generate succeeds");
    let deployed = live_config_path(dir.path());
    write(&deployed, &generated.text);

    let verification = verify_live_config(dir.path(), PROFILE_ID)
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
    let dir = stage_full_config_tree("prod-overlay-tampered");
    let generated = generate_live_config(dir.path(), PROFILE_ID).expect("generate succeeds");
    let deployed = live_config_path(dir.path());
    // Flip a real value (sizing notional) — still valid TOML, but not what the
    // approved overlay composes to.
    let tampered = generated.text.replace(
        "default_max_notional_per_order",
        "default_max_notional_per_order # tampered",
    );
    assert_ne!(tampered, generated.text, "the tamper must change the bytes");
    write(&deployed, &tampered);

    let error = verify_live_config(dir.path(), PROFILE_ID)
        .expect_err("a hand-edited deployed config must fail verification");
    assert!(
        matches!(error, ProfileError::Mismatch(_)),
        "tampered deployed config must be rejected as a profile mismatch, got: {error}"
    );
}

#[test]
fn verify_rejects_a_stale_keyed_deployed_config_as_a_mismatch() {
    // A deployed config carrying the 2026-06-18 stale key differs from the approved
    // bytes, so verify rejects it at the byte-equality check as a profile mismatch.
    // The loader-level staleness proof is binary_rejects_the_2026_06_18_stale_key_directly.
    let dir = stage_full_config_tree("prod-overlay-stale");
    let generated = generate_live_config(dir.path(), PROFILE_ID).expect("generate succeeds");
    let stale = generated.text.replace(
        "[nautilus.data_engine]\n",
        &format!("[nautilus.data_engine]\n{STALE_DEPLOY_KEY} = true\n"),
    );
    assert!(
        stale.contains(STALE_DEPLOY_KEY),
        "the stale key must have been injected"
    );
    assert_ne!(stale, generated.text, "the injection must change the bytes");
    let deployed = live_config_path(dir.path());
    write(&deployed, &stale);

    let error = verify_live_config(dir.path(), PROFILE_ID)
        .expect_err("a stale-keyed deployed config must fail verification");
    assert!(
        matches!(error, ProfileError::Mismatch(_)),
        "a deployed config carrying an injected stale key differs from the approved bytes and \
         must be rejected as a profile mismatch, got: {error}"
    );
}

#[test]
fn binary_rejects_the_2026_06_18_stale_key_directly() {
    // The foundational guarantee #768 relies on: the loader itself rejects the
    // exact stale key, independent of generate/verify. Inject it into the COMPOSED
    // config (the overlay has no [nautilus.data_engine]; the base template owns it).
    let generated =
        generate_live_config(&repo_config_root(), PROFILE_ID).expect("generate succeeds");
    let stale = generated.text.replace(
        "[nautilus.data_engine]\n",
        &format!("[nautilus.data_engine]\n{STALE_DEPLOY_KEY} = true\n"),
    );
    assert!(
        stale.contains(STALE_DEPLOY_KEY),
        "the stale key must have been injected"
    );
    let dir = stage_runtime_dir("prod-overlay-stale-load");
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

// --- The overlay is a typed, delta-only artifact -----------------------------

#[test]
fn overlay_declares_only_the_allowed_delta_keys() {
    // Root IS the single source for shared infrastructure, so drift is structurally
    // impossible — the old `prod_profile_does_not_drift_from_root_shared_blocks` test
    // is obsolete. Its replacement guarantee: the overlay carries ONLY the allowed
    // delta keys (an infrastructure key smuggled in here would be `deny_unknown_fields`
    // rejected at parse, but assert the top-level key set explicitly too).
    let overlay_value: toml::Value =
        toml::from_str(&support::repo_text(OVERLAY)).expect("overlay parses as TOML");
    let table = overlay_value
        .as_table()
        .expect("overlay is a TOML table at the top level");
    let mut keys: Vec<&str> = table.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "active_clients",
            "active_rv_surfaces",
            "client_execution",
            "loss_governor",
            "rv_policy_overrides",
            "strategy_files",
        ],
        "overlay must declare ONLY the allowed delta keys; an unexpected top-level key means \
         shared infrastructure is being duplicated instead of single-sourced in config/root.toml"
    );

    // And it must round-trip into the typed `ProdOverlay` (deny_unknown_fields).
    let overlay: ProdOverlay =
        toml::from_str(&support::repo_text(OVERLAY)).expect("overlay deserializes as ProdOverlay");
    assert_eq!(overlay.strategy_files, vec![BTC_STRATEGY.to_string()]);
    assert!(
        overlay
            .active_clients
            .iter()
            .any(|name| name == "polymarket_main"),
        "overlay must retain the execution client"
    );
    assert_eq!(
        overlay.active_rv_surfaces,
        vec!["btc_usdt_midpoint_rv".to_string()]
    );
}

#[test]
fn base_template_is_present_and_overlay_prunes_shared_clients() {
    // The composition's base must exist and contain shared client infrastructure
    // outside this profile's active subset, so the overlay is genuinely a delta
    // over shared infrastructure (not a second full config).
    let base: toml::Value =
        toml::from_str(&support::repo_text(BASE)).expect("base root.toml parses");
    let overlay: toml::Value =
        toml::from_str(&support::repo_text(OVERLAY)).expect("overlay parses");
    let base_clients = base
        .get("clients")
        .and_then(toml::Value::as_table)
        .expect("base declares clients");
    let active_clients: BTreeSet<&str> = overlay
        .get("active_clients")
        .and_then(toml::Value::as_array)
        .expect("overlay declares active_clients")
        .iter()
        .map(|value| value.as_str().expect("active client names are strings"))
        .collect();
    assert!(
        base_clients
            .keys()
            .any(|name| !active_clients.contains(name.as_str())),
        "base template must include shared clients outside the profile's active subset"
    );
}

// --- Composition / generation fails closed on bad/insufficient overlays -------

#[test]
fn generate_rejects_overlay_with_unknown_field() {
    let dir = stage_full_config_tree("prod-overlay-unknown-field");
    let overlay_text = support::repo_text(OVERLAY);
    let invalid = overlay_text.replace(
        "strategy_files =",
        "bogus_unknown_overlay_key = 1\nstrategy_files =",
    );
    assert_ne!(
        invalid, overlay_text,
        "the unknown key must have been injected"
    );
    let profile_id = ProfileId::parse("prod-bad").expect("test profile id is valid");
    let invalid_path = profile_overlay_path(dir.path(), &profile_id);
    write(&invalid_path, &invalid);

    let error = generate_live_config(dir.path(), profile_id.as_str())
        .expect_err("an unknown overlay field must fail generation closed");
    assert!(
        matches!(error, ProfileError::Overlay { .. }),
        "unknown-field overlay must fail at overlay parse, got: {error}"
    );
}

#[test]
fn generate_rejects_overlay_with_unknown_active_client() {
    // An active_clients name that is not in base.clients must fail composition,
    // never silently drop — fail-closed against a typo selecting a phantom client.
    let dir = stage_full_config_tree("prod-overlay-phantom-client");
    let overlay_text = support::repo_text(OVERLAY);
    let invalid = overlay_text.replace(
        "  \"polymarket_main\",\n",
        "  \"polymarket_main\",\n  \"no_such_client_in_base\",\n",
    );
    assert_ne!(
        invalid, overlay_text,
        "the phantom client must have been injected"
    );
    let profile_id = ProfileId::parse("prod-phantom").expect("test profile id is valid");
    let invalid_path = profile_overlay_path(dir.path(), &profile_id);
    write(&invalid_path, &invalid);

    let error = generate_live_config(dir.path(), profile_id.as_str())
        .expect_err("an active_clients name absent from base must fail composition closed");
    assert!(
        matches!(error, ProfileError::Composition(_)),
        "phantom active_clients name must fail at composition, got: {error}"
    );
}

#[test]
fn generate_rejects_strategy_file_parent_escape_even_when_target_exists() {
    // The selected strategy must come from the reviewed config bundle. A valid
    // TOML file outside <config-root>/strategies must not become a production
    // source just because the overlay and generated live.toml are self-consistent.
    let dir = stage_full_config_tree("prod-overlay-strategy-parent-escape");
    let external_strategy = dir
        .path()
        .parent()
        .expect("staged config root has a parent")
        .join("external-strategy.toml");
    write(&external_strategy, &repo_config_text(BTC_STRATEGY));
    let overlay_text = support::repo_text(OVERLAY);
    let escaped = overlay_text.replace(BTC_STRATEGY, "../external-strategy.toml");
    assert_ne!(escaped, overlay_text, "the strategy path must be replaced");
    let profile_id = ProfileId::parse("prod-escape").expect("test profile id is valid");
    let path = profile_overlay_path(dir.path(), &profile_id);
    write(&path, &escaped);

    let error = generate_live_config(dir.path(), profile_id.as_str())
        .expect_err("strategy_files parent-directory escape must fail generation closed");
    match error {
        ProfileError::Composition(message) => assert!(
            message.contains("strategy_files"),
            "composition error must identify strategy_files, got: {message}"
        ),
        other => panic!("expected a strategy_files composition error, got: {other}"),
    }
}

#[test]
fn generate_rejects_strategy_file_absolute_escape_even_when_target_exists() {
    let dir = stage_full_config_tree("prod-overlay-strategy-absolute-escape");
    let external_strategy = dir
        .path()
        .parent()
        .expect("staged config root has a parent")
        .join("absolute-strategy.toml");
    write(&external_strategy, &repo_config_text(BTC_STRATEGY));
    let overlay_text = support::repo_text(OVERLAY);
    let escaped = overlay_text.replace(BTC_STRATEGY, &external_strategy.display().to_string());
    assert_ne!(escaped, overlay_text, "the strategy path must be replaced");
    let profile_id = ProfileId::parse("prod-absolute").expect("test profile id is valid");
    let path = profile_overlay_path(dir.path(), &profile_id);
    write(&path, &escaped);

    let error = generate_live_config(dir.path(), profile_id.as_str())
        .expect_err("strategy_files absolute path must fail generation closed");
    match error {
        ProfileError::Composition(message) => assert!(
            message.contains("strategy_files"),
            "composition error must identify strategy_files, got: {message}"
        ),
        other => panic!("expected a strategy_files composition error, got: {other}"),
    }
}

#[cfg(unix)]
#[test]
fn generate_rejects_strategy_file_symlink_escape_even_when_entry_is_under_strategies() {
    let dir = stage_full_config_tree("prod-overlay-strategy-symlink-escape");
    let external_strategy = dir
        .path()
        .parent()
        .expect("staged config root has a parent")
        .join("symlink-target-strategy.toml");
    write(&external_strategy, &repo_config_text(BTC_STRATEGY));
    let symlink_strategy = dir.path().join("strategies").join("symlinked.toml");
    std::os::unix::fs::symlink(&external_strategy, &symlink_strategy)
        .expect("strategy symlink should be created");
    let overlay_text = support::repo_text(OVERLAY);
    let escaped = overlay_text.replace(BTC_STRATEGY, "strategies/symlinked.toml");
    assert_ne!(escaped, overlay_text, "the strategy path must be replaced");
    let profile_id = ProfileId::parse("prod-symlink").expect("test profile id is valid");
    let path = profile_overlay_path(dir.path(), &profile_id);
    write(&path, &escaped);

    let error = generate_live_config(dir.path(), profile_id.as_str())
        .expect_err("strategy_files symlink escape must fail generation closed");
    match error {
        ProfileError::Composition(message) => assert!(
            message.contains("strategy_files"),
            "composition error must identify strategy_files, got: {message}"
        ),
        other => panic!("expected a strategy_files composition error, got: {other}"),
    }
}

#[cfg(unix)]
#[test]
fn generate_rejects_symlinked_strategies_directory_even_when_target_exists() {
    let dir = stage_full_config_tree("prod-overlay-strategy-dir-symlink");
    let external_strategies = dir
        .path()
        .parent()
        .expect("staged config root has a parent")
        .join("external-strategies-dir");
    std::fs::create_dir_all(&external_strategies)
        .expect("external strategies dir should be created");
    write(
        &external_strategies.join("binary_oracle_btc.toml"),
        &repo_config_text(BTC_STRATEGY),
    );
    std::fs::remove_dir_all(dir.path().join("strategies"))
        .expect("staged strategies dir should remove");
    std::os::unix::fs::symlink(&external_strategies, dir.path().join("strategies"))
        .expect("strategies dir symlink should be created");

    let error = generate_live_config(dir.path(), PROFILE_ID)
        .expect_err("symlinked strategies directory must fail generation closed");
    match error {
        ProfileError::Composition(message) => assert!(
            message.contains("strategy_files directory") && message.contains("symbolic link"),
            "composition error must identify symlinked strategies dir, got: {message}"
        ),
        other => panic!("expected a symlink strategies-dir composition error, got: {other}"),
    }
}

#[test]
fn generate_rejects_overlay_without_loss_rails() {
    // Removing the overlay's [loss_governor] block makes it fail typed parse
    // (loss_governor is a required ProdOverlay field), failing generation closed.
    let dir = stage_full_config_tree("prod-overlay-no-rails");
    let overlay_text = support::repo_text(OVERLAY);
    let without_rails = without_overlay_loss_governor(&overlay_text);
    assert!(
        !without_rails.contains("[loss_governor]"),
        "the loss_governor block must have been removed for this negative case"
    );
    let profile_id = ProfileId::parse("prod-no-rails").expect("test profile id is valid");
    let path = profile_overlay_path(dir.path(), &profile_id);
    write(&path, &without_rails);

    let error = generate_live_config(dir.path(), profile_id.as_str())
        .expect_err("an overlay without a loss-governor block must fail closed");
    assert!(
        matches!(error, ProfileError::Overlay { .. }),
        "overlay missing the required loss_governor must fail at overlay parse, got: {error}"
    );
}

#[test]
fn generate_allows_overlay_with_disabled_loss_governor() {
    // Disabled loss governor is the explicit production policy for this profile.
    // The composed config still records that state for runtime and operator evidence.
    let dir = stage_full_config_tree("prod-overlay-disabled-rails");
    let overlay_text = support::repo_text(OVERLAY);
    assert!(
        overlay_loss_governor_enabled_is_false(&overlay_text),
        "the profile overlay must explicitly disable loss governor"
    );
    let profile_id = ProfileId::parse("prod-disabled").expect("test profile id is valid");
    let path = profile_overlay_path(dir.path(), &profile_id);
    write(&path, &overlay_text);

    let generated = generate_live_config(dir.path(), profile_id.as_str())
        .expect("an overlay with explicit disabled loss governor must generate");
    let loaded = load_text_in_staged_dir("prod-overlay-disabled-rails-generated", &generated.text);
    let governor = loaded
        .root
        .risk
        .loss_governor
        .as_ref()
        .expect("generated config must retain the loss-governor block");
    assert!(
        !governor.enabled,
        "generated config must preserve explicit disabled loss governor"
    );
}

#[test]
fn generate_rejects_live_local_path_as_profile_source() {
    let dir = stage_runtime_dir("prod-overlay-live-local-rejected");

    let error = generate_live_config(dir.path(), "config/live.local.toml")
        .expect_err("live.local.toml must not be accepted as a profile source");
    assert!(
        matches!(error, ProfileError::InvalidProfileId(_)),
        "legacy live.local.toml path-shaped input must fail before filesystem access, got: {error}"
    );
}

#[test]
fn generate_rejects_an_already_generated_file() {
    let dir = stage_full_config_tree("prod-overlay-double-generate");
    let generated =
        generate_live_config(&repo_config_root(), PROFILE_ID).expect("generate succeeds");
    let profile_id = ProfileId::parse(PROFILE_ID).expect("repo profile id is valid");
    let already = profile_overlay_path(dir.path(), &profile_id);
    write(&already, &generated.text);

    let error = generate_live_config(dir.path(), PROFILE_ID)
        .expect_err("generating from an already-generated file must fail closed");
    assert!(
        matches!(error, ProfileError::AlreadyGenerated(_)),
        "must refuse to double-stamp a generated config, got: {error}"
    );
}

#[test]
fn verify_rejects_divergent_deployed_strategy_file() {
    // Production single-tree layout: the overlay, root, the strategies, and the
    // deployed live.toml all live in ONE config dir (mirroring /opt/bolt-v2/config), so
    // generate and verify resolve strategy paths to the SAME on-box files. A
    // tampered strategy is still rejected — caught by byte-parity, because `generate`
    // re-reads the on-box strategy and the provenance header embeds a checksum over every
    // strategy file's content. This is the genuine differential: with the deployed and
    // approved strategy resolving to one file, only the regenerated-header checksum — not
    // a file-vs-file compare — can detect the tamper.
    let dir = stage_full_config_tree("prod-overlay-divergent-strategy");
    let generated = generate_live_config(dir.path(), PROFILE_ID).expect("generate succeeds");
    let deployed = live_config_path(dir.path());
    write(&deployed, &generated.text);

    // Sanity: the freshly generated config verifies in this tree BEFORE any tamper, so a
    // later rejection is attributable to the tamper, not to the topology.
    verify_live_config(dir.path(), PROFILE_ID)
        .expect("a freshly generated deployed config must verify in the production tree");

    // Tamper the single on-box strategy file (still valid TOML). At re-verify `generate`
    // re-reads it, recomputes a different bundle checksum, and the regenerated header no
    // longer matches the deployed bytes.
    let strategy = dir.path().join("strategies").join("binary_oracle_btc.toml");
    let original = std::fs::read_to_string(&strategy).expect("staged strategy readable");
    write(&strategy, &format!("{original}\n# on-box tamper\n"));

    let error = verify_live_config(dir.path(), PROFILE_ID)
        .expect_err("a divergent on-box strategy file must fail verification");
    assert!(
        matches!(error, ProfileError::Mismatch(_)),
        "a divergent on-box strategy file must be rejected as a mismatch, got: {error}"
    );
}

#[test]
fn deploy_readme_documents_the_full_pre_arm_gate() {
    // #768 step 3 pre-arm duties: config identity (generate/verify), live secret
    // resolution (3c), and arming-gate/readiness (3d). `prestart-check` does config-load +
    // storage only — (3d) is structural (the bot starts disarmed behind the arming gate;
    // data-client readiness is probed separately). The runbook must name all of them so an
    // operator is not told prestart-check verified readiness when it did not.
    let readme = support::repo_text("deploy/README.md");
    for needle in [
        "generate-live-config",
        "verify-live-config",
        "ops launch",
        "secrets resolve",
        "prestart-check",
        "arming",
        "data-client-probe",
    ] {
        assert!(
            readme.contains(needle),
            "deploy/README.md must document `{needle}` in the pre-arm sequence (#768 step 3)"
        );
    }

    for forbidden in [
        "bolt-v2 run --config ...` remains a non-canonical primitive",
        "keeps only its minimal generated-config, invariant, and storage prestart guards",
        "verify-live-config / prestart-check / reference-current-price-health / run",
    ] {
        assert!(
            !readme.contains(forbidden),
            "deploy/README.md must not present the old separate live-arming lane `{forbidden}`"
        );
    }
}

fn overlay_loss_governor_enabled_is_false(overlay_text: &str) -> bool {
    let mut in_loss_governor = false;
    for line in overlay_text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_loss_governor = trimmed == "[loss_governor]";
            continue;
        }
        if in_loss_governor && trimmed == "enabled = false" {
            return true;
        }
    }
    false
}

/// Drop the `[loss_governor]` table (header + its key lines) up to the next
/// section header, leaving an otherwise-valid overlay with no loss-governor block.
fn without_overlay_loss_governor(overlay_text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut skipping = false;
    for line in overlay_text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("[loss_governor]") {
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
