//! Test-only helpers for Nautilus LiveNode construction.

use std::sync::{Mutex, MutexGuard, OnceLock};

static LIVE_NODE_BUILD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Serializes LiveNode construction inside unit-test binaries that touch NT global runtime state.
pub(crate) fn lock_live_node_build() -> MutexGuard<'static, ()> {
    LIVE_NODE_BUILD_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap()
}
