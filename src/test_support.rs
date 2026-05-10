//! Test-only helpers for Nautilus LiveNode construction.

mod live_node_build_lock;

pub(crate) use live_node_build_lock::lock_live_node_build;

#[cfg(test)]
mod tests {
    use super::lock_live_node_build;

    #[test]
    fn live_node_build_lock_recovers_after_poisoned_guard() {
        let poisoned = std::panic::catch_unwind(|| {
            let _guard = lock_live_node_build();
            panic!("poison LiveNode build lock");
        });
        assert!(poisoned.is_err());

        let _guard = lock_live_node_build();
    }
}
