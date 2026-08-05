use std::collections::BTreeMap;

pub(crate) trait EpisodeFirstNs {
    fn first_ns(&self) -> u64;
}

pub(crate) fn evict_oldest_episodes_over_cap<V: EpisodeFirstNs>(
    map: &mut BTreeMap<String, V>,
    cap: usize,
) {
    while map.len() > cap {
        let oldest_key = map
            .iter()
            .min_by_key(|(_, episode)| episode.first_ns())
            .map(|(key, _)| key.clone());
        let Some(oldest_key) = oldest_key else {
            break;
        };
        map.remove(&oldest_key);
    }
}
