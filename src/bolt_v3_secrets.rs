//! Forbidden credential environment-variable checks and SSM secret
//! resolution for bolt-v3 venues.
//!
//! Per docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md Section 3, every
//! configured venue with a [secrets] block must fail live validation and
//! startup if any canonical credential environment variables for that venue
//! kind are present. The blocklist is owned by the venue-kind handler in
//! bolt code and must be checked before any NautilusTrader client
//! constructor is called.
//!
//! Once the env-var blocklist passes, this module also resolves every
//! configured `[secrets]` block from Amazon Web Services Systems Manager
//! using `[aws].region` as the resolver region. Resolved values are held
//! behind provider-owned handles whose Debug output redacts every secret field; the
//! resolved error type carries venue key, secret-config field, and SSM
//! path context, but never the resolved secret value itself.

use std::collections::BTreeMap;

use crate::{
    bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config, ProviderKey},
    bolt_v3_providers::{
        self, ProviderSecretResolveContext, ResolvedVenueSecrets, SsmSecretResolver,
    },
    secrets::SsmResolverSession,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForbiddenEnvVarFinding {
    pub venue_key: String,
    pub provider_key: ProviderKey,
    pub env_var: &'static str,
}

impl std::fmt::Display for ForbiddenEnvVarFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "venues.{key} (kind={kind}) declares [secrets] but the forbidden credential environment variable `{var}` is set; \
             the bolt-v3 secret contract requires SSM resolution and forbids env-var fallbacks for this venue kind",
            key = self.venue_key,
            kind = self.provider_key.as_str(),
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
        let blocklist = match bolt_v3_providers::binding_for_provider_key(venue.kind.as_str()) {
            Some(binding) => binding.forbidden_env_vars,
            None => &[],
        };
        for env_var in blocklist {
            if env_is_set(env_var) {
                findings.push(ForbiddenEnvVarFinding {
                    venue_key: key.clone(),
                    provider_key: venue.kind.clone(),
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

pub type ResolvedBoltV3VenueSecrets = ResolvedVenueSecrets;

#[derive(Clone)]
pub struct ResolvedBoltV3Secrets {
    pub venues: BTreeMap<String, ResolvedBoltV3VenueSecrets>,
}

impl ResolvedBoltV3Secrets {
    pub fn get_as<T: 'static>(&self, venue_key: &str) -> Option<&T> {
        self.venues
            .get(venue_key)
            .and_then(|secrets| secrets.as_any().downcast_ref())
    }

    pub fn redaction_values(&self) -> Vec<String> {
        let mut values = self
            .venues
            .values()
            .flat_map(|secrets| secrets.redaction_values())
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        values.sort();
        values.dedup();
        values
    }
}

impl std::fmt::Debug for ResolvedBoltV3Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedBoltV3Secrets")
            .field("venues", &self.venues)
            .finish()
    }
}

#[derive(Debug)]
pub struct BoltV3SecretError {
    pub venue_key: String,
    pub field: String,
    pub ssm_path: String,
    pub source: String,
}

impl std::fmt::Display for BoltV3SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.ssm_path.is_empty() {
            write!(
                f,
                "venues.{venue}.secrets.{field}: {source}",
                venue = self.venue_key,
                field = self.field,
                source = self.source,
            )
        } else {
            write!(
                f,
                "venues.{venue}.secrets.{field} (path={path}): {source}",
                venue = self.venue_key,
                field = self.field,
                path = self.ssm_path,
                source = self.source,
            )
        }
    }
}

impl std::error::Error for BoltV3SecretError {}

/// Resolve every configured bolt-v3 venue `[secrets]` block from Amazon Web
/// Services Systems Manager using `[aws].region` and the explicit per-venue
/// SSM paths in the parsed root config. Production startup must use this
/// function; tests should call [`resolve_bolt_v3_secrets_with`] with an
/// injected resolver instead.
///
/// The caller owns the [`SsmResolverSession`] and passes `&session` so a
/// single AWS SDK config and `SsmClient` cache live for the entire bolt-v3
/// startup boundary, not just the bolt-v3 secret-resolution step. The
/// closure passed to [`resolve_bolt_v3_secrets_with`] captures
/// `session.resolve` for that purpose.
pub fn resolve_bolt_v3_secrets(
    session: &SsmResolverSession,
    loaded: &LoadedBoltV3Config,
) -> Result<ResolvedBoltV3Secrets, BoltV3SecretError> {
    resolve_bolt_v3_secrets_with(loaded, |region, path| session.resolve(region, path))
}

