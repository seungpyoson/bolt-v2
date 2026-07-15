use std::cmp::Ordering;
use zeroize::Zeroize;

use super::bounded::{ProjectionClass, RedactedProjection, keyed_digest};
use super::config::ResolvedRedemptionCredentials;

const NONCE_BYTES: usize = 32;
const MAX_DECIMAL_BYTES: usize = 78;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Zeroize)]
pub struct SafeNonce([u8; NONCE_BYTES]);

impl SafeNonce {
    pub const ZERO: Self = Self([0; NONCE_BYTES]);
    pub const MAX: Self = Self([u8::MAX; NONCE_BYTES]);

    pub const fn from_be_bytes(bytes: [u8; NONCE_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn from_decimal(value: &str) -> Result<Self, NonceError> {
        let bytes = value.as_bytes();
        if bytes.is_empty()
            || bytes.len() > MAX_DECIMAL_BYTES
            || (bytes.len() > 1 && bytes[0] == b'0')
        {
            return Err(NonceError::Malformed);
        }
        let mut output = [0u8; NONCE_BYTES];
        for digit in bytes {
            if !digit.is_ascii_digit() {
                return Err(NonceError::Malformed);
            }
            let mut carry = u16::from(*digit - b'0');
            for byte in output.iter_mut().rev() {
                let next = u16::from(*byte) * 10 + carry;
                *byte = next as u8;
                carry = next >> 8;
            }
            if carry != 0 {
                return Err(NonceError::Overflow);
            }
        }
        Ok(Self(output))
    }

    pub fn successor(self) -> Option<Self> {
        let mut next = self.0;
        for byte in next.iter_mut().rev() {
            let (value, overflow) = byte.overflowing_add(1);
            *byte = value;
            if !overflow {
                return Some(Self(next));
            }
        }
        None
    }

    pub const fn is_max(self) -> bool {
        let mut index = 0;
        while index < NONCE_BYTES {
            if self.0[index] != u8::MAX {
                return false;
            }
            index += 1;
        }
        true
    }

    pub fn relation(self, other: Self) -> Ordering {
        self.cmp(&other)
    }

    pub fn projection(self, credentials: &ResolvedRedemptionCredentials) -> RedactedProjection {
        RedactedProjection {
            class: ProjectionClass::Nonce,
            item_count: 1,
            byte_len: NONCE_BYTES,
            keyed_digest: keyed_digest(credentials.redaction_hmac_key(), &self.0),
            key_version: credentials.key_version(),
        }
    }

    pub(super) const fn as_word(&self) -> &[u8; NONCE_BYTES] {
        &self.0
    }

    pub(super) fn write_decimal(&self, output: &mut [u8; MAX_DECIMAL_BYTES]) -> usize {
        if *self == Self::ZERO {
            output[0] = b'0';
            return 1;
        }
        let mut quotient = self.0;
        let mut reverse = [0u8; MAX_DECIMAL_BYTES];
        let mut len = 0;
        while quotient.iter().any(|byte| *byte != 0) {
            let mut remainder = 0u16;
            for byte in &mut quotient {
                let value = (remainder << 8) | u16::from(*byte);
                *byte = (value / 10) as u8;
                remainder = value % 10;
            }
            reverse[len] = b'0' + remainder as u8;
            len += 1;
        }
        for index in 0..len {
            output[index] = reverse[len - index - 1];
        }
        len
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonceError {
    Malformed,
    Overflow,
}
