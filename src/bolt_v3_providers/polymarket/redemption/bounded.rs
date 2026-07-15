use std::io::Read;

use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionClass {
    Credentials,
    RequestBody,
    AuthorizationHeaders,
    RelayerResponse,
    ChainResponse,
    QuerySet,
    Nonce,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedactedProjection {
    pub class: ProjectionClass,
    pub item_count: usize,
    pub byte_len: usize,
    pub keyed_digest: [u8; 32],
    pub key_version: u32,
}

pub(super) struct CappedBytes {
    storage: Zeroizing<Box<[u8]>>,
    len: usize,
}

impl CappedBytes {
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            storage: Zeroizing::new(vec![0; capacity].into_boxed_slice()),
            len: 0,
        }
    }

    pub(super) fn read_with_probe(
        mut reader: impl Read,
        limit: usize,
        probe_bytes: usize,
        projection_key: &[u8],
        key_version: u32,
        class: ProjectionClass,
    ) -> Result<Self, CappedIoError> {
        let charged = limit
            .checked_add(probe_bytes)
            .ok_or(CappedIoError::InvalidLimit)?;
        let mut value = Self::with_capacity(charged);
        while value.len < charged {
            let read = reader
                .read(&mut value.storage[value.len..])
                .map_err(|_| CappedIoError::Read)?;
            if read == 0 {
                break;
            }
            value.len += read;
        }
        if value.len > limit {
            return Err(CappedIoError::Oversize(value.projection(
                class,
                0,
                projection_key,
                key_version,
            )));
        }
        Ok(value)
    }

    pub(super) fn push(&mut self, byte: u8) -> Result<(), CappedIoError> {
        if self.len == self.storage.len() {
            return Err(CappedIoError::Capacity);
        }
        self.storage[self.len] = byte;
        self.len += 1;
        Ok(())
    }

    pub(super) fn extend(&mut self, bytes: &[u8]) -> Result<(), CappedIoError> {
        let end = self
            .len
            .checked_add(bytes.len())
            .ok_or(CappedIoError::Capacity)?;
        if end > self.storage.len() {
            return Err(CappedIoError::Capacity);
        }
        self.storage[self.len..end].copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }

    pub(super) fn append_hex(&mut self, bytes: &[u8]) -> Result<(), CappedIoError> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in bytes {
            self.push(HEX[(byte >> 4) as usize])?;
            self.push(HEX[(byte & 0x0f) as usize])?;
        }
        Ok(())
    }

    pub(super) fn append_json_string(&mut self, value: &[u8]) -> Result<(), CappedIoError> {
        self.push(b'"')?;
        for byte in value {
            match byte {
                b'"' => self.extend(br#"\""#)?,
                b'\\' => self.extend(br#"\\"#)?,
                0x08 => self.extend(br"\b")?,
                0x0c => self.extend(br"\f")?,
                b'\n' => self.extend(br"\n")?,
                b'\r' => self.extend(br"\r")?,
                b'\t' => self.extend(br"\t")?,
                0x00..=0x1f => {
                    self.extend(br"\u00")?;
                    self.append_hex(&[*byte])?;
                }
                _ => self.push(*byte)?,
            }
        }
        self.push(b'"')
    }

    pub(super) fn as_slice(&self) -> &[u8] {
        &self.storage[..self.len]
    }

    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn hash(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};

        Sha256::digest(self.as_slice()).into()
    }

    pub(super) fn projection(
        &self,
        class: ProjectionClass,
        item_count: usize,
        projection_key: &[u8],
        key_version: u32,
    ) -> RedactedProjection {
        RedactedProjection {
            class,
            item_count,
            byte_len: self.len,
            keyed_digest: keyed_digest(projection_key, self.as_slice()),
            key_version,
        }
    }
}

pub(super) fn keyed_digest(key: &[u8], value: &[u8]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts every key length");
    mac.update(value);
    mac.finalize().into_bytes().into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CappedIoError {
    InvalidLimit,
    Read,
    Capacity,
    Oversize(RedactedProjection),
}