/// Test-friendly variant of [`resolve_bolt_v3_secrets`] which lets the caller
/// inject the SSM resolver. The closure is invoked with `(region, ssm_path)`
/// pairs derived from `[aws].region` and the per-venue secret-config paths.
pub fn resolve_bolt_v3_secrets_with<F, E>(
    loaded: &LoadedBoltV3Config,
    mut resolver: F,
) -> Result<ResolvedBoltV3Secrets, BoltV3SecretError>
where
    F: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    let region = loaded.root.aws.region.as_str();
    let mut venues = BTreeMap::new();

    for (venue_key, venue) in &loaded.root.venues {
        match venue.secrets.as_ref() {
            Some(_) => {}
            None => continue,
        }

        let Some(binding) = bolt_v3_providers::binding_for_provider_key(venue.kind.as_str()) else {
            return Err(BoltV3SecretError {
                venue_key: venue_key.clone(),
                field: "kind".to_string(),
                ssm_path: String::new(),
                source: format!(
                    "provider key `{}` is not supported by this build",
                    venue.kind.as_str()
                ),
            });
        };
        let resolved = (binding.resolve_secrets)(
            ProviderSecretResolveContext {
                venue_key,
                region,
                venue,
            },
            &mut resolver,
        )?;
        venues.insert(venue_key.clone(), resolved);
    }

    Ok(ResolvedBoltV3Secrets { venues })
}

