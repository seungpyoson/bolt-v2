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

/// Whether `value` is a lowercase-hex SHA-256 digest.
#[must_use]
pub fn is_lowercase_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::{is_lowercase_sha256_hex, sha256_hex};

    #[test]
    fn empty_input_digest_matches_sha256_test_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn lowercase_sha256_validator_rejects_non_digest_strings() {
        assert!(is_lowercase_sha256_hex(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ));
        assert!(!is_lowercase_sha256_hex(""));
        assert!(!is_lowercase_sha256_hex("abc123"));
        assert!(!is_lowercase_sha256_hex(
            "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
        ));
        assert!(!is_lowercase_sha256_hex(
            "g3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ));
    }
}
