//! Forbidden credential environment-variable checks and SSM secret
//! resolution for bolt-v3 clients.
//!
//! Per docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md Section 3, every
//! configured client must fail live validation and startup if any canonical
//! credential environment variables for that provider are present. The
//! blocklist is owned by the provider handler in bolt code and must be checked
//! before any NautilusTrader client constructor is called.
//!
//! Once the env-var blocklist passes, this module also resolves every
//! configured `[secrets]` block from Amazon Web Services Systems Manager
//! using `[aws].region` as the resolver region. Resolved values are held
//! behind provider-owned handles whose Debug output redacts every secret field; the
//! resolved error type carries client key, secret-config field, and SSM
//! field context, but never the resolved secret value or raw SSM path itself.

use std::collections::BTreeMap;

use zeroize::Zeroizing;

use crate::{
    bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config},
    bolt_v3_providers::{
        self, ProviderSecretResolveContext, ResolvedClientSecrets, SsmSecretResolver,
    },
    secrets::SsmResolverSession,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForbiddenEnvVarFinding {
    pub client_key: String,
    pub provider_key: String,
    pub env_var: &'static str,
}

impl std::fmt::Display for ForbiddenEnvVarFinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "clients.{key} (provider={provider}) has forbidden credential environment variable `{var}` set; \
             the bolt-v3 secret contract requires SSM resolution and forbids env-var fallbacks for this provider",
            key = self.client_key,
            provider = self.provider_key,
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

/// Fails closed if any provider's forbidden credential environment variable is
/// set, so NT can never silently fall back to env-var credentials behind the
/// SSM-only bolt-v3 secret contract.
///
/// Soundness rests on a single-process, single-threaded *startup* invariant:
/// bolt-v3 runs this check once at boot, before any NT client is constructed,
/// and bolt-v3 itself never mutates the process environment. There is a
/// theoretical time-of-check/time-of-use window — another thread or an
/// out-of-process actor could `setenv` a forbidden variable between this check
/// and NT client construction — but the bolt-v3 boot sequence does no
/// concurrent environment mutation, so the window is not reachable in practice.
/// Collapsing it entirely would require resolving credentials and constructing
/// NT clients under a held environment lock, which NT's API does not expose; if
/// bolt-v3 ever introduces concurrent boot work that touches `std::env`, this
/// check must be re-derived against that new ordering.
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
    for (key, client) in &config.clients {
        let blocklist = match bolt_v3_providers::binding_for_provider_key(client.venue.as_str()) {
            Some(binding) => binding.forbidden_env_vars,
            None => &[],
        };
        for env_var in blocklist {
            if env_is_set(env_var) {
                findings.push(ForbiddenEnvVarFinding {
                    client_key: key.clone(),
                    provider_key: client.venue.as_str().to_string(),
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

pub type ResolvedBoltV3ClientSecrets = ResolvedClientSecrets;

#[derive(Clone)]
pub struct ResolvedBoltV3Secrets {
    pub clients: BTreeMap<String, ResolvedBoltV3ClientSecrets>,
}

impl ResolvedBoltV3Secrets {
    pub fn get_as<T: 'static>(&self, client_key: &str) -> Option<&T> {
        self.clients
            .get(client_key)
            .and_then(|secrets| secrets.as_any().downcast_ref())
    }

    pub fn redaction_values(&self) -> Vec<Zeroizing<String>> {
        let mut values = self
            .clients
            .values()
            .flat_map(|secrets| secrets.redaction_values())
            .filter(|value| !value.is_empty())
            .map(|value| Zeroizing::new(value.to_string()))
            .collect::<Vec<_>>();
        values.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        values.dedup_by(|left, right| left.as_str() == right.as_str());
        values
    }
}

impl std::fmt::Debug for ResolvedBoltV3Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedBoltV3Secrets")
            .field("clients", &self.clients)
            .finish()
    }
}

pub struct BoltV3SecretError {
    pub client_key: String,
    pub field: String,
    pub source: String,
}

impl std::fmt::Display for BoltV3SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "clients.{client_key}.secrets.{field}: {source}",
            client_key = self.client_key,
            field = self.field,
            source = self.source,
        )
    }
}

