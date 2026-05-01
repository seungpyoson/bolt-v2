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

mod support;

use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::io::AsRawFd;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config, bolt_v3_live_node::build_bolt_v3_live_node_with_summary,
};
use tempfile::tempfile;

const FORBIDDEN_CREDENTIAL_MARKERS: &[&str] = &[
    // nautilus_polymarket::common::credential::Credentials::resolve
    "Polymarket credentials resolved",
    // nautilus_binance::common::credential::SigningCredential::new
    "Auto-detected Ed25519 API key",
    "Using HMAC SHA256 API key",
];

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
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

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
