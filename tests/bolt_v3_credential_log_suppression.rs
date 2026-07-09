#![cfg(unix)]

//! Behavioral isolation regression: in a process where bolt-v3 is the
//! only path initializing the NT global logger, no NT credential-info
//! log string from `nautilus_polymarket::common::credential` or
//! `nautilus_binance::common::credential` may reach stdout or stderr
//! during a v3 `LiveNodeBuilder::build()`. The bolt-v3
//! `LoggerConfig.module_level` filter must drop them at the NT logger
//! thread.
//!
//! This test deliberately lives in its own dedicated test binary
//! (`cargo test --test bolt_v3_credential_log_suppression`). NT's
//! global logger only honors the *first* `LoggerConfig` an in-process
//! caller hands it: once any other code initializes the NT logger
//! without bolt-v3 module filters (for example, a legacy bolt-v2
//! `LoggerConfig::default()` path in another test binary), later
//! bolt-v3 configs cannot retroactively install module filters. By
//! living in its own test binary, this test guarantees the bolt-v3
//! `LoggerConfig` is the first and only thing initializing NT's
//! logger in this process, so the assertion proves real behavior
//! rather than relying on test ordering.
//!
//! The configuration-level companion check
//! (`live_node_config_suppresses_nt_credential_module_logs_to_warn`)
//! lives in `src/bolt_v3_live_node.rs` and pins the bolt-v3
//! `LoggerConfig.module_level` shape; this test pins what the NT
//! logger thread actually emits to the process's standard streams
//! when that config is the active one.

use crate::support;

use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::io::AsRawFd;
use std::panic::{AssertUnwindSafe, catch_unwind};

use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::{
        build_bolt_v3_live_node_with_summary, build_bolt_v3_strategy_free_live_node_with_summary,
    },
    bolt_v3_providers::{
        binance::ResolvedBoltV3BinanceSecrets, polymarket::ResolvedBoltV3PolymarketSecrets,
    },
};
use nautilus_core::string::secret::REDACTED;
use tempfile::tempfile;
use zeroize::ZeroizeOnDrop;

const FORBIDDEN_CREDENTIAL_MARKERS: &[&str] = &[
    // nautilus_polymarket::common::credential::Credentials::resolve
    "Polymarket credentials resolved",
    // nautilus_binance::common::credential::SigningCredential::new
    "Auto-detected Ed25519 API key",
    "Using HMAC SHA256 API key",
];
const LOGGER_SURVIVAL_CHILD_ENV: &str = "BOLT_V3_LOGGER_SURVIVAL_CHILD_MODE";
const LOGGER_SURVIVAL_SENTINEL: &str = "bolt-v3-logger-survival-after-reference-health";

fn assert_zeroize_on_drop<T: ZeroizeOnDrop>() {}

fn load_logger_probe_config(label: &str) -> (support::TempCaseDir, LoadedBoltV3Config) {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new(label);
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    (temp, loaded)
}

fn build_stop_drop_strategy_free_logger_probe(label: &str) {
    let (_temp, loaded) = load_logger_probe_config(label);
    let (runtime, _summary) = build_bolt_v3_strategy_free_live_node_with_summary(
        &loaded,
        |_| false,
        support::fake_bolt_v3_resolver,
    )
    .expect("strategy-free logger probe LiveNode should build");
    runtime.handle().stop();
    drop(runtime);
    std::thread::sleep(std::time::Duration::from_millis(250));
}

fn emit_logger_survival_record_from_later_kernel() {
    let (_temp, loaded) = load_logger_probe_config("bolt-v3-logger-survival-later-kernel");
    let (runtime, _summary) = build_bolt_v3_strategy_free_live_node_with_summary(
        &loaded,
        |_| false,
        support::fake_bolt_v3_resolver,
    )
    .expect("later strategy-free LiveNode should build after the health probe");

    assert_ne!(
        log::max_level(),
        log::LevelFilter::Off,
        "the later kernel must leave the process logger enabled"
    );
    assert!(
        log::log_enabled!(log::Level::Error),
        "the later kernel must enable error records"
    );
    log::error!("{LOGGER_SURVIVAL_SENTINEL}");
    log::logger().flush();
    std::thread::sleep(std::time::Duration::from_millis(250));

    runtime.handle().stop();
    drop(runtime);
}

