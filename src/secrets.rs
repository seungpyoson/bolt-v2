use std::cell::RefCell;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::rc::Rc;

use aws_config::BehaviorVersion;
use aws_sdk_ssm::{Client as SsmClient, config::Region};

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
    pub(crate) fn for_test(message: String) -> Self {
        Self(message)
    }
}

/// Venue-provider-neutral AWS Systems Manager resolver for synchronous
/// Bolt startup. The AWS SDK is the SSM client; the resolver itself
/// carries no venue-provider-specific knowledge and is keyed only by
/// AWS region and SSM parameter path. Owns one `current_thread` Tokio
/// runtime (so the AWS SDK's async API can be bridged from the
/// synchronous startup boundary) and a per-region `SsmClient` cache (so
/// AWS credential discovery and HTTP-client construction happen at most
/// once per region per session).
///
/// Each session-owning entry point is a synchronous bolt-v3 startup
/// boundary. The boundaries do not nest or share state:
///
/// 1. `fn main` Secrets Resolve subcommand (`src/main.rs`) — operator
///    smoke-test path that resolves and validates configured bolt-v3 venues.
/// 2. `build_bolt_v3_live_node` (`src/bolt_v3_live_node.rs`) — bolt-v3
///    LiveNode assembly entry point invoked before the NT runtime starts.
///
/// Current callers resolve bolt-v3 venue secrets by passing `&session` into
/// `resolve_bolt_v3_secrets` rather than constructing their own session.
///
/// `SsmResolverSession` is intentionally `!Send + !Sync` because it is
/// only ever used from the synchronous startup thread, before any
/// multi-threaded NT runtime is built. Sharing it across threads or
/// calling `resolve` from inside another runtime would re-introduce the
/// per-call `block_on` panic that motivated this type. `RefCell` carries
/// `!Sync` structurally; the `_not_send_sync: PhantomData<Rc<()>>` marker
/// carries `!Send` (and `!Sync` redundantly) so a future contributor
/// cannot accidentally move the session into `tokio::spawn` or share it
/// across threads. The shared
/// `Self::ensure_not_inside_active_tokio_runtime()` helper (#255-3 / #256-A1)
/// is called both at the top of `resolve` and at the top of
/// `client_for` so every `Runtime::block_on` site on this type
/// converts a same-thread misuse into a structured `SecretError`
/// instead of a runtime panic. Compile-time regression guards live in
/// `tests::ssm_resolver_session_is_not_send` and
/// `tests::ssm_resolver_session_is_not_sync`; runtime guards live in
/// `tests::ssm_resolver_session_resolve_inside_active_tokio_runtime_returns_secret_error`
/// and
/// `tests::ssm_resolver_session_client_for_inside_active_tokio_runtime_returns_secret_error`.
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

    /// Per #256-A1: every `Runtime::block_on` site on this type must be
    /// guarded so a same-thread misuse from inside an outer Tokio
    /// runtime returns a structured `SecretError` rather than
    /// panicking. Both `resolve` (whose direct `block_on` drives the
    /// SSM `GetParameter`) and `client_for` (whose `block_on` drives
    /// `aws_config::defaults().load()`) call this helper before
    /// reaching their respective `block_on`s. Production sync startup
    /// paths run with no outer runtime active, so the helper is a
    /// no-op there.
    fn ensure_not_inside_active_tokio_runtime() -> Result<(), SecretError> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(SecretError(
                "SsmResolverSession invoked from inside an active Tokio \
                 runtime; SSM resolution must run on the synchronous startup \
                 boundary, before any NT runtime is built"
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub fn resolve(&self, region: &str, ssm_path: &str) -> Result<String, SecretError> {
        // Per #256-A1: the runtime-context guard is the shared
        // `ensure_not_inside_active_tokio_runtime` helper, called both
        // here (covering this method's direct `block_on` for
        // GetParameter) and inside `client_for` (covering the
        // aws-config-load `block_on`). The `PhantomData<Rc<()>>` marker
        // (#253) rejects the threaded-spawn footgun at compile time;
        // this guard rejects the same-thread variant at runtime.
        Self::ensure_not_inside_active_tokio_runtime()?;
        let client = self.client_for(region)?;
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
                .map(|raw| raw.to_string())
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

    fn client_for(&self, region: &str) -> Result<SsmClient, SecretError> {
        // Per #256-A1: `client_for` owns its own runtime-context guard
        // because its `block_on` (driving `aws_config::defaults().load()`)
        // would panic the same way `resolve`'s `block_on` would.
        // Placing the guard before the cache-hit short-circuit means
        // even a cache hit fails fast on misuse, keeping the contract
        // uniform across hit and miss paths.
        Self::ensure_not_inside_active_tokio_runtime()?;
        if let Some(client) = self.clients.borrow().get(region) {
            return Ok(client.clone());
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
        Ok(client)
    }
}

#[cfg(test)]
mod tests {
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

        session
            .client_for("us-east-1")
            .expect("client_for must succeed outside any active Tokio runtime");
        assert_eq!(
            session.cached_region_count(),
            1,
            "first call for a region must populate the cache"
        );

        session
            .client_for("us-east-1")
            .expect("repeat client_for call must succeed");
        assert_eq!(
            session.cached_region_count(),
            1,
            "same-region calls must hit the cache, not allocate a new entry"
        );

        session
            .client_for("eu-west-1")
            .expect("client_for for second region must succeed");
        assert_eq!(
            session.cached_region_count(),
            2,
            "different-region calls must allocate a new cache entry"
        );

        session
            .client_for("us-east-1")
            .expect("returning client_for call must succeed");
        assert_eq!(
            session.cached_region_count(),
            2,
            "returning to a previously-cached region must still hit the cache"
        );
    }

    #[test]
    fn ssm_resolver_session_client_for_inside_active_tokio_runtime_returns_secret_error() {
        // Per #256-A1: `client_for` is the actual caller of
        // `runtime.block_on(aws_config::defaults().load())`, so it must
        // independently reject misuse from inside an outer Tokio runtime.
        // The guard at the top of `resolve` covers `resolve`'s direct
        // `block_on`; a separate guard inside `client_for` covers any
        // future direct caller (today there are none — `client_for` is
        // private — but the cache-reuse test calls it directly, and any
        // refactor that re-exposes it must keep the runtime-nested
        // misuse path returning a structured `SecretError` rather than
        // panicking via Tokio.
        let session =
            super::SsmResolverSession::new().expect("session must build before outer runtime");
        let outer = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("outer current-thread runtime must build for this test");
        let result = outer.block_on(async { session.client_for("us-east-1") });
        let err = result.expect_err(
            "client_for must return Err instead of panicking when called \
             from inside an active Tokio runtime",
        );
        assert!(
            err.to_string().contains("active Tokio runtime"),
            "guard error must name the nested-runtime cause; got: {err}"
        );
        assert_eq!(
            session.cached_region_count(),
            0,
            "client_for nested-runtime guard must fail before mutating the region cache"
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
        // `impl` block's closing brace appears at column 0.
        let source = include_str!("secrets.rs");
        let production_source = source
            .split_once("#[cfg(test)]\nmod tests")
            .map(|(prod, _)| prod)
            .expect("secrets.rs must contain a #[cfg(test)] mod tests block");

        // Both endpoints use the same `trim_end()` policy so optional
        // trailing whitespace is tolerated symmetrically while leading
        // whitespace remains the load-bearing column anchor: a `}` that
        // closes an inner item is indented (column ≥ 4) and is rejected
        // here, but the impl block's closing brace at column 0 is matched.
        let lines: Vec<&str> = production_source.lines().collect();
        let impl_open_idx = lines
            .iter()
            .position(|line| line.trim_end() == "impl SsmResolverSession {")
            .expect("SsmResolverSession impl block must open on its own line");
        let impl_close_offset = lines[impl_open_idx + 1..]
            .iter()
            .position(|line| line.trim_end() == "}")
            .expect(
                "SsmResolverSession impl block must close on a column-0 `}` line; \
                 trailing whitespace is tolerated",
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

    #[test]
    fn ssm_resolver_session_does_not_trim_resolved_secret_values() {
        // The bolt-v3 secret contract pins the SSM resolver as byte-exact:
        // `SsmResolverSession::resolve` returns the parameter value
        // unchanged, and `bolt_v3_secrets::resolve_field` owns every
        // whitespace rejection (empty, padded, embedded). The earlier
        // version of this guard only forbade `.trim()`, which left
        // `.trim_start()`, `.trim_end()`, `.replace(`, and regex-based
        // normalization paths able to silently regress the byte-exact
        // contract without tripping any test. Per MECE PR #331 P3
        // round-1 finding P3-NB1, this guard now rejects every
        // transform variant the contract forbids on the resolve body.
        let source = include_str!("secrets.rs");
        let resolve_body = source
            .split_once("    pub fn resolve(")
            .and_then(|(_, tail)| tail.split_once("    pub fn cached_region_count"))
            .map(|(body, _)| body)
            .expect("SsmResolverSession::resolve body must be present");

        for forbidden in [
            ".trim()",
            ".trim_start()",
            ".trim_end()",
            ".replace(",
            "Regex::",
            "regex::Regex",
        ] {
            assert!(
                !resolve_body.contains(forbidden),
                "SsmResolverSession::resolve must return the raw SSM value so \
                 bolt_v3_secrets::resolve_field owns whitespace rejection; found \
                 forbidden transform `{forbidden}` in the resolve body"
            );
        }
    }
}
