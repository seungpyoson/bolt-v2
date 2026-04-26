use aws_config::BehaviorVersion;
use aws_sdk_ssm::{Client as SsmClient, config::Region};
use nautilus_binance::common::credential::Ed25519Credential;

use crate::config::{BinanceSharedConfig, ChainlinkSharedConfig, ExecClientSecrets};

#[derive(Debug)]
pub struct SecretError(String);

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for SecretError {}

struct RedactedDebug;

impl std::fmt::Debug for RedactedDebug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Clone)]
pub struct ResolvedPolymarketSecrets {
    pub private_key: String,
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: String,
}

#[derive(Clone)]
pub struct ResolvedChainlinkSecrets {
    pub api_key: String,
    pub api_secret: String,
}

#[derive(Clone)]
pub struct ResolvedBinanceSecrets {
    pub api_key: String,
    pub api_secret: String,
}

impl std::fmt::Debug for ResolvedPolymarketSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = RedactedDebug;

        f.debug_struct("ResolvedPolymarketSecrets")
            .field("private_key", &redacted)
            .field("api_key", &redacted)
            .field("api_secret", &redacted)
            .field("passphrase", &redacted)
            .finish()
    }
}

impl std::fmt::Debug for ResolvedChainlinkSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = RedactedDebug;

        f.debug_struct("ResolvedChainlinkSecrets")
            .field("api_key", &redacted)
            .field("api_secret", &redacted)
            .finish()
    }
}

impl std::fmt::Debug for ResolvedBinanceSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = RedactedDebug;

        f.debug_struct("ResolvedBinanceSecrets")
            .field("api_key", &redacted)
            .field("api_secret", &redacted)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretConfigCheck {
    pub present: Vec<&'static str>,
    pub missing: Vec<&'static str>,
}

impl SecretConfigCheck {
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }
}

pub(crate) struct BinanceSecretConfigContract<'a> {
    pub region: &'a str,
    pub api_key_path: &'a str,
    pub api_secret_path: &'a str,
}

pub(crate) fn binance_secret_config_contract(
    shared: &BinanceSharedConfig,
) -> BinanceSecretConfigContract<'_> {
    BinanceSecretConfigContract {
        region: &shared.region,
        api_key_path: &shared.api_key,
        api_secret_path: &shared.api_secret,
    }
}

pub(crate) fn resolve_secret(region: &str, ssm_path: &str) -> Result<String, SecretError> {
    let region_owned = region.to_string();
    let ssm_path_owned = ssm_path.to_string();
    // Production startup is synchronous (see `fn main` in src/main.rs and
    // `bolt_v3_live_node::build_bolt_v3_live_node`), so the AWS SDK's async
    // GetParameter call is bridged through a contained current-thread Tokio
    // runtime here rather than propagating async-ness through every caller.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            SecretError(format!(
                "failed to build Tokio runtime for SSM resolution at {ssm_path_owned}: {error}"
            ))
        })?;
    runtime.block_on(async move {
        let aws_config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region_owned))
            .load()
            .await;
        let client = SsmClient::new(&aws_config);
        let response = client
            .get_parameter()
            .name(&ssm_path_owned)
            .with_decryption(true)
            .send()
            .await
            .map_err(|error| {
                SecretError(format!(
                    "AWS SSM GetParameter failed for {ssm_path_owned}: {}",
                    aws_sdk_ssm::error::DisplayErrorContext(&error),
                ))
            })?;
        response
            .parameter()
            .and_then(|parameter| parameter.value())
            .map(|raw| raw.trim().to_string())
            .ok_or_else(|| {
                SecretError(format!(
                    "AWS SSM GetParameter returned no value for {ssm_path_owned}"
                ))
            })
    })
}

pub(crate) fn validate_binance_api_secret_shape(api_secret: &str) -> Result<(), SecretError> {
    if api_secret.trim().is_empty() {
        return Err(SecretError(
            "resolved Binance api_secret is empty".to_string(),
        ));
    }

    Ed25519Credential::new("BINANCE-SHAPE-CHECK".to_string(), api_secret)
        .map(|_| ())
        .map_err(|error| {
            SecretError(format!(
                "resolved Binance api_secret is not valid Ed25519 key material accepted by the NT Binance adapter: {error}"
            ))
        })
}

pub(crate) fn pad_base64(mut secret: String) -> String {
    let pad_len = (4 - secret.len() % 4) % 4;
    secret.extend(std::iter::repeat_n('=', pad_len));
    secret
}

fn is_present(value: Option<&String>) -> bool {
    value.is_some_and(|v| !v.trim().is_empty())
}

