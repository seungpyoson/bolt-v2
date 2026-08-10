use std::sync::{Mutex, OnceLock};

#[derive(Default)]
struct CapturingLogger {
    records: Mutex<Vec<(log::Level, String)>>,
}

impl log::Log for CapturingLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        self.records
            .lock()
            .expect("capturing logger mutex poisoned")
            .push((record.level(), record.args().to_string()));
    }

    fn flush(&self) {}
}

impl CapturingLogger {
    fn reset(&self) {
        self.records
            .lock()
            .expect("capturing logger mutex poisoned")
            .clear();
    }

    fn records(&self) -> Vec<(log::Level, String)> {
        self.records
            .lock()
            .expect("capturing logger mutex poisoned")
            .clone()
    }
}

static CAPTURING_LOGGER: OnceLock<&'static CapturingLogger> = OnceLock::new();
static CAPTURE_SESSION: Mutex<()> = Mutex::new(());
const LOG_CAPTURE_CHILD_ENV: &str = "BOLT_TEST_LOG_CAPTURE_CASE";

pub(crate) fn enter_isolated_log_capture(test_filter: &str, case: &str) -> bool {
    if std::env::var(LOG_CAPTURE_CHILD_ENV).ok().as_deref() == Some(case) {
        return true;
    }

    let output = std::process::Command::new(
        std::env::current_exe().expect("current test binary should be available"),
    )
    .arg(test_filter)
    .arg("--nocapture")
    .arg("--test-threads=1")
    .env(LOG_CAPTURE_CHILD_ENV, case)
    .output()
    .expect("isolated log-capture test should launch");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "isolated log-capture test failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout,
        stderr
    );
    assert!(
        stdout.contains("running 1 test"),
        "log-capture filter `{test_filter}` must run exactly one test\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    false
}

fn install_capturing_logger() -> &'static CapturingLogger {
    let logger = CAPTURING_LOGGER.get_or_init(|| Box::leak(Box::new(CapturingLogger::default())));
    log::set_logger(*logger).expect("the shared test logger must be the sole global logger");
    log::set_max_level(log::LevelFilter::Trace);
    *logger
}

pub(crate) fn with_captured_logs<R>(action: impl FnOnce() -> R) -> (R, Vec<(log::Level, String)>) {
    let _session = CAPTURE_SESSION
        .lock()
        .expect("capturing logger session mutex poisoned");
    let logger = CAPTURING_LOGGER
        .get()
        .copied()
        .unwrap_or_else(install_capturing_logger);
    logger.reset();
    let result = action();
    (result, logger.records())
}
