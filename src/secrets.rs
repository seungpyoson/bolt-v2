use std::cell::RefCell;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::rc::Rc;

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

#[cfg(test)]
impl SecretError {
    /// Test-only constructor used by other modules to build a `SecretError`
    /// without going through a real failure path. Hidden behind `cfg(test)`
    /// so the production tuple-field stays private and no out-of-module
    /// caller can fabricate a `SecretError` at runtime.
    pub(crate) fn __test_new(message: String) -> Self {
        Self(message)
    }
}

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

/// Provider-neutral AWS Systems Manager resolver for synchronous Bolt
/// startup. Owns one `current_thread` Tokio runtime (so the AWS SDK's
/// async API can be bridged from the synchronous startup boundary) and a
/// per-region `SsmClient` cache (so AWS credential discovery and
/// HTTP-client construction happen at most once per region per process).
/// The session itself is keyed only by AWS region and SSM parameter path;
/// it has no provider-specific knowledge.
///
/// Each Bolt binary invocation runs through exactly one of three
/// independent sync startup boundaries, and each boundary owns its own
/// session. The boundaries do not nest or share state:
///
/// 1. `fn main` Run subcommand (`src/main.rs`) — legacy reference-data
///    pipeline + Polymarket V1 secret resolution.
/// 2. `fn main` Secrets Resolve subcommand (`src/main.rs`) — operator
///    smoke-test path that resolves and validates configured venues.
/// 3. `build_bolt_v3_live_node` (`src/bolt_v3_live_node.rs`) — bolt-v3
///    LiveNode assembly entry point invoked by external callers; this
///    binary's `Run` subcommand does not call it.
///
/// Current callers within those boundaries include Binance, Chainlink,
/// Polymarket, and bolt-v3 venue secret resolution; new providers plug
/// in by passing `&session` into a new `resolve_*` helper rather than
/// constructing their own session.
///
/// `SsmResolverSession` is intentionally `!Send + !Sync` because it is
/// only ever used from the synchronous startup thread, before any
/// multi-threaded NT runtime is built. Sharing it across threads or
/// calling `resolve` from inside another runtime would re-introduce the
/// per-call `block_on` panic that motivated this type. `RefCell` carries
/// `!Sync` structurally; the `_not_send_sync: PhantomData<Rc<()>>` marker
/// carries `!Send` (and `!Sync` redundantly) so a future contributor
/// cannot accidentally move the session into `tokio::spawn` or share it
/// across threads. The `Handle::try_current()` guard in `resolve` (#255-3)
/// converts a same-thread misuse — calling `resolve` from inside an
/// active runtime context — into a structured `SecretError` instead of a
/// runtime panic. Compile-time regression guards live in
/// `tests::ssm_resolver_session_is_not_send` and
/// `tests::ssm_resolver_session_is_not_sync`; runtime guards live in
/// `tests::ssm_resolver_session_resolve_inside_active_tokio_runtime_returns_secret_error`.
pub struct SsmResolverSession {
    runtime: tokio::runtime::Runtime,
    clients: RefCell<BTreeMap<String, SsmClient>>,
    _not_send_sync: PhantomData<Rc<()>>,
}

