use std::num::NonZeroU64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PositiveFiniteEvidenceReadCap(NonZeroU64);

impl PositiveFiniteEvidenceReadCap {
    pub fn new(value: u64) -> Result<Self, String> {
        if value == u64::MAX {
            return Err("must be finite".to_string());
        }
        NonZeroU64::new(value)
            .map(Self)
            .ok_or_else(|| "must be a positive integer".to_string())
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    #[must_use]
    pub(crate) fn sentinel(self) -> u64 {
        self.get()
            .checked_add(1)
            .expect("finite current-evidence cap must have a sentinel")
    }
}