fn run_logger_survival_child(test_filter: &str, mode: &str) {
    let output = std::process::Command::new(
        std::env::current_exe().expect("current test binary should be available"),
    )
    .arg(test_filter)
    .arg("--nocapture")
    .arg("--test-threads=1")
    .env(LOGGER_SURVIVAL_CHILD_ENV, mode)
    .output()
    .expect("logger-survival child test should launch");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "logger-survival child mode `{mode}` failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout,
        stderr
    );
    assert!(
        stdout.contains("running 1 test"),
        "logger-survival child filter `{test_filter}` must run exactly one test\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

fn capture_standard_streams(action: impl FnOnce()) -> (String, String) {
    let mut stdout_capture = tempfile().expect("tempfile for stdout capture");
    let mut stderr_capture = tempfile().expect("tempfile for stderr capture");

    let real_stdout = unsafe { libc::dup(1) };
    let real_stderr = unsafe { libc::dup(2) };
    assert!(real_stdout >= 0, "dup(1) failed");
    assert!(real_stderr >= 0, "dup(2) failed");

    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    unsafe {
        libc::dup2(stdout_capture.as_raw_fd(), 1);
        libc::dup2(stderr_capture.as_raw_fd(), 2);
    }

    let action_result = catch_unwind(AssertUnwindSafe(action));

    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    unsafe {
        libc::dup2(real_stdout, 1);
        libc::dup2(real_stderr, 2);
        libc::close(real_stdout);
        libc::close(real_stderr);
    }

    stdout_capture
        .seek(SeekFrom::Start(0))
        .expect("stdout seek");
    let mut stdout_text = String::new();
    stdout_capture
        .read_to_string(&mut stdout_text)
        .expect("stdout read");

    stderr_capture
        .seek(SeekFrom::Start(0))
        .expect("stderr seek");
    let mut stderr_text = String::new();
    stderr_capture
        .read_to_string(&mut stderr_text)
        .expect("stderr read");

    if action_result.is_err() {
        panic!(
            "captured logger-survival action panicked; captured stdout=`{stdout_text}`; \
             captured stderr=`{stderr_text}`"
        );
    }

    (stdout_text, stderr_text)
}

#[test]
fn resolved_provider_secret_debug_redacts_and_zeroizes_on_drop() {
    assert_zeroize_on_drop::<ResolvedBoltV3PolymarketSecrets>();
    assert_zeroize_on_drop::<ResolvedBoltV3BinanceSecrets>();

    let polymarket = ResolvedBoltV3PolymarketSecrets {
        private_key: zeroize::Zeroizing::new("poly-private-sentinel".to_string()),
        api_key: zeroize::Zeroizing::new("poly-api-key-sentinel".to_string()),
        api_secret: zeroize::Zeroizing::new("poly-api-secret-sentinel".to_string()),
        passphrase: zeroize::Zeroizing::new("poly-passphrase-sentinel".to_string()),
    };
    let polymarket_debug = format!("{polymarket:?}");
    assert!(polymarket_debug.contains(REDACTED));
    for secret in [
        "poly-private-sentinel",
        "poly-api-key-sentinel",
        "poly-api-secret-sentinel",
        "poly-passphrase-sentinel",
    ] {
        assert!(
            !polymarket_debug.contains(secret),
            "Polymarket resolved-secret Debug leaked `{secret}`: {polymarket_debug}"
        );
    }

    let binance = ResolvedBoltV3BinanceSecrets {
        api_key: zeroize::Zeroizing::new("binance-api-key-sentinel".to_string()),
        api_secret: zeroize::Zeroizing::new("binance-api-secret-sentinel".to_string()),
    };
    let binance_debug = format!("{binance:?}");
    assert!(binance_debug.contains(REDACTED));
    for secret in ["binance-api-key-sentinel", "binance-api-secret-sentinel"] {
        assert!(
            !binance_debug.contains(secret),
            "Binance resolved-secret Debug leaked `{secret}`: {binance_debug}"
        );
    }
}

#[test]
fn in_process_reference_health_probe_preserves_parent_logger_for_later_kernel() {
    match std::env::var(LOGGER_SURVIVAL_CHILD_ENV).ok().as_deref() {
        None => {
            run_logger_survival_child(
                "in_process_reference_health_probe_preserves_parent_logger_for_later_kernel",
                "parent",
            );
        }
        Some("parent") => {
            let (_stdout, stderr) = capture_standard_streams(|| {
                build_stop_drop_strategy_free_logger_probe("bolt-v3-logger-survival-health-probe");
                emit_logger_survival_record_from_later_kernel();
            });

            assert!(
                stderr.contains(LOGGER_SURVIVAL_SENTINEL),
                "later kernel error record must reach the NT stderr sink after in-process health; \
                 captured stderr=`{stderr}`"
            );
        }
        Some(mode) => panic!("unexpected logger survival child mode `{mode}`"),
    }
}

#[test]
fn v3_livenode_build_does_not_emit_nt_credential_info_logs_to_standard_streams() {
    // Capture stdout and stderr at the file-descriptor level. NT's
    // logger thread writes formatted log lines to the process stdout
    // via Rust's `std::io::stdout()`, which ultimately writes to
    // file descriptor 1. We dup2 a tempfile onto fds 1 and 2, run the
    // bolt-v3 build, restore the real fds, then read what NT actually
    // wrote.
    let mut stdout_capture = tempfile().expect("tempfile for stdout capture");
    let mut stderr_capture = tempfile().expect("tempfile for stderr capture");

    // SAFETY: we are interacting with libc to dup/dup2 on POSIX file
    // descriptors. Each `dup` returns a non-negative fd or -1; we
    // assert non-negative before using it. The `dup2` calls always
    // succeed for valid fds. We restore the original fds before
    // returning so the test process keeps a working stdout/stderr.
    let real_stdout = unsafe { libc::dup(1) };
    let real_stderr = unsafe { libc::dup(2) };
    assert!(real_stdout >= 0, "dup(1) failed");
    assert!(real_stderr >= 0, "dup(2) failed");

    // Flush Rust's stdio buffers before swapping the underlying fds
    // so any already-buffered output goes to the real stdout/stderr,
    // not into our capture file.
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    unsafe {
        libc::dup2(stdout_capture.as_raw_fd(), 1);
        libc::dup2(stderr_capture.as_raw_fd(), 2);
    }

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-credential-log-suppression");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    // Build the v3 LiveNode. This is the first thing in this test
    // binary's process to call NT's logger init, so the bolt-v3
    // `LoggerConfig.module_level` filter installed by
    // `make_live_node_config` is the active filter for the rest of
    // the process. The NT credential constructors run inside
    // `LiveNodeBuilder::build` and emit `log::info!` lines from the
    // forbidden modules; the filter must drop every one of them.
    let build_result =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver);
    let build_error = build_result.as_ref().err().map(ToString::to_string);

    // Drop the node (if any) before restoring fds so the LogGuard
    // owned by the LiveNode flushes any buffered NT log lines into
    // our capture files.
    drop(build_result);

    // Give NT's async logger thread time to drain any messages the
    // mpsc channel still holds before we restore the real fds.
    std::thread::sleep(std::time::Duration::from_millis(500));

    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    // Restore real stdout/stderr.
    unsafe {
        libc::dup2(real_stdout, 1);
        libc::dup2(real_stderr, 2);
        libc::close(real_stdout);
        libc::close(real_stderr);
    }

    // Read what NT's logger thread actually wrote.
    stdout_capture
        .seek(SeekFrom::Start(0))
        .expect("stdout seek");
    let mut stdout_text = String::new();
    stdout_capture
        .read_to_string(&mut stdout_text)
        .expect("stdout read");

    stderr_capture
        .seek(SeekFrom::Start(0))
        .expect("stderr seek");
    let mut stderr_text = String::new();
    stderr_capture
        .read_to_string(&mut stderr_text)
        .expect("stderr read");

    assert!(
        build_error.is_none(),
        "v3 LiveNode build must succeed so this test reaches NT credential constructors; error={build_error:?}"
    );

    for marker in FORBIDDEN_CREDENTIAL_MARKERS {
        assert!(
            !stdout_text.contains(marker),
            "NT credential log marker `{marker}` leaked to stdout despite bolt-v3 module_level filter; \
             captured stdout=`{stdout_text}`"
        );
        assert!(
            !stderr_text.contains(marker),
            "NT credential log marker `{marker}` leaked to stderr despite bolt-v3 module_level filter; \
             captured stderr=`{stderr_text}`"
        );
    }
}