pub fn check_polymarket_secret_config(secrets: &ExecClientSecrets) -> SecretConfigCheck {
    let mut present = Vec::new();
    let mut missing = Vec::new();

    for (field, configured) in [
        ("region", !secrets.region.trim().is_empty()),
        ("pk", is_present(secrets.pk.as_ref())),
        ("api_key", is_present(secrets.api_key.as_ref())),
        ("api_secret", is_present(secrets.api_secret.as_ref())),
        ("passphrase", is_present(secrets.passphrase.as_ref())),
    ] {
        if configured {
            present.push(field);
        } else {
            missing.push(field);
        }
    }

    SecretConfigCheck { present, missing }
}

pub fn check_chainlink_secret_config(shared: &ChainlinkSharedConfig) -> SecretConfigCheck {
    let mut present = Vec::new();
    let mut missing = Vec::new();

    for (field, configured) in [
        ("region", !shared.region.trim().is_empty()),
        ("api_key", !shared.api_key.trim().is_empty()),
        ("api_secret", !shared.api_secret.trim().is_empty()),
    ] {
        if configured {
            present.push(field);
        } else {
            missing.push(field);
        }
    }

    SecretConfigCheck { present, missing }
}

pub fn check_binance_secret_config(shared: &BinanceSharedConfig) -> SecretConfigCheck {
    let contract = binance_secret_config_contract(shared);
    let mut present = Vec::new();
    let mut missing = Vec::new();

    for (field, configured) in [
        ("region", !contract.region.trim().is_empty()),
        ("api_key", !contract.api_key_path.trim().is_empty()),
        ("api_secret", !contract.api_secret_path.trim().is_empty()),
    ] {
        if configured {
            present.push(field);
        } else {
            missing.push(field);
        }
    }

    SecretConfigCheck { present, missing }
}

pub fn resolve_polymarket(
    secrets: &ExecClientSecrets,
) -> Result<ResolvedPolymarketSecrets, SecretError> {
    let check = check_polymarket_secret_config(secrets);
    if !check.is_complete() {
        return Err(SecretError(format!(
            "Missing required secret config fields: {}",
            check.missing.join(", ")
        )));
    }

    let region = &secrets.region;

    let private_key_path = secrets
        .pk
        .as_ref()
        .expect("pk must exist after config check");
    let api_key_path = secrets
        .api_key
        .as_ref()
        .expect("api_key must exist after config check");
    let api_secret_path = secrets
        .api_secret
        .as_ref()
        .expect("api_secret must exist after config check");
    let passphrase_path = secrets
        .passphrase
        .as_ref()
        .expect("passphrase must exist after config check");

    Ok(ResolvedPolymarketSecrets {
        private_key: resolve_secret(region, private_key_path)?,
        api_key: resolve_secret(region, api_key_path)?,
        api_secret: pad_base64(resolve_secret(region, api_secret_path)?),
        passphrase: resolve_secret(region, passphrase_path)?,
    })
}

pub fn resolve_chainlink(
    region: &str,
    api_key_path: &str,
    api_secret_path: &str,
) -> Result<ResolvedChainlinkSecrets, SecretError> {
    Ok(ResolvedChainlinkSecrets {
        api_key: resolve_secret(region, api_key_path)?,
        api_secret: resolve_secret(region, api_secret_path)?,
    })
}

pub fn resolve_binance(
    region: &str,
    api_key_path: &str,
    api_secret_path: &str,
) -> Result<ResolvedBinanceSecrets, SecretError> {
    resolve_binance_with(region, api_key_path, api_secret_path, resolve_secret)
}