pub fn resolve_field(
    venue_key: &str,
    field: &'static str,
    region: &str,
    ssm_path: &str,
    resolver: &mut dyn SsmSecretResolver,
) -> Result<String, BoltV3SecretError> {
    resolver
        .resolve_secret(region, ssm_path)
        .map_err(|error| BoltV3SecretError {
            venue_key: venue_key.to_string(),
            field: field.to_string(),
            ssm_path: ssm_path.to_string(),
            source: error,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config};
    use crate::bolt_v3_providers::{
        binance::{self, ResolvedBoltV3BinanceSecrets},
        polymarket::{self, ResolvedBoltV3PolymarketSecrets},
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
    use std::path::PathBuf;

    fn minimal_root_toml() -> &'static str {
        include_str!("../tests/fixtures/bolt_v3/root.toml")
    }

    fn fixture_loaded_config() -> LoadedBoltV3Config {
        LoadedBoltV3Config {
            root_path: PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            root: toml::from_str(minimal_root_toml()).unwrap(),
            strategies: Vec::new(),
        }
    }

    fn synthetic_binance_secret() -> String {
        // PKCS8-wrapped Ed25519 private key, base64-encoded. Mirrors the
        // shape accepted by `validate_binance_api_secret_shape` so the
        // resolver can run its production validator over this synthetic
        // value without rejecting it.
        let mut der = vec![0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03];
        der.extend_from_slice(&[0x2B, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20]);
        der.extend(0_u8..32);
        BASE64_STANDARD.encode(der)
    }

    fn fake_secret_value(path: &str) -> String {
        match path {
            "/bolt/polymarket_main/private_key" => "poly-private-key".to_string(),
            "/bolt/polymarket_main/api_key" => "poly-api-key".to_string(),
            "/bolt/polymarket_main/api_secret" => "abc".to_string(),
            "/bolt/polymarket_main/passphrase" => "poly-passphrase".to_string(),
            "/bolt/binance_reference/api_key" => "binance-api-key".to_string(),
            "/bolt/binance_reference/api_secret" => synthetic_binance_secret(),
            _ => panic!("unexpected SSM path: {path}"),
        }
    }

    #[test]
    fn polymarket_blocklist_matches_runtime_contract() {
        assert_eq!(
            polymarket::FORBIDDEN_ENV_VARS,
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
            binance::FORBIDDEN_ENV_VARS,
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
        assert_eq!(error.findings[0].provider_key.as_str(), polymarket::KEY);
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
        assert_eq!(error.findings[0].provider_key.as_str(), binance::KEY);
        assert_eq!(error.findings[0].env_var, "BINANCE_API_SECRET");
    }

    #[test]
    fn passes_when_no_forbidden_var_is_set() {
        let root: BoltV3RootConfig = toml::from_str(minimal_root_toml()).unwrap();
        check_no_forbidden_credential_env_vars_with(&root, |_| false)
            .expect("no forbidden env vars set should pass");
    }

    #[test]
    fn resolves_configured_bolt_v3_venue_secrets_from_ssm_paths() {
        let loaded = fixture_loaded_config();
        let mut calls = Vec::new();

        let resolved = resolve_bolt_v3_secrets_with(&loaded, |region, path| {
            calls.push((region.to_string(), path.to_string()));
            Ok::<_, &'static str>(fake_secret_value(path))
        })
        .expect("fixture secrets should resolve");

        assert_eq!(resolved.venues.len(), 2);
        assert!(
            calls.iter().all(|(region, _)| region == "eu-west-1"),
            "all SSM calls must use [aws].region from the fixture root.toml: {calls:#?}"
        );
        for path in [
            "/bolt/polymarket_main/private_key",
            "/bolt/polymarket_main/api_key",
            "/bolt/polymarket_main/api_secret",
            "/bolt/polymarket_main/passphrase",
            "/bolt/binance_reference/api_key",
            "/bolt/binance_reference/api_secret",
        ] {
            assert!(
                calls.iter().any(|(_, called_path)| called_path == path),
                "missing SSM resolution call for {path}: {calls:#?}"
            );
        }

        let polymarket = resolved
            .get_as::<ResolvedBoltV3PolymarketSecrets>("polymarket_main")
            .expect("polymarket_main should resolve to Polymarket secrets");
        assert_eq!(polymarket.private_key, "poly-private-key");
        assert_eq!(polymarket.api_key, "poly-api-key");
        assert_eq!(polymarket.api_secret, "abc=");
        assert_eq!(polymarket.passphrase, "poly-passphrase");

        let binance = resolved
            .get_as::<ResolvedBoltV3BinanceSecrets>("binance_reference")
            .expect("binance_reference should resolve to Binance secrets");
        assert_eq!(binance.api_key, "binance-api-key");
        assert_eq!(binance.api_secret, synthetic_binance_secret());
    }

    #[test]
    fn resolved_bolt_v3_secrets_debug_does_not_leak_secret_values() {
        let loaded = fixture_loaded_config();

        let resolved = resolve_bolt_v3_secrets_with(&loaded, |_, path| {
            Ok::<_, &'static str>(fake_secret_value(path))
        })
        .expect("fixture secrets should resolve");
        let debug = format!("{resolved:?}");

        assert!(debug.contains("polymarket_main"));
        assert!(debug.contains("binance_reference"));
        for secret in [
            "poly-private-key",
            "poly-api-key",
            "poly-passphrase",
            "binance-api-key",
            synthetic_binance_secret().as_str(),
        ] {
            assert!(
                !debug.contains(secret),
                "resolved secret Debug output must not leak secret values"
            );
        }
    }

    #[test]
    fn ssm_failure_reports_bolt_v3_venue_field_and_path() {
        let loaded = fixture_loaded_config();

        let error = resolve_bolt_v3_secrets_with(&loaded, |_, path| {
            if path == "/bolt/binance_reference/api_secret" {
                Err("simulated ssm failure")
            } else {
                Ok(fake_secret_value(path))
            }
        })
        .expect_err("SSM failure should abort resolution");
        let message = error.to_string();

        assert!(
            message.contains("venues.binance_reference.secrets.api_secret_ssm_path"),
            "expected field context in error: {message}"
        );
        assert!(
            message.contains("/bolt/binance_reference/api_secret"),
            "expected SSM path in error: {message}"
        );
        assert!(
            message.contains("simulated ssm failure"),
            "expected resolver error in message: {message}"
        );
    }

    #[test]
    fn resolve_bolt_v3_secrets_takes_session_and_loaded_config() {
        // Per #252 design review: production startup owns the
        // `SsmResolverSession` at the `build_bolt_v3_live_node` boundary
        // and threads it down explicitly, so every top-level `resolve_*`
        // helper has the same shape: caller-owned session passed by
        // reference. Letting `resolve_bolt_v3_secrets` build its own
        // session internally (the prior shape) created an asymmetry —
        // sister resolvers (`resolve_polymarket`, `resolve_chainlink`,
        // `resolve_binance`) take `&SsmResolverSession`, while the
        // bolt-v3 entry point silently constructed and dropped its own,
        // hiding the session lifetime from the caller and preventing
        // future code from sharing one session across both bolt-v3
        // secrets and other startup-side resolution. This guard pins the
        // lifted shape; the test seam remains
        // [`resolve_bolt_v3_secrets_with`].
        fn _assert_signature<F>(_f: F)
        where
            F: Fn(
                &super::SsmResolverSession,
                &LoadedBoltV3Config,
            ) -> Result<super::ResolvedBoltV3Secrets, super::BoltV3SecretError>,
        {
        }
        _assert_signature(super::resolve_bolt_v3_secrets);
    }
}
