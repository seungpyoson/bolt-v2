use crate::config::{ChainlinkSharedConfig, ExecClientSecrets};

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

fn resolve_secret(region: &str, ssm_path: &str) -> Result<String, SecretError> {
    let output = std::process::Command::new("aws")
        .args([
            "ssm",
            "get-parameter",
            "--region",
            region,
            "--name",
            ssm_path,
            "--with-decryption",
            "--query",
            "Parameter.Value",
            "--output",
            "text",
        ])
        .output()
        .map_err(|e| {
            SecretError(format!(
                "Failed to run aws ssm get-parameter for {ssm_path}: {e}"
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(SecretError(format!(
            "aws ssm get-parameter failed for {ssm_path}: {stderr}"
        )));
    }

    String::from_utf8(output.stdout)
        .map(|s| s.trim().to_string())
        .map_err(|e| SecretError(format!("Invalid UTF-8 from SSM for {ssm_path}: {e}")))
}

fn pad_base64(mut secret: String) -> String {
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

pub fn check_binance_secret_config(
    shared: &crate::config::BinanceSharedConfig,
) -> SecretConfigCheck {
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
    Ok(ResolvedBinanceSecrets {
        api_key: resolve_secret(region, api_key_path)?,
        api_secret: resolve_secret(region, api_secret_path)?,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ResolvedBinanceSecrets, ResolvedChainlinkSecrets, ResolvedPolymarketSecrets, pad_base64,
    };

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
}
