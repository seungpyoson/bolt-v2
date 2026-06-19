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

mod support;

use std::path::Path;

use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_prod_profile::{
        GENERATED_MARKER_PREFIX, ProdOverlay, ProfileError, generate_live_config,
        verify_live_config,
    },
};

/// The tracked production OVERLAY — the pilot deltas over the shared base template.
const OVERLAY: &str = "config/profiles/prod-btc-5m.overlay.toml";
/// The shared multi-asset base template the overlay composes onto.
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

// --- Core CI gate: composition loads against THIS binary ---------------------

#[test]
fn composed_prod_config_loads_against_this_binary() {
    let generated =
        generate_live_config(&support::repo_path(OVERLAY)).expect("composition must succeed");
    let loaded = load_text_in_staged_dir("prod-overlay-loads", &generated.text);
    assert!(
        !loaded.config_bundle_checksum.is_empty(),
        "a loaded composed config must produce a config bundle checksum"
    );
}

#[test]
fn composed_prod_config_is_btc_only_with_loss_rails() {
    let generated =
        generate_live_config(&support::repo_path(OVERLAY)).expect("composition must succeed");
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
        .expect("pilot config must declare [risk.loss_governor] loss rails");
    assert!(
        governor.enabled,
        "pilot loss rails must be ENABLED — present-but-disabled rails are inert at runtime"
    );
    assert!(
        governor.max_per_trade_loss.is_some()
            && governor.max_daily_loss.is_some()
            && governor.max_rolling_loss.is_some()
            && governor.max_drawdown.is_some(),
        "pilot loss rails (per-trade/daily/rolling/drawdown) must all be set"
    );
}

#[test]
fn composed_prod_config_references_only_ssm_secret_paths() {
    // Every credential must be an SSM reference; no inline secret values. Scan the
    // COMPOSED runtime config (what actually deploys), not just the overlay.
    let generated =
        generate_live_config(&support::repo_path(OVERLAY)).expect("composition must succeed");
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
        generate_live_config(&support::repo_path(OVERLAY)).expect("composition must succeed");
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
    let first = generate_live_config(&support::repo_path(OVERLAY)).expect("generate succeeds");
    let second = generate_live_config(&support::repo_path(OVERLAY)).expect("generate succeeds");
    assert_eq!(
        first.text, second.text,
        "composition must be deterministic so the deployed artifact is reproducible from \
         overlay ⊕ base byte-for-byte"
    );
    assert_eq!(first.profile_bundle_sha256, second.profile_bundle_sha256);
}

#[test]
fn generated_live_config_carries_provenance_and_loads() {
    let dir = stage_runtime_dir("prod-overlay-provenance");
    let generated = generate_live_config(&support::repo_path(OVERLAY)).expect("generate succeeds");

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
    load_bolt_v3_config(&deployed)
        .expect("generated runtime config (header is comments) must still load against the binary");
}

// --- Verification: byte-equality AND independent load ------------------------

