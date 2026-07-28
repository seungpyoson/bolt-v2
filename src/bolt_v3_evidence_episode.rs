//! Suppression state for a producer that records once per semantic episode.
//!
//! Three strategy producers held the same thing under three names: the identity
//! of the episode currently being observed, plus a finite mask of which novelty
//! bits have already been recorded within it. Each hand-rolled the same compare,
//! the same `&` test and the same `|` update, and a producer that got its
//! identity wrong -- by keying on a field that varies while the episode does not
//! -- re-emitted on every tick, which is the defect #1354 exists to stop.
//!
//! The identity stays with the producer, because only the producer knows what
//! its episode is. What moves here is the part that was copied.
//!
//! The maker's requote-throttle suppression deliberately does not use this. It
//! holds *many* live episodes at once, one per market and leg, bounded by an
//! explicit clear when a leg unblocks; a single current episode would let one
//! market's record displace another's. The choice between one current episode
//! and a set of them is a real per-producer decision, so it is not abstracted
//! away -- only the mask arithmetic is.

/// The episode a producer is currently recording against, if any.
///
/// `K` is the producer's semantic identity. A key that differs from the current
/// one opens a new episode, which is what bounds this to one entry however often
/// the identity churns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentEpisode<K> {
    open: Option<Episode<K>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Episode<K> {
    key: K,
    seen: u16,
}

impl<K> Default for CurrentEpisode<K> {
    fn default() -> Self {
        Self { open: None }
    }
}

impl<K: PartialEq> CurrentEpisode<K> {
    /// Whether `bit` is new for `key`, marking it as seen either way.
    ///
    /// Marking happens here rather than after the caller's write, which is a
    /// deliberate failure-semantics choice and the one place it is now made. A
    /// telemetry write that fails must not leave the episode unmarked, because
    /// the next tick would then present the same semantic state as new and the
    /// producer would retry on every tick for as long as the sink stayed broken
    /// -- turning a failing sink into the flood this suppression exists to
    /// prevent. The cost is that a dropped write is not retried. Both producers
    /// already behaved this way -- each swallows its own write failure, so the
    /// mark ran whether the write landed or not; only one of them said so.
    pub fn admit(&mut self, key: K, bit: u16) -> bool {
        match &mut self.open {
            Some(open) if open.key == key => {
                if open.seen & bit != 0 {
                    return false;
                }
                open.seen |= bit;
                true
            }
            _ => {
                self.open = Some(Episode { key, seen: bit });
                true
            }
        }
    }

    /// Forget the open episode, so the next observation of any key is new.
    pub fn clear(&mut self) {
        self.open = None;
    }

    /// Whether an episode is currently open.
    ///
    /// Exists for the one caller that has no behavioural signal to assert on:
    /// the exit-decision producer returns nothing when it suppresses, so
    /// "the episode stayed marked through a failed write" can only be asked of
    /// the state. Every other producer reports suppression in its return value
    /// and is checked through that instead.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }
}

/// The single bit for a producer whose episode carries no finite novelty axis.
///
/// Suppression then means "record once per identity", which is the degenerate
/// case of the mask rather than a second mechanism.
pub const WHOLE_EPISODE: u16 = 1;

#[cfg(test)]
mod tests {
    use super::{CurrentEpisode, WHOLE_EPISODE};

    #[test]
    fn the_first_observation_of_an_episode_is_new() {
        let mut episode = CurrentEpisode::default();
        assert!(episode.admit("blocked", WHOLE_EPISODE));
    }

    #[test]
    fn the_same_bit_within_one_episode_is_not_new_again() {
        let mut episode = CurrentEpisode::default();
        assert!(episode.admit("blocked", 0b01));
        assert!(!episode.admit("blocked", 0b01));
    }

    #[test]
    fn a_second_bit_within_one_episode_is_new_and_does_not_forget_the_first() {
        let mut episode = CurrentEpisode::default();
        assert!(episode.admit("blocked", 0b01));
        assert!(episode.admit("blocked", 0b10));
        assert!(!episode.admit("blocked", 0b01));
        assert!(!episode.admit("blocked", 0b10));
    }

    /// The property that makes this bounded: an identity that churns cannot
    /// accumulate, because a new key replaces the episode rather than joining a
    /// set. Alternating A/B/A therefore records each time, which is correct --
    /// the episode genuinely ended -- and holds exactly one entry throughout.
    #[test]
    fn a_new_identity_opens_a_new_episode_rather_than_accumulating() {
        let mut episode = CurrentEpisode::default();
        assert!(episode.admit("a", 0b01));
        assert!(episode.admit("b", 0b01));
        assert!(episode.admit("a", 0b01));
        assert!(!episode.admit("a", 0b01));
    }

    #[test]
    fn clearing_makes_the_next_observation_new() {
        let mut episode = CurrentEpisode::default();
        assert!(episode.admit("blocked", WHOLE_EPISODE));
        episode.clear();
        assert!(episode.admit("blocked", WHOLE_EPISODE));
    }

    /// Marking is unconditional, so a caller whose write fails does not get a
    /// second chance at the same bit. Pinned because it is a deliberate trade,
    /// not an accident of ordering.
    #[test]
    fn a_bit_is_marked_even_though_the_caller_may_fail_to_write_it() {
        let mut episode = CurrentEpisode::default();
        assert!(episode.admit("blocked", WHOLE_EPISODE));
        // The caller's write failed here; nothing tells the episode so.
        assert!(!episode.admit("blocked", WHOLE_EPISODE));
    }
}
