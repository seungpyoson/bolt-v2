use std::sync::{Mutex, MutexGuard, OnceLock};

static LIVE_NODE_BUILD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(crate) fn lock_live_node_build() -> MutexGuard<'static, ()> {
    LIVE_NODE_BUILD_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap()
}
