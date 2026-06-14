//! Single owner of the pipeline's content-hash encoding.
//!
//! Every artifact module hashes serialized bytes with SHA-256 and records the
//! lowercase-hex digest; this module is the one implementation of that
//! encoding so the digest format cannot drift between producers.

use sha2::{Digest, Sha256};

/// Lowercase-hex SHA-256 digest of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    #[test]
    fn empty_input_digest_matches_sha256_test_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