impl std::fmt::Debug for BoltV3SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoltV3SecretError")
            .field("client_key", &self.client_key)
            .field("field", &self.field)
            .field("source", &self.source)
            .finish()
    }
}

impl std::error::Error for BoltV3SecretError {}

/// Resolve every configured bolt-v3 client `[secrets]` block from Amazon Web
/// Services Systems Manager using `[aws].region` and the explicit per-client
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

/// Resolve only one configured bolt-v3 client `[secrets]` block from SSM.
///
/// This is for operator-artifact commands that materialize or preflight one
/// selected live-submit client. Production live-node startup must continue to
/// use [`resolve_bolt_v3_secrets`] so full-runtime signer ownership is checked
/// across every configured secret-bearing client.
pub fn resolve_bolt_v3_client_secrets(
    session: &SsmResolverSession,
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<ResolvedBoltV3Secrets, BoltV3SecretError> {
    resolve_bolt_v3_client_secrets_with(loaded, client_key, |region, path| {
        session.resolve(region, path)
    })
}

/// Test-friendly variant of [`resolve_bolt_v3_secrets`] which lets the caller
/// inject the SSM resolver. The closure is invoked with `(region, ssm_path)`
/// pairs derived from `[aws].region` and the per-client secret-config paths.
pub fn resolve_bolt_v3_secrets_with<F, E>(
    loaded: &LoadedBoltV3Config,
    mut resolver: F,
) -> Result<ResolvedBoltV3Secrets, BoltV3SecretError>
where
    F: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    let region = loaded.root.aws.region.as_str();
    let mut clients = BTreeMap::new();
    let mut exclusive_signer_owners: BTreeMap<(&'static str, String), String> = BTreeMap::new();

    for (client_key, client) in &loaded.root.clients {
        let Some(resolved) =
            resolve_configured_client_secrets(client_key, client, region, &mut resolver)?
        else {
            continue;
        };
        if let Some(owner) = resolved.exclusive_signer_owner() {
            let owner_key = (owner.provider_key, owner.fingerprint.clone());
            if let Some(existing_client_key) = exclusive_signer_owners.get(&owner_key) {
                return Err(BoltV3SecretError {
                    client_key: client_key.clone(),
                    field: "signer_owner".to_string(),
                    source: format!(
                        "provider `{}` signer/API-wallet owner is already assigned to client `{existing_client_key}`; duplicate execution clients sharing one signer are not allowed",
                        owner.provider_key
                    ),
                });
            }
            exclusive_signer_owners.insert(owner_key, client_key.clone());
        }
        clients.insert(client_key.clone(), resolved);
    }

    Ok(ResolvedBoltV3Secrets { clients })
}

/// Test-friendly variant of [`resolve_bolt_v3_client_secrets`] which lets the
/// caller inject the SSM resolver. The closure is invoked only for the selected
/// client's `[secrets]` paths.
pub fn resolve_bolt_v3_client_secrets_with<F, E>(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    mut resolver: F,
) -> Result<ResolvedBoltV3Secrets, BoltV3SecretError>
where
    F: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    let region = loaded.root.aws.region.as_str();
    let client = loaded
        .root
        .clients
        .get(client_key)
        .ok_or_else(|| BoltV3SecretError {
            client_key: client_key.to_string(),
            field: "client".to_string(),
            source: "client is not configured".to_string(),
        })?;
    let mut clients = BTreeMap::new();
    if let Some(resolved) =
        resolve_configured_client_secrets(client_key, client, region, &mut resolver)?
    {
        clients.insert(client_key.to_string(), resolved);
    }
    Ok(ResolvedBoltV3Secrets { clients })
}

fn resolve_configured_client_secrets(
    client_key: &str,
    client: &crate::bolt_v3_config::ClientBlock,
    region: &str,
    resolver: &mut dyn SsmSecretResolver,
) -> Result<Option<ResolvedBoltV3ClientSecrets>, BoltV3SecretError> {
    if client.secrets.is_none() {
        return Ok(None);
    }

    let Some(binding) = bolt_v3_providers::binding_for_provider_key(client.venue.as_str()) else {
        return Err(BoltV3SecretError {
            client_key: client_key.to_string(),
            field: "venue".to_string(),
            source: format!(
                "venue `{}` is not supported by this build",
                client.venue.as_str()
            ),
        });
    };
    (binding.resolve_secrets)(
        ProviderSecretResolveContext {
            client_key,
            region,
            client,
        },
        resolver,
    )
    .map(Some)
}

