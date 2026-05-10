use std::sync::{Mutex, MutexGuard, OnceLock};

static LIVE_NODE_BUILD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Serializes LiveNode construction inside one test binary that touches NT global runtime state.
///
/// Cargo integration tests are separate binaries, so each including binary gets its own process-local
/// lock. That is intentional: the lock prevents concurrent construction within one binary without
/// exposing test support through the production crate API.
pub(crate) fn lock_live_node_build() -> MutexGuard<'static, ()> {
    LIVE_NODE_BUILD_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