pub(crate) fn resolve_binance_with<F>(
    region: &str,
    api_key_path: &str,
    api_secret_path: &str,
    resolve_secret_fn: F,
) -> Result<ResolvedBinanceSecrets, SecretError>
where
    F: Fn(&str, &str) -> Result<String, SecretError>,
{
    // Validate the secret before resolving the companion API key so failures
    // localize to unusable key material immediately.
    let api_secret = resolve_secret_fn(region, api_secret_path)?;
    validate_binance_api_secret_shape(&api_secret)?;

    Ok(ResolvedBinanceSecrets {
        api_key: resolve_secret_fn(region, api_key_path)?,
        api_secret,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ResolvedBinanceSecrets, ResolvedChainlinkSecrets, ResolvedPolymarketSecrets, pad_base64,
        validate_binance_api_secret_shape,
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};

    fn synthetic_ed25519_pkcs8_base64() -> String {
        let mut der = vec![0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03];
        der.extend_from_slice(&[0x2B, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20]);
        der.extend(0_u8..32);
        BASE64_STANDARD.encode(der)
    }

    #[test]
    fn debug_redacts_resolved_polymarket_secrets() {
        let secrets = ResolvedPolymarketSecrets {
            private_key: "private-key-value".to_string(),
            api_key: "api-key-value".to_string(),
            api_secret: "api-secret-value".to_string(),
            passphrase: "passphrase-value".to_string(),
        };

        let debug = format!("{secrets:?}");

        assert!(debug.contains("ResolvedPolymarketSecrets"));
        assert!(debug.contains("[REDACTED]"));
        for field in ["private_key", "api_key", "api_secret", "passphrase"] {
            assert!(debug.contains(field), "debug output should mention {field}");
        }
        for (i, secret) in [
            "private-key-value",
            "api-key-value",
            "api-secret-value",
            "passphrase-value",
        ]
        .iter()
        .enumerate()
        {
            assert!(
                !debug.contains(secret),
                "debug output leaked secret at index {i}"
            );
        }
    }

    #[test]
    fn debug_redacts_resolved_chainlink_secrets() {
        let secrets = ResolvedChainlinkSecrets {
            api_key: "api-key-value".to_string(),
            api_secret: "api-secret-value".to_string(),
        };

        let debug = format!("{secrets:?}");

        assert!(debug.contains("ResolvedChainlinkSecrets"));
        assert!(debug.contains("[REDACTED]"));
        for field in ["api_key", "api_secret"] {
            assert!(debug.contains(field), "debug output should mention {field}");
        }
        for secret in ["api-key-value", "api-secret-value"] {
            assert!(
                !debug.contains(secret),
                "debug output should not contain secret material"
            );
        }
    }

    #[test]
    fn debug_redacts_resolved_binance_secrets() {
        let secrets = ResolvedBinanceSecrets {
            api_key: "api-key-value".to_string(),
            api_secret: "api-secret-value".to_string(),
        };

        let debug = format!("{secrets:?}");

        assert!(debug.contains("ResolvedBinanceSecrets"));
        assert!(debug.contains("[REDACTED]"));
        for field in ["api_key", "api_secret"] {
            assert!(debug.contains(field), "debug output should mention {field}");
        }
        for secret in ["api-key-value", "api-secret-value"] {
            assert!(
                !debug.contains(secret),
                "debug output should not contain secret material"
            );
        }
    }

    #[test]
    fn pad_base64_preserves_existing_padding_shape() {
        assert_eq!(pad_base64("abcd".to_string()), "abcd");
        assert_eq!(pad_base64("abc".to_string()), "abc=");
        assert_eq!(pad_base64("ab".to_string()), "ab==");
    }

    #[test]
    fn validate_binance_api_secret_shape_accepts_base64_pkcs8_ed25519() {
        let secret = synthetic_ed25519_pkcs8_base64();
        validate_binance_api_secret_shape(&secret).expect("synthetic ed25519 base64 should pass");
    }

    #[test]
    fn validate_binance_api_secret_shape_accepts_raw_32_byte_seed_base64() {
        let secret = BASE64_STANDARD.encode((0_u8..32).collect::<Vec<_>>());
        validate_binance_api_secret_shape(&secret).expect("raw 32-byte ed25519 seed should pass");
    }

    #[test]
    fn validate_binance_api_secret_shape_accepts_pem_wrapped_pkcs8_ed25519() {
        let secret = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
            synthetic_ed25519_pkcs8_base64()
        );
        validate_binance_api_secret_shape(&secret).expect("synthetic ed25519 pem should pass");
    }

    #[test]
    fn validate_binance_api_secret_shape_rejects_short_base64_seed() {
        let secret = BASE64_STANDARD.encode((0_u8..31).collect::<Vec<_>>());

        let error =
            validate_binance_api_secret_shape(&secret).expect_err("short ed25519 seed should fail");
        assert!(
            error
                .to_string()
                .contains("Ed25519 private key must be 32 bytes")
        );
    }

    #[test]
    fn validate_binance_api_secret_shape_rejects_oid_only_false_positive() {
        let secret = BASE64_STANDARD.encode([0x2B, 0x65, 0x70]);

        let error = validate_binance_api_secret_shape(&secret)
            .expect_err("short oid-bearing blob should fail");
        assert!(
            error
                .to_string()
                .contains("Ed25519 private key must be 32 bytes")
        );
    }

    #[test]
    fn validate_binance_api_secret_shape_rejects_non_key_material() {
        let error = validate_binance_api_secret_shape("not-a-valid-binance-secret")
            .expect_err("plain invalid string should fail");
        assert!(error.to_string().contains("valid Ed25519 key material"));
    }

    #[test]
    fn production_resolve_secret_does_not_shell_out_to_aws_cli() {
        let source = include_str!("secrets.rs");
        assert!(
            !source.contains("std::process::Command::new(\"aws\")"),
            "bolt-v3 contract: production resolver must not invoke the AWS CLI; \
             it must use the Rust AWS SDK"
        );
        assert!(
            !source.contains("\"get-parameter\""),
            "bolt-v3 contract: production resolver must not pass `get-parameter` \
             to a subprocess; it must call the Rust SSM client"
        );
        assert!(
            source.contains("aws_sdk_ssm::"),
            "bolt-v3 contract: production resolver must use the aws-sdk-ssm crate"
        );
    }
}