impl SsmResolverSession {
    pub fn new() -> Result<Self, SecretError> {
        // Production startup is synchronous (see `fn main` in src/main.rs and
        // `bolt_v3_live_node::build_bolt_v3_live_node`), so the AWS SDK's
        // async GetParameter calls are bridged through a single contained
        // current-thread Tokio runtime owned by the session, rather than
        // building a fresh runtime per resolution.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                SecretError(format!(
                    "failed to build Tokio runtime for SSM resolver session: {error}"
                ))
            })?;
        Ok(Self {
            runtime,
            clients: RefCell::new(BTreeMap::new()),
            _not_send_sync: PhantomData,
        })
    }

    pub fn resolve(&self, region: &str, ssm_path: &str) -> Result<String, SecretError> {
        // Per #255-3: `Runtime::block_on` panics when called inside an
        // active Tokio runtime context (the same panic geometry the
        // session was created to prevent). The `PhantomData<Rc<()>>`
        // marker rejects the threaded-spawn footgun at compile time, but
        // a same-thread caller could still call `resolve` from inside
        // another runtime's `block_on` body or an async fn driven by an
        // active runtime. Convert that misuse into a structured
        // `SecretError` so the failure surfaces through the normal error
        // path instead of unwinding the runtime. Production paths
        // (`fn main` Run, `fn main` Secrets Resolve, `build_bolt_v3_live_node`)
        // run synchronously before any NT runtime is built, so this
        // guard is invariant under current production behavior.
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(SecretError(
                "SsmResolverSession::resolve called from inside an active \
                 Tokio runtime; SSM resolution must run on the synchronous \
                 startup boundary, before any NT runtime is built"
                    .to_string(),
            ));
        }
        let client = self.client_for(region);
        let ssm_path_owned = ssm_path.to_string();
        self.runtime.block_on(async move {
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

    pub fn cached_region_count(&self) -> usize {
        self.clients.borrow().len()
    }

    fn client_for(&self, region: &str) -> SsmClient {
        if let Some(client) = self.clients.borrow().get(region) {
            return client.clone();
        }
        let region_owned = region.to_string();
        let aws_config = self.runtime.block_on(
            aws_config::defaults(BehaviorVersion::latest())
                .region(Region::new(region_owned))
                .load(),
        );
        let client = SsmClient::new(&aws_config);
        self.clients
            .borrow_mut()
            .insert(region.to_string(), client.clone());
        client
    }
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
    session: &SsmResolverSession,
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
        private_key: session.resolve(region, private_key_path)?,
        api_key: session.resolve(region, api_key_path)?,
        api_secret: pad_base64(session.resolve(region, api_secret_path)?),
        passphrase: session.resolve(region, passphrase_path)?,
    })
}

pub fn resolve_chainlink(
    session: &SsmResolverSession,
    region: &str,
    api_key_path: &str,
    api_secret_path: &str,
) -> Result<ResolvedChainlinkSecrets, SecretError> {
    Ok(ResolvedChainlinkSecrets {
        api_key: session.resolve(region, api_key_path)?,
        api_secret: session.resolve(region, api_secret_path)?,
    })
}

