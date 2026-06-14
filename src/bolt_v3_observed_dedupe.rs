use std::collections::BTreeMap;

pub(crate) fn prune_observed_dedupe_entries<K: Ord>(
    entries: &mut BTreeMap<K, u64>,
    now_ns: u64,
    retention_ns: u64,
) {
    entries.retain(|_, observed_at_ns| {
        *observed_at_ns > now_ns || now_ns - *observed_at_ns <= retention_ns
    });
}
