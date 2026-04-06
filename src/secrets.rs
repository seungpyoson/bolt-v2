use crate::config::ExecClientSecrets;

#[derive(Debug)]
pub struct SecretError(String);

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for SecretError {}

#[derive(Debug, Clone)]
pub struct ResolvedPolymarketSecrets {
    pub private_key: String,
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: String,
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
        .map_err(|e| SecretError(format!("Failed to run aws ssm get-parameter for {ssm_path}: {e}")))?;

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

pub fn resolve_polymarket(
    secrets: &ExecClientSecrets,
) -> Result<ResolvedPolymarketSecrets, SecretError> {
    let region = &secrets.region;

    let private_key_path = secrets
        .pk
        .as_ref()
        .ok_or_else(|| SecretError("Missing pk SSM path".to_string()))?;
    let api_key_path = secrets
        .api_key
        .as_ref()
        .ok_or_else(|| SecretError("Missing api_key SSM path".to_string()))?;
    let api_secret_path = secrets
        .api_secret
        .as_ref()
        .ok_or_else(|| SecretError("Missing api_secret SSM path".to_string()))?;
    let passphrase_path = secrets
        .passphrase
        .as_ref()
        .ok_or_else(|| SecretError("Missing passphrase SSM path".to_string()))?;

    Ok(ResolvedPolymarketSecrets {
        private_key: resolve_secret(region, private_key_path)?,
        api_key: resolve_secret(region, api_key_path)?,
        api_secret: pad_base64(resolve_secret(region, api_secret_path)?),
        passphrase: resolve_secret(region, passphrase_path)?,
    })
}