pub fn resolve_binance(
    session: &SsmResolverSession,
    region: &str,
    api_key_path: &str,
    api_secret_path: &str,
) -> Result<ResolvedBinanceSecrets, SecretError> {
    resolve_binance_with(region, api_key_path, api_secret_path, |r, p| {
        session.resolve(r, p)
    })
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

    #[test]
    fn ssm_resolver_session_constructs_without_aws_calls_and_starts_empty() {
        // Per #252: the session must own one Tokio runtime + per-region
        // SsmClient cache. Construction itself does not hit AWS — the cache
        // populates lazily on the first resolve() per region.
        let session = super::SsmResolverSession::new()
            .expect("SsmResolverSession::new must succeed without AWS network calls");
        assert_eq!(
            session.cached_region_count(),
            0,
            "fresh session must have no cached SsmClient instances"
        );
    }

    #[test]
    fn ssm_resolver_session_resolve_takes_region_and_path() {
        // Per #252: the production resolver entry point is
        // `SsmResolverSession::resolve(&self, region, path)`, taking
        // `&SsmResolverSession` so a single AWS SDK config + SsmClient is
        // reused across every secret resolution at startup. A bare
        // `fn(&str, &str) -> ...` shape — the pre-fix signature Gemini
        // flagged on PR #251 — would force per-call construction of both;
        // this guard pins the new shape.
        fn _assert_signature<F>(_f: F)
        where
            F: Fn(&super::SsmResolverSession, &str, &str) -> Result<String, super::SecretError>,
        {
        }
        _assert_signature(super::SsmResolverSession::resolve);
    }

    #[test]
    fn ssm_resolver_session_is_not_send() {
        // Per #252 design review: the docstring on `SsmResolverSession`
        // claims `!Send + !Sync`, but Rust's auto-derive only carried
        // `!Sync` (from `RefCell`); the type was actually `Send`. A future
        // contributor moving the session into `tokio::spawn` would
        // re-introduce the per-call `block_on` panic geometry that
        // motivated the type — `block_on` panics inside an active
        // multi-thread runtime. This guard pins `!Send` so that footgun
        // is rejected at compile time. Implementation: a
        // `PhantomData<Rc<()>>` marker on the struct.
        //
        // Compile-time `!Send` assertion (see `static_assertions`'s
        // `assert_not_impl_any!`): two impls of an auxiliary trait pick
        // different `A` parameters, and the call site lets the compiler
        // infer `_`. If `SsmResolverSession: Send`, both impls apply,
        // inference is ambiguous, and the test fails to compile.
        trait AmbiguousIfSend<A> {
            fn _check() {}
        }
        impl<T: ?Sized> AmbiguousIfSend<()> for T {}
        struct Invalid;
        impl<T: ?Sized + Send> AmbiguousIfSend<Invalid> for T {}
        let _ = <super::SsmResolverSession as AmbiguousIfSend<_>>::_check;
    }

    #[test]
    fn ssm_resolver_session_is_not_sync() {
        // Per #252 design review: `!Sync` is structurally enforced by
        // `RefCell`, but pinning it here rejects regressions that swap
        // `RefCell` for an interior-mutability primitive that is `Sync`
        // (e.g., `Mutex`) without re-evaluating cross-thread sharing of
        // the contained Tokio runtime + AWS clients.
        trait AmbiguousIfSync<A> {
            fn _check() {}
        }
        impl<T: ?Sized> AmbiguousIfSync<()> for T {}
        struct Invalid;
        impl<T: ?Sized + Sync> AmbiguousIfSync<Invalid> for T {}
        let _ = <super::SsmResolverSession as AmbiguousIfSync<_>>::_check;
    }

    #[test]
    fn ssm_resolver_session_caches_clients_per_region() {
        // Per #255-6: the per-region SsmClient cache is the load-bearing
        // mechanism that makes AWS-config + SsmClient construction
        // amortize across all secret resolutions on one startup
        // boundary. The structural source guard
        // (`ssm_resolver_session_owns_runtime_and_aws_config_construction`)
        // pins *where* construction may happen; this test pins that the
        // cache actually re-uses entries on a hit (same region) and adds
        // entries on a miss (different region). It exercises the
        // private `client_for` path directly — accessible because
        // `mod tests` is the same module that owns the type — to
        // observe `cached_region_count` transitions deterministically
        // without a factory seam.
        //
        // No network round-trip: `aws_config::defaults(...).load()`
        // builds the config struct synchronously (profile-file reads,
        // optional `~/.aws/config` parse). Credential resolution is
        // lazy in the AWS SDK and only happens on the first AWS API
        // call — which this test never makes. `SsmClient::new` is also
        // offline. The session's contained current-thread runtime
        // drives `load()` to completion on the test thread.
        let session = super::SsmResolverSession::new()
            .expect("SsmResolverSession::new must succeed without AWS network calls");
        assert_eq!(session.cached_region_count(), 0);

        let _a = session.client_for("us-east-1");
        assert_eq!(
            session.cached_region_count(),
            1,
            "first call for a region must populate the cache"
        );

        let _b = session.client_for("us-east-1");
        assert_eq!(
            session.cached_region_count(),
            1,
            "same-region calls must hit the cache, not allocate a new entry"
        );

        let _c = session.client_for("eu-west-1");
        assert_eq!(
            session.cached_region_count(),
            2,
            "different-region calls must allocate a new cache entry"
        );

        let _d = session.client_for("us-east-1");
        assert_eq!(
            session.cached_region_count(),
            2,
            "returning to a previously-cached region must still hit the cache"
        );
    }

    #[test]
    fn ssm_resolver_session_resolve_inside_active_tokio_runtime_returns_secret_error() {
        // Per #255-3: SsmResolverSession::resolve uses Runtime::block_on
        // internally. Tokio panics if `block_on` runs inside another
        // runtime's task. The PhantomData<Rc<()>> marker added in #253
        // prevents the session from crossing threads via `tokio::spawn`,
        // but a future caller could still call `resolve` from inside the
        // current thread's existing runtime (e.g., `outer.block_on(async {
        // session.resolve(...) })` or from an async fn invoked synchronously
        // inside `block_on`). Without a guard, that panics. The guard
        // converts the runtime context into a structured `SecretError` so
        // the call site can surface the misuse through the normal error
        // path instead of unwinding the runtime.
        // The inner session is constructed *before* the outer runtime so
        // its contained current-thread runtime is dropped on the test
        // thread after `outer.block_on` returns — Tokio panics if a
        // runtime is dropped from inside an async context.
        let session = super::SsmResolverSession::new()
            .expect("inner SsmResolverSession::new must succeed before outer runtime");
        let outer = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("outer current-thread runtime must build for this test");
        let result = outer.block_on(async { session.resolve("us-east-1", "/bolt/test/dummy") });
        let err = result.expect_err(
            "resolve must return Err instead of panicking when called from \
             inside an active Tokio runtime",
        );
        let message = err.to_string();
        assert!(
            message.contains("active Tokio runtime"),
            "guard error must name the nested-runtime cause; got: {message}"
        );
    }

    #[test]
    fn ssm_resolver_session_owns_runtime_and_aws_config_construction() {
        // Per #252 / #255-5: the SsmResolverSession impl block is the only
        // place in this module's production code that may construct a
        // Tokio runtime, an AWS SDK config, or an SsmClient — and the
        // only place that may consult an existing Tokio runtime handle.
        // This guard catches regressions that would reintroduce per-call
        // construction or alternate runtime-context paths. The
        // `#[cfg(test)] mod tests` block is excluded from the scan
        // because the assertions below reference these literals as
        // identifiers in their own source.
        //
        // #255-5 hardening: the impl block boundaries are located via a
        // column-anchored line scan rather than `split_once("\n}\n")` so
        // a brace-pair coincidentally matching that substring inside the
        // impl body cannot mis-bound the search. In Rust formatting, an
        // `impl` block's closing brace appears on a line that is
        // exactly `}` (no leading whitespace).
        let source = include_str!("secrets.rs");
        let production_source = source
            .split_once("#[cfg(test)]\nmod tests")
            .map(|(prod, _)| prod)
            .expect("secrets.rs must contain a #[cfg(test)] mod tests block");

        let lines: Vec<&str> = production_source.lines().collect();
        let impl_open_idx = lines
            .iter()
            .position(|line| line.trim_end() == "impl SsmResolverSession {")
            .expect("SsmResolverSession impl block must open on its own line");
        let impl_close_offset = lines[impl_open_idx + 1..]
            .iter()
            .position(|line| *line == "}")
            .expect(
                "SsmResolverSession impl block must close on a line that is exactly \
                 `}` with no leading whitespace",
            );
        let impl_close_idx = impl_open_idx + 1 + impl_close_offset;
        let session_impl: String = lines[impl_open_idx + 1..impl_close_idx].join("\n");
        let outside_session: String = lines[..impl_open_idx]
            .iter()
            .chain(lines[impl_close_idx + 1..].iter())
            .copied()
            .collect::<Vec<_>>()
            .join("\n");

        // Required-inside-impl: every runtime/AWS-config/SsmClient
        // construction site for this module's production code.
        assert!(
            session_impl.contains("tokio::runtime::Builder::new_current_thread"),
            "SsmResolverSession impl must own the Tokio runtime construction"
        );
        assert!(
            session_impl.contains("aws_config::defaults"),
            "SsmResolverSession impl must own AWS SDK config construction"
        );
        assert!(
            session_impl.contains("SsmClient::new"),
            "SsmResolverSession impl must own SsmClient construction"
        );

        // Forbidden-outside-impl: alternate construction or
        // runtime-context paths that would re-introduce the per-call
        // panic geometry the session was created to prevent. Each
        // pattern is anchored precisely enough that it does not collide
        // with a sibling identifier (e.g., `Handle::current(` does not
        // appear as a substring of `Handle::try_current(`).
        for forbidden in [
            "tokio::runtime::Builder",
            "tokio::runtime::Runtime::new",
            "aws_config::defaults",
            "SsmClient::new",
            "Handle::current(",
        ] {
            assert!(
                !outside_session.contains(forbidden),
                "`{forbidden}` must be centralized inside the SsmResolverSession \
                 impl block; found another call site outside it"
            );
        }
    }
}
