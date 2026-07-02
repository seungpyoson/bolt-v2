#![cfg(test)]

use super::*;

#[test]
fn submit_capable_live_node_rejects_ungoverned_boot_without_declaration() {
    let loaded = ungoverned_submit_capable_loaded_config();

    let error = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect_err("submit-capable live boot must reject undeclared ungoverned operation");

    match error {
        BoltV3LiveNodeError::RiskPolicy(error) => {
            let message = error.to_string();
            assert!(
                message.contains("risk.live_submit_governance"),
                "error must name the required declaration: {message}"
            );
            assert!(
                message.contains("submit-capable"),
                "error must identify the active strategy risk: {message}"
            );
            assert!(
                message.contains("capital admission")
                    && message.contains("live-submit approval limits")
                    && message.contains("loss policy"),
                "error must name the missing governance surfaces: {message}"
            );
        }
        other => panic!("expected risk-policy rejection, got {other:?}"),
    }
}

#[test]
fn supervised_deposit_capped_declaration_allows_ungoverned_submit_capable_boot() {
    let mut loaded = ungoverned_submit_capable_loaded_config();
    loaded.root.risk.live_submit_governance =
        Some(crate::bolt_v3_config::LiveSubmitGovernanceBlock {
            mode: crate::bolt_v3_config::LiveSubmitGovernanceMode::SupervisedDepositCapped,
        });

    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
        .expect("explicit supervised deposit-capped declaration should allow boot");

    assert!(
        !runtime.loss_governor_configured(),
        "declaration must not synthesize a loss policy"
    );
    assert!(
        !runtime.capital_admission_configured(),
        "declaration must not synthesize capital admission"
    );
}

fn ungoverned_submit_capable_loaded_config() -> LoadedBoltV3Config {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    assert!(
        !loaded.strategies.is_empty(),
        "fixture must include submit-capable strategies for this boot invariant"
    );
    let catalog_id = NEXT_TEST_CATALOG_ID.fetch_add(1, Ordering::Relaxed);
    loaded.root.persistence.catalog_directory = std::env::temp_dir()
        .join(format!(
            "bolt-v3-governance-mode-test-catalog-{}-{catalog_id}",
            std::process::id()
        ))
        .to_string_lossy()
        .to_string();
    loaded.root.risk.loss_governor = None;
    if let Some(pools) = loaded.root.risk.capital_pools.as_mut() {
        for pool in pools {
            pool.enforce_submit_admission = false;
        }
    }
    loaded
}
