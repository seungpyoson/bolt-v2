//! Forbidden credential environment-variable checks for bolt-v3 venues.
//!
//! Per docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md Section 3, every
//! configured venue with a [secrets] block must fail live validation and
//! startup if any canonical credential environment variables for that venue
//! kind are present. The blocklist is owned by the venue-kind handler in
//! bolt code and must be checked before any NautilusTrader client
//! constructor is called.

use crate::bolt_v3_config::{BoltV3RootConfig, VenueKind};

pub fn polymarket_forbidden_env_vars() -> &'static [&'static str] {
    &[
        "POLYMARKET_PK",
        "POLYMARKET_FUNDER",
        "POLYMARKET_API_KEY",
        "POLYMARKET_API_SECRET",
        "POLYMARKET_PASSPHRASE",
    ]
}

pub fn binance_forbidden_env_vars() -> &'static [&'static str] {
    &[
        "BINANCE_ED25519_API_KEY",
        "BINANCE_ED25519_API_SECRET",
        "BINANCE_API_KEY",
        "BINANCE_API_SECRET",
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForbiddenEnvVarFinding {
    pub venue_key: String,
    pub venue_kind: VenueKind,
    pub env_var: &'static str,
}

impl std::fmt::Display for ForbiddenEnvVarFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "venues.{key} (kind={kind}) declares [secrets] but the forbidden credential environment variable `{var}` is set; \
             the bolt-v3 secret contract requires SSM resolution and forbids env-var fallbacks for this venue kind",
            key = self.venue_key,
            kind = self.venue_kind.as_str(),
            var = self.env_var,
        )
    }
}

#[derive(Debug)]
pub struct ForbiddenEnvVarError {
    pub findings: Vec<ForbiddenEnvVarFinding>,
}

impl std::fmt::Display for ForbiddenEnvVarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "bolt-v3 forbidden credential environment variable check failed ({} finding{}):",
            self.findings.len(),
            if self.findings.len() == 1 { "" } else { "s" }
        )?;
        for finding in &self.findings {
            writeln!(f, "  - {finding}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ForbiddenEnvVarError {}

pub fn check_no_forbidden_credential_env_vars(
    config: &BoltV3RootConfig,
) -> Result<(), ForbiddenEnvVarError> {
    check_no_forbidden_credential_env_vars_with(config, |var| std::env::var_os(var).is_some())
}

pub fn check_no_forbidden_credential_env_vars_with<F>(
    config: &BoltV3RootConfig,
    mut env_is_set: F,
) -> Result<(), ForbiddenEnvVarError>
where
    F: FnMut(&str) -> bool,
{
    let mut findings = Vec::new();
    for (key, venue) in &config.venues {
        if venue.secrets.is_none() {
            continue;
        }
        let blocklist: &[&'static str] = match venue.kind {
            VenueKind::Polymarket => polymarket_forbidden_env_vars(),
            VenueKind::Binance => binance_forbidden_env_vars(),
        };
        for env_var in blocklist {
            if env_is_set(env_var) {
                findings.push(ForbiddenEnvVarFinding {
                    venue_key: key.clone(),
                    venue_kind: venue.kind,
                    env_var,
                });
            }
        }
    }

    if findings.is_empty() {
        Ok(())
    } else {
        Err(ForbiddenEnvVarError { findings })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_config::BoltV3RootConfig;

    fn minimal_root_toml() -> &'static str {
        include_str!("../tests/fixtures/bolt_v3/root.toml")
    }

    #[test]
    fn polymarket_blocklist_matches_runtime_contract() {
        assert_eq!(
            polymarket_forbidden_env_vars(),
            &[
                "POLYMARKET_PK",
                "POLYMARKET_FUNDER",
                "POLYMARKET_API_KEY",
                "POLYMARKET_API_SECRET",
                "POLYMARKET_PASSPHRASE",
            ]
        );
    }

    #[test]
    fn binance_blocklist_matches_runtime_contract() {
        assert_eq!(
            binance_forbidden_env_vars(),
            &[
                "BINANCE_ED25519_API_KEY",
                "BINANCE_ED25519_API_SECRET",
                "BINANCE_API_KEY",
                "BINANCE_API_SECRET",
            ]
        );
    }

    #[test]
    fn flags_set_polymarket_var_for_configured_polymarket_venue() {
        let root: BoltV3RootConfig = toml::from_str(minimal_root_toml()).unwrap();
        let error =
            check_no_forbidden_credential_env_vars_with(&root, |var| var == "POLYMARKET_PK")
                .expect_err("POLYMARKET_PK should trip the polymarket blocklist");
        assert_eq!(error.findings.len(), 1);
        assert_eq!(error.findings[0].venue_key, "polymarket_main");
        assert_eq!(error.findings[0].venue_kind, VenueKind::Polymarket);
        assert_eq!(error.findings[0].env_var, "POLYMARKET_PK");
    }

    #[test]
    fn flags_set_binance_var_for_configured_binance_venue() {
        let root: BoltV3RootConfig = toml::from_str(minimal_root_toml()).unwrap();
        let error =
            check_no_forbidden_credential_env_vars_with(&root, |var| var == "BINANCE_API_SECRET")
                .expect_err("BINANCE_API_SECRET should trip the binance blocklist");
        assert_eq!(error.findings.len(), 1);
        assert_eq!(error.findings[0].venue_key, "binance_reference");
        assert_eq!(error.findings[0].venue_kind, VenueKind::Binance);
        assert_eq!(error.findings[0].env_var, "BINANCE_API_SECRET");
    }

    #[test]
    fn passes_when_no_forbidden_var_is_set() {
        let root: BoltV3RootConfig = toml::from_str(minimal_root_toml()).unwrap();
        check_no_forbidden_credential_env_vars_with(&root, |_| false)
            .expect("no forbidden env vars set should pass");
    }
}