#[test]
fn verify_passes_for_freshly_generated_deployed_config() {
    let dir = stage_runtime_dir("prod-overlay-verify-ok");
    let generated = generate_live_config(&support::repo_path(OVERLAY)).expect("generate succeeds");
    let deployed = dir.path().join("live.toml");
    write(&deployed, &generated.text);

    let verification = verify_live_config(&support::repo_path(OVERLAY), &deployed)
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
    let dir = stage_runtime_dir("prod-overlay-tampered");
    let generated = generate_live_config(&support::repo_path(OVERLAY)).expect("generate succeeds");
    let deployed = dir.path().join("live.toml");
    // Flip a real value (sizing notional) — still valid TOML, but not what the
    // approved overlay composes to.
    let tampered = generated.text.replace(
        "default_max_notional_per_order",
        "default_max_notional_per_order # tampered",
    );
    assert_ne!(tampered, generated.text, "the tamper must change the bytes");
    write(&deployed, &tampered);

    let error = verify_live_config(&support::repo_path(OVERLAY), &deployed)
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
    let dir = stage_runtime_dir("prod-overlay-stale");
    let generated = generate_live_config(&support::repo_path(OVERLAY)).expect("generate succeeds");
    let stale = generated.text.replace(
        "[nautilus.data_engine]\n",
        &format!("[nautilus.data_engine]\n{STALE_DEPLOY_KEY} = true\n"),
    );
    assert!(
        stale.contains(STALE_DEPLOY_KEY),
        "the stale key must have been injected"
    );
    assert_ne!(stale, generated.text, "the injection must change the bytes");
    let deployed = dir.path().join("live.toml");
    write(&deployed, &stale);

    let error = verify_live_config(&support::repo_path(OVERLAY), &deployed)
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
    let generated = generate_live_config(&support::repo_path(OVERLAY)).expect("generate succeeds");
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
            "base",
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
    assert_eq!(
        overlay.base, "../root.toml",
        "overlay base must point at the shared template relative to the overlay dir"
    );
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
fn base_template_is_present_and_multi_asset() {
    // The composition's base must exist and be the broad multi-asset template, so the
    // overlay is genuinely a delta over shared infrastructure (not a second full config).
    let base: toml::Value =
        toml::from_str(&support::repo_text(BASE)).expect("base root.toml parses");
    let strategy_files = base
        .get("strategy_files")
        .and_then(toml::Value::as_array)
        .expect("base declares strategy_files");
    assert!(
        strategy_files.len() > 1,
        "base template must be multi-asset (more than the single BTC pilot strategy)"
    );
}

// --- Composition / generation fails closed on bad/insufficient overlays -------

#[test]
fn generate_rejects_overlay_with_unknown_field() {
    let dir = stage_runtime_dir("prod-overlay-unknown-field");
    let overlay_text = support::repo_text(OVERLAY);
    let invalid = overlay_text.replace(
        "base = \"../root.toml\"\n",
        "base = \"../root.toml\"\nbogus_unknown_overlay_key = 1\n",
    );
    assert_ne!(
        invalid, overlay_text,
        "the unknown key must have been injected"
    );
    let invalid_path = dir.path().join("prod-bad.overlay.toml");
    write(&invalid_path, &invalid);

    let error = generate_live_config(&invalid_path)
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
    let dir = stage_runtime_dir("prod-overlay-phantom-client");
    let overlay_text = support::repo_text(OVERLAY);
    let invalid = overlay_text.replace(
        "  \"polymarket_main\",\n",
        "  \"polymarket_main\",\n  \"no_such_client_in_base\",\n",
    );
    assert_ne!(
        invalid, overlay_text,
        "the phantom client must have been injected"
    );
    // Write into a dir that still resolves the base (the overlay base is `../root.toml`,
    // so the overlay must sit one level under a dir whose parent has root.toml).
    let composed_dir = stage_runtime_dir("prod-overlay-phantom-client-base");
    std::fs::copy(
        support::repo_path(BASE),
        composed_dir.path().join("root.toml"),
    )
    .expect("base copies next to the overlay's resolved base");
    let profiles = composed_dir.path().join("profiles");
    std::fs::create_dir_all(&profiles).expect("overlay profiles dir creates");
    let invalid_path = profiles.join("prod-phantom.overlay.toml");
    write(&invalid_path, &invalid);

    let error = generate_live_config(&invalid_path)
        .expect_err("an active_clients name absent from base must fail composition closed");
    assert!(
        matches!(error, ProfileError::Composition(_)),
        "phantom active_clients name must fail at composition, got: {error}"
    );
    let _ = dir;
}

#[test]
fn generate_rejects_overlay_without_loss_rails() {
    // Removing the overlay's [loss_governor] block makes it fail typed parse
    // (loss_governor is a required ProdOverlay field), failing generation closed.
    let dir = stage_runtime_dir("prod-overlay-no-rails");
    let overlay_text = support::repo_text(OVERLAY);
    let without_rails = without_overlay_loss_governor(&overlay_text);
    assert!(
        !without_rails.contains("[loss_governor]"),
        "the loss_governor block must have been removed for this negative case"
    );
    let path = dir.path().join("prod-no-rails.overlay.toml");
    write(&path, &without_rails);

    let error =
        generate_live_config(&path).expect_err("an overlay without loss rails must fail closed");
    assert!(
        matches!(error, ProfileError::Overlay { .. }),
        "overlay missing the required loss_governor must fail at overlay parse, got: {error}"
    );
}

#[test]
fn generate_rejects_overlay_with_disabled_loss_rails() {
    // Present-but-disabled rails parse fine but are inert at runtime, so the
    // production invariant on the COMPOSED config must reject them.
    let dir = stage_runtime_dir("prod-overlay-disabled-rails");
    // Compose onto a base copied next to the overlay so the disabled overlay still resolves.
    let base_root = dir.path().join("root.toml");
    std::fs::copy(support::repo_path(BASE), &base_root)
        .expect("base copies for disabled-rails case");
    let strategies = dir.path().join("strategies");
    std::fs::create_dir_all(&strategies).expect("staged strategies dir");
    for entry in std::fs::read_dir(support::repo_path("config/strategies")).expect("strategies dir")
    {
        let path = entry.expect("strategy dir entry").path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
            let name = path.file_name().expect("strategy name");
            std::fs::copy(&path, strategies.join(name)).expect("strategy copies");
        }
    }
    let profiles = dir.path().join("profiles");
    std::fs::create_dir_all(&profiles).expect("profiles dir");
    let overlay_text = support::repo_text(OVERLAY);
    let disabled = overlay_text.replace("enabled = true\n", "enabled = false\n");
    assert_ne!(
        disabled, overlay_text,
        "the enabled flag must have been flipped to false"
    );
    let path = profiles.join("prod-disabled.overlay.toml");
    write(&path, &disabled);

    let error = generate_live_config(&path)
        .expect_err("an overlay with present-but-disabled loss rails must fail closed");
    match error {
        ProfileError::Invariant(message) => assert!(
            message.contains("enabled"),
            "invariant error must point at the disabled rails, got: {message}"
        ),
        other => panic!("expected an invariant error, got: {other}"),
    }
}