pub fn resolve_field(
    client_key: &str,
    field: &'static str,
    region: &str,
    ssm_path: &str,
    resolver: &mut dyn SsmSecretResolver,
) -> Result<String, BoltV3SecretError> {
    let value = resolver
        .resolve_secret(region, ssm_path)
        .map_err(|error| BoltV3SecretError {
            client_key: client_key.to_string(),
            field: field.to_string(),
            source: error,
        })?;
    if value.trim().is_empty() {
        return Err(BoltV3SecretError {
            client_key: client_key.to_string(),
            field: field.to_string(),
            source: "resolved SSM value is empty or whitespace-only".to_string(),
        });
    }
    if value.trim() != value {
        return Err(BoltV3SecretError {
            client_key: client_key.to_string(),
            field: field.to_string(),
            source: "resolved SSM value has leading or trailing whitespace".to_string(),
        });
    }
    if value.chars().any(char::is_whitespace) {
        return Err(BoltV3SecretError {
            client_key: client_key.to_string(),
            field: field.to_string(),
            source: "resolved SSM value contains embedded whitespace".to_string(),
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config};
    use crate::bolt_v3_providers::{
        binance::{self, ResolvedBoltV3BinanceSecrets},
        chainlink::ResolvedBoltV3ChainlinkSecrets,
        chainlink_reference::ResolvedBoltV3ChainlinkReferenceSecrets,
        polymarket::{self, ResolvedBoltV3PolymarketSecrets},
        polyresearch::ResolvedBoltV3PolyResearchSecrets,
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
    use std::path::PathBuf;

    const SYNTHETIC_POLYMARKET_PRIVATE_KEY: &str =
        "0x1111111111111111111111111111111111111111111111111111111111111111";

    fn minimal_root_toml() -> &'static str {
        include_str!("../tests/fixtures/bolt_v3/root.toml")
    }

    fn fixture_loaded_config() -> LoadedBoltV3Config {
        LoadedBoltV3Config {
            root_path: PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            config_bundle_checksum: "test-config-bundle-checksum".to_string(),
            root: toml::from_str(minimal_root_toml()).unwrap(),
            strategies: Vec::new(),
        }
    }

    fn binance_reference_client() -> crate::bolt_v3_config::ClientBlock {
        toml::from_str(include_str!(
            "../tests/fixtures/bolt_v3/binance_reference_client.toml"
        ))
        .expect("binance provider fixture client should parse")
    }

    fn bybit_data_client_without_secrets() -> crate::bolt_v3_config::ClientBlock {
        toml::from_str(
            r#"
venue = "BYBIT"

[data]
product_types = ["spot", "linear"]
environment = "testnet"
transport_backend = "sockudo"
"#,
        )
        .expect("bybit data-only fixture client should parse")
    }

    fn fixture_loaded_config_with_binance_reference() -> LoadedBoltV3Config {
        let mut loaded = fixture_loaded_config();
        loaded
            .root
            .clients
            .insert("binance_reference".to_string(), binance_reference_client());
        loaded
    }

    fn synthetic_binance_secret() -> String {
        // PKCS8-wrapped Ed25519 private key, base64-encoded. Mirrors the
        // shape accepted by the Binance provider secret validator, so the
        // resolver can run its provider-owned check over this synthetic
        // value without rejecting it.
        let mut der = vec![0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03];
        der.extend_from_slice(&[0x2B, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20]);
        der.extend(0_u8..32);
        BASE64_STANDARD.encode(der)
    }

    fn fake_secret_value(path: &str) -> String {
        match path {
            "/bolt/polymarket/private-key" => SYNTHETIC_POLYMARKET_PRIVATE_KEY.to_string(),
            "/bolt/polymarket/api-key" => "poly-api-key".to_string(),
            "/bolt/polymarket/api-secret" => "YWJj".to_string(),
            "/bolt/polymarket/api-passphrase" => "poly-passphrase".to_string(),
            "/bolt/binance_reference/api_key" => "binance-api-key".to_string(),
            "/bolt/binance_reference/api_secret" => synthetic_binance_secret(),
            "/bolt/testnet/chainlink/api-key" => "chainlink-api-key".to_string(),
            "/bolt/testnet/chainlink/api-secret" => "chainlink-api-secret".to_string(),
            "/bolt/polyresearch/api-key" => "polyresearch-api-key".to_string(),
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
                "BINANCE_TESTNET_API_KEY",
                "BINANCE_TESTNET_API_SECRET",
                "BINANCE_TESTNET_ED25519_API_KEY",
                "BINANCE_TESTNET_ED25519_API_SECRET",
                "BINANCE_FUTURES_TESTNET_API_KEY",
                "BINANCE_FUTURES_TESTNET_API_SECRET",
                "BINANCE_FUTURES_TESTNET_ED25519_API_KEY",
                "BINANCE_FUTURES_TESTNET_ED25519_API_SECRET",
                "BINANCE_DEMO_API_KEY",
                "BINANCE_DEMO_API_SECRET",
            ]
        );
    }

    #[test]
    fn flags_set_polymarket_var_for_configured_polymarket_client() {
        let root: BoltV3RootConfig = toml::from_str(minimal_root_toml()).unwrap();
        let error =
            check_no_forbidden_credential_env_vars_with(&root, |var| var == "POLYMARKET_PK")
                .expect_err("POLYMARKET_PK should trip the polymarket blocklist");
        assert_eq!(error.findings.len(), 1);
        assert_eq!(error.findings[0].client_key, "polymarket_main");
        assert_eq!(error.findings[0].provider_key, polymarket::KEY);
        assert_eq!(error.findings[0].env_var, "POLYMARKET_PK");
    }

    #[test]
    fn flags_set_binance_var_for_configured_binance_client() {
        let root = fixture_loaded_config_with_binance_reference().root;
        let error =
            check_no_forbidden_credential_env_vars_with(&root, |var| var == "BINANCE_API_SECRET")
                .expect_err("BINANCE_API_SECRET should trip the binance blocklist");
        assert_eq!(error.findings.len(), 1);
        assert_eq!(error.findings[0].client_key, "binance_reference");
        assert_eq!(error.findings[0].provider_key, binance::KEY);
        assert_eq!(error.findings[0].env_var, "BINANCE_API_SECRET");
    }

    #[test]
    fn flags_set_provider_var_for_configured_data_only_client_without_secrets() {
        let mut root: BoltV3RootConfig = toml::from_str(minimal_root_toml()).unwrap();
        root.clients.insert(
            "bybit_data".to_string(),
            bybit_data_client_without_secrets(),
        );

        let error = check_no_forbidden_credential_env_vars_with(&root, |var| {
            var == "BYBIT_TESTNET_API_KEY"
        })
        .expect_err("BYBIT_TESTNET_API_KEY should trip the bybit blocklist");
        assert_eq!(error.findings.len(), 1);
        assert_eq!(error.findings[0].client_key, "bybit_data");
        assert_eq!(error.findings[0].provider_key, "BYBIT");
        assert_eq!(error.findings[0].env_var, "BYBIT_TESTNET_API_KEY");
    }

    #[test]
    fn passes_when_no_forbidden_var_is_set() {
        let root: BoltV3RootConfig = toml::from_str(minimal_root_toml()).unwrap();
        check_no_forbidden_credential_env_vars_with(&root, |_| false)
            .expect("no forbidden env vars set should pass");
    }

    #[test]
    fn resolves_configured_bolt_v3_client_secrets_from_ssm_paths() {
        let loaded = fixture_loaded_config_with_binance_reference();
        let mut calls = Vec::new();

        let resolved = resolve_bolt_v3_secrets_with(&loaded, |region, path| {
            calls.push((region.to_string(), path.to_string()));
            Ok::<_, &'static str>(fake_secret_value(path))
        })
        .expect("fixture secrets should resolve");

        // polymarket_main + binance_reference + chainlink_strike + the two
        // reference-current-price clients from the fixture strategy.
        assert_eq!(resolved.clients.len(), 5);
        assert!(
            calls.iter().all(|(region, _)| region == "eu-west-2"),
            "all SSM calls must use [aws].region from the fixture root.toml: {calls:#?}"
        );
        for path in [
            "/bolt/polymarket/private-key",
            "/bolt/polymarket/api-key",
            "/bolt/polymarket/api-secret",
            "/bolt/polymarket/api-passphrase",
            "/bolt/binance_reference/api_key",
            "/bolt/binance_reference/api_secret",
            "/bolt/testnet/chainlink/api-key",
            "/bolt/testnet/chainlink/api-secret",
            "/bolt/polyresearch/api-key",
        ] {
            assert!(
                calls.iter().any(|(_, called_path)| called_path == path),
                "missing SSM resolution call for {path}: {calls:#?}"
            );
        }

        let polymarket = resolved
            .get_as::<ResolvedBoltV3PolymarketSecrets>("polymarket_main")
            .expect("polymarket_main should resolve to Polymarket secrets");
        assert_eq!(
            polymarket.private_key.as_str(),
            SYNTHETIC_POLYMARKET_PRIVATE_KEY
        );
        assert_eq!(polymarket.api_key.as_str(), "poly-api-key");
        assert_eq!(polymarket.api_secret.as_str(), "YWJj");
        assert_eq!(polymarket.passphrase.as_str(), "poly-passphrase");

        let binance = resolved
            .get_as::<ResolvedBoltV3BinanceSecrets>("binance_reference")
            .expect("binance_reference should resolve to Binance secrets");
        assert_eq!(binance.api_key.as_str(), "binance-api-key");
        assert_eq!(binance.api_secret.as_str(), synthetic_binance_secret());

        let chainlink_strike = resolved
            .get_as::<ResolvedBoltV3ChainlinkSecrets>("chainlink_strike")
            .expect("chainlink_strike should resolve to Chainlink strike secrets");
        assert_eq!(chainlink_strike.api_key.as_str(), "chainlink-api-key");
        assert_eq!(chainlink_strike.api_secret.as_str(), "chainlink-api-secret");

        let chainlink_reference = resolved
            .get_as::<ResolvedBoltV3ChainlinkReferenceSecrets>("chainlink_reference")
            .expect("chainlink_reference should resolve to Chainlink reference-price secrets");
        assert_eq!(chainlink_reference.api_key.as_str(), "chainlink-api-key");
        assert_eq!(
            chainlink_reference.api_secret.as_str(),
            "chainlink-api-secret"
        );

        let polyresearch = resolved
            .get_as::<ResolvedBoltV3PolyResearchSecrets>("polyresearch_reference")
            .expect("polyresearch_reference should resolve to PolyResearch secrets");
        assert_eq!(polyresearch.api_key.as_str(), "polyresearch-api-key");
    }

    #[test]
    fn resolves_only_selected_client_secrets_for_operator_artifacts() {
        let loaded = fixture_loaded_config_with_binance_reference();
        let mut calls = Vec::new();

        let resolved =
            resolve_bolt_v3_client_secrets_with(&loaded, "polymarket_main", |region, path| {
                calls.push((region.to_string(), path.to_string()));
                Ok::<_, &'static str>(fake_secret_value(path))
            })
            .expect("selected client secrets should resolve");

        assert_eq!(resolved.clients.len(), 1);
        assert!(resolved.clients.contains_key("polymarket_main"));
        assert!(!resolved.clients.contains_key("binance_reference"));
        assert!(
            calls.iter().all(|(region, _)| region == "eu-west-2"),
            "selected-client SSM calls must use [aws].region: {calls:#?}"
        );
        for path in [
            "/bolt/polymarket/private-key",
            "/bolt/polymarket/api-key",
            "/bolt/polymarket/api-secret",
            "/bolt/polymarket/api-passphrase",
        ] {
            assert!(
                calls.iter().any(|(_, called_path)| called_path == path),
                "missing selected-client SSM resolution call for {path}: {calls:#?}"
            );
        }
        for path in [
            "/bolt/binance_reference/api_key",
            "/bolt/binance_reference/api_secret",
        ] {
            assert!(
                calls.iter().all(|(_, called_path)| called_path != path),
                "selected-client resolution must not touch unrelated SSM path {path}: {calls:#?}"
            );
        }
    }

    #[test]
    fn rejects_empty_resolved_secret_values_before_nt_can_fall_back_to_env() {
        let loaded = fixture_loaded_config();

        let error = resolve_bolt_v3_secrets_with(&loaded, |_, path| {
            if path == "/bolt/polymarket/api-key" {
                Ok::<_, &'static str>("   ".to_string())
            } else {
                Ok(fake_secret_value(path))
            }
        })
        .expect_err("empty resolved SSM secret value must fail before NT env fallback");

        assert_eq!(error.client_key, "polymarket_main");
        assert_eq!(error.field, "api_key_ssm_path");
        assert_eq!(
            error.source,
            "resolved SSM value is empty or whitespace-only"
        );
    }

    #[test]
    fn rejects_invalid_resolved_polymarket_private_key_shape() {
        let loaded = fixture_loaded_config();

        let error = resolve_bolt_v3_secrets_with(&loaded, |_, path| {
            if path == "/bolt/polymarket/private-key" {
                Ok::<_, &'static str>("not-a-valid-evm-private-key".to_string())
            } else {
                Ok(fake_secret_value(path))
            }
        })
        .expect_err("invalid resolved Polymarket private key must fail before NT client build");

        assert_eq!(error.client_key, "polymarket_main");
        assert_eq!(error.field, "private_key_ssm_path");
        assert!(
            error.source.contains(
                "resolved polymarket private_key is not valid EVM private key material accepted by the NautilusTrader polymarket adapter:"
            ),
            "error should preserve adapter diagnostic detail, got: {}",
            error.source
        );
    }

    #[test]
    fn wrapped_polymarket_private_key_error_does_not_leak_raw_input_bytes() {
        // Per MECE PR #331 P3 round-1 finding P3-NB2: when the
        // resolver wraps the NT EvmPrivateKey validator error, the raw
        // SSM value must not appear in the wrapped error chain. NT's
        // current EvmPrivateKey::new diagnostic does not embed the
        // input, but a future NT revision that included the offending
        // bytes in its error string would propagate them through
        // `BoltV3SecretError::Display` to operator logs. This guard
        // pins the no-leak contract by passing a distinct sentinel
        // value and asserting the sentinel is not a substring of any
        // surface of the wrapped error (`source` field or `Display`
        // output).
        let loaded = fixture_loaded_config();
        let sentinel = "BOLTV3_PRIVATE_KEY_SENTINEL_DO_NOT_LEAK_2BC58A4DE0F1";

        let error = resolve_bolt_v3_secrets_with(&loaded, |_, path| {
            if path == "/bolt/polymarket/private-key" {
                Ok::<_, &'static str>(sentinel.to_string())
            } else {
                Ok(fake_secret_value(path))
            }
        })
        .expect_err(
            "sentinel private_key must fail shape validation before NT client construction",
        );

        assert_eq!(error.field, "private_key_ssm_path");
        assert!(
            !error.source.contains(sentinel),
            "wrapped error source must not include the raw secret bytes; got source: {}",
            error.source
        );
        let displayed = error.to_string();
        assert!(
            !displayed.contains(sentinel),
            "wrapped error Display output must not include the raw secret bytes; got: {displayed}"
        );
    }

    #[test]
    fn wrapped_binance_api_secret_error_does_not_leak_raw_input_bytes() {
        // Per MECE PR #331 P3 round-1 finding P3-NB2: same no-leak
        // contract as
        // `wrapped_polymarket_private_key_error_does_not_leak_raw_input_bytes`,
        // applied to the Binance Ed25519Credential validator wrapper.
        // The sentinel passes resolve_field's whitespace checks but
        // fails Ed25519 PKCS8 base64 shape validation; the wrapped
        // error must not surface the sentinel bytes.
        let loaded = fixture_loaded_config_with_binance_reference();
        let sentinel = "BOLTV3_API_SECRET_SENTINEL_DO_NOT_LEAK_8D4F2E1AC3B7";

        let error = resolve_bolt_v3_secrets_with(&loaded, |_, path| {
            if path == "/bolt/binance_reference/api_secret" {
                Ok::<_, &'static str>(sentinel.to_string())
            } else {
                Ok(fake_secret_value(path))
            }
        })
        .expect_err("sentinel api_secret must fail shape validation before NT client construction");

        assert_eq!(error.field, "api_secret_ssm_path");
        assert!(
            !error.source.contains(sentinel),
            "wrapped error source must not include the raw secret bytes; got source: {}",
            error.source
        );
        let displayed = error.to_string();
        assert!(
            !displayed.contains(sentinel),
            "wrapped error Display output must not include the raw secret bytes; got: {displayed}"
        );
    }

    #[test]
    fn rejects_whitespace_padded_resolved_secret_values_without_trimming() {
        let loaded = fixture_loaded_config();

        let error = resolve_bolt_v3_secrets_with(&loaded, |_, path| {
            if path == "/bolt/polymarket/api-secret" {
                Ok::<_, &'static str>(" YWJj ".to_string())
            } else {
                Ok(fake_secret_value(path))
            }
        })
        .expect_err("SSM secret values must be exact and must not be trimmed in code");

        assert_eq!(error.client_key, "polymarket_main");
        assert_eq!(error.field, "api_secret_ssm_path");
        assert_eq!(
            error.source,
            "resolved SSM value has leading or trailing whitespace"
        );
    }

    #[test]
    fn rejects_embedded_whitespace_resolved_secret_values_without_normalizing() {
        let loaded = fixture_loaded_config();

        let error = resolve_bolt_v3_secrets_with(&loaded, |_, path| {
            if path == "/bolt/polymarket/api-key" {
                Ok::<_, &'static str>("abc def".to_string())
            } else {
                Ok(fake_secret_value(path))
            }
        })
        .expect_err("SSM secret values must be exact and must not be normalized in code");

        assert_eq!(error.client_key, "polymarket_main");
        assert_eq!(error.field, "api_key_ssm_path");
        assert_eq!(
            error.source,
            "resolved SSM value contains embedded whitespace"
        );
    }

    #[test]
    fn resolved_bolt_v3_secrets_debug_does_not_leak_secret_values() {
        let loaded = fixture_loaded_config_with_binance_reference();

        let resolved = resolve_bolt_v3_secrets_with(&loaded, |_, path| {
            Ok::<_, &'static str>(fake_secret_value(path))
        })
        .expect("fixture secrets should resolve");
        let debug = format!("{resolved:?}");

        assert!(debug.contains("polymarket_main"));
        assert!(debug.contains("binance_reference"));
        for secret in [
            SYNTHETIC_POLYMARKET_PRIVATE_KEY,
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
    fn ssm_failure_reports_bolt_v3_client_field_without_path() {
        let loaded = fixture_loaded_config_with_binance_reference();

        let error = resolve_bolt_v3_secrets_with(&loaded, |_, path| {
            if path == "/bolt/binance_reference/api_secret" {
                Err("simulated ssm failure")
            } else {
                Ok(fake_secret_value(path))
            }
        })
        .expect_err("SSM failure should abort resolution");
        let raw_path = "/bolt/binance_reference/api_secret";
        let message = error.to_string();

        assert!(
            message.contains("clients.binance_reference.secrets.api_secret_ssm_path"),
            "expected field context in error: {message}"
        );
        assert!(
            !message.contains(raw_path),
            "SSM failure message must not expose raw path: {message}"
        );
        assert!(
            message.contains("simulated ssm failure"),
            "expected resolver error in message: {message}"
        );

        let debug = format!("{error:?}");
        assert!(
            !debug.contains(raw_path),
            "SSM failure Debug output must not expose raw path: {debug}"
        );
    }

    #[test]
    fn bolt_v3_secret_error_does_not_expose_raw_ssm_path_as_public_api() {
        let source = include_str!("bolt_v3_secrets.rs");
        let forbidden = format!("{} {}", "pub", "ssm_path:");

        assert!(
            !source.contains(&forbidden),
            "BoltV3SecretError must not expose raw SSM paths as a public field"
        );
    }

    #[test]
    fn resolve_bolt_v3_secrets_takes_session_and_loaded_config() {
        // Per #252 design review: production startup owns the
        // `SsmResolverSession` at the `build_bolt_v3_live_node` boundary
        // and threads it down explicitly. Letting
        // `resolve_bolt_v3_secrets` build its own session internally
        // hides the session lifetime from the caller and prevents the
        // startup boundary from sharing one session across all bolt-v3
        // venue secret resolution. This guard pins the lifted shape;
        // tests keep using [`resolve_bolt_v3_secrets_with`].
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