#[test]
fn generate_rejects_an_already_generated_file() {
    let dir = stage_runtime_dir("prod-overlay-double-generate");
    let generated = generate_live_config(&support::repo_path(OVERLAY)).expect("generate succeeds");
    let already = dir.path().join("live.toml");
    write(&already, &generated.text);

    let error = generate_live_config(&already)
        .expect_err("generating from an already-generated file must fail closed");
    assert!(
        matches!(error, ProfileError::AlreadyGenerated(_)),
        "must refuse to double-stamp a generated config, got: {error}"
    );
}

#[test]
fn verify_rejects_divergent_deployed_strategy_file() {
    let dir = stage_runtime_dir("prod-overlay-divergent-strategy");
    let generated = generate_live_config(&support::repo_path(OVERLAY)).expect("generate succeeds");
    let deployed = dir.path().join("live.toml");
    write(&deployed, &generated.text);
    // The deployed live.toml bytes are unchanged; only the referenced strategy file is
    // tampered (still valid TOML). The byte-compare cannot see this — only verify's
    // explicit strategy-content comparison can.
    let strategy = dir.path().join("strategies").join("binary_oracle_btc.toml");
    let original = std::fs::read_to_string(&strategy).expect("staged strategy readable");
    write(&strategy, &format!("{original}\n# deployed-side tamper\n"));

    let error = verify_live_config(&support::repo_path(OVERLAY), &deployed)
        .expect_err("a deployed strategy file diverging from the profile must fail verification");
    assert!(
        matches!(error, ProfileError::Mismatch(_)),
        "a divergent deployed strategy file must be rejected as a mismatch, got: {error}"
    );
}

#[test]
fn deploy_readme_documents_the_full_pre_arm_gate() {
    // #768 step 3 pre-arm duties: config identity (generate/verify), live secret
    // resolution (3c), and no-submit/readiness (3d). `prestart-check` does config-load +
    // storage only — (3d) is structural (the bot starts disarmed behind the arming gate;
    // data-client readiness is probed separately). The runbook must name all of them so an
    // operator is not told prestart-check verified readiness when it did not.
    let readme = support::repo_text("deploy/README.md");
    for needle in [
        "generate-live-config",
        "verify-live-config",
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
}

/// Drop the `[loss_governor]` table (header + its key lines) up to the next
/// section header, leaving an otherwise-valid overlay with no loss rails.
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
