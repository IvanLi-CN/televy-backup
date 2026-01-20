use serde::{Deserialize, Serialize};

use crate::crypto::encrypt_framed;
use crate::{Error, Result};

pub const PACK_TARGET_BYTES: usize = 32 * 1024 * 1024;
pub const PACK_MAX_BYTES: usize = 49 * 1024 * 1024;

const HEADER_TRAILER_BYTES: usize = 4;
const PACK_HEADER_AAD: &[u8] = b"televy.pack.header.v1";

#[derive(Debug, Clone)]
pub struct PackBlob {
    pub chunk_hash: String,
    pub blob: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackHeader {
    pub version: u32,
    pub hash_alg: String,
    pub enc_alg: String,
    pub entries: Vec<PackHeaderEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackHeaderEntry {
    pub chunk_hash: String,
    pub offset: u64,
    pub len: u64,
}

#[derive(Debug, Clone)]
pub struct FinalizedPack {
    pub bytes: Vec<u8>,
    pub entries: Vec<PackHeaderEntry>,
}

#[derive(Debug, Default)]
pub struct PackBuilder {
    blob_bytes: Vec<u8>,
    entries: Vec<PackHeaderEntry>,
}

impl PackBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn blob_len(&self) -> usize {
        self.blob_bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn push_blob(&mut self, blob: PackBlob) -> Result<()> {
        let offset = self.blob_bytes.len();
        self.blob_bytes.extend_from_slice(&blob.blob);
        self.entries.push(PackHeaderEntry {
            chunk_hash: blob.chunk_hash,
            offset: offset as u64,
            len: blob.blob.len() as u64,
        });
        Ok(())
    }

    pub fn finalize_fit(
        &mut self,
        master_key: &[u8; 32],
        hard_max_bytes: usize,
    ) -> Result<(FinalizedPack, Vec<PackBlob>)> {
        if self.entries.is_empty() {
            return Err(Error::InvalidConfig {
                message: "pack finalize called with no entries".to_string(),
            });
        }

        let mut carry = Vec::<PackBlob>::new();

        loop {
            let header = PackHeader {
                version: 1,
                hash_alg: "blake3".to_string(),
                enc_alg: "xchacha20poly1305".to_string(),
                entries: self.entries.clone(),
            };
            let header_json = serde_json::to_vec(&header).map_err(|e| Error::InvalidConfig {
                message: format!("pack header serialize failed: {e}"),
            })?;
            let header_enc = encrypt_framed(master_key, PACK_HEADER_AAD, &header_json)?;

            if header_enc.len() > u32::MAX as usize {
                return Err(Error::InvalidConfig {
                    message: "pack header too large".to_string(),
                });
            }

            let total = self
                .blob_bytes
                .len()
                .saturating_add(header_enc.len())
                .saturating_add(HEADER_TRAILER_BYTES);

            if total <= hard_max_bytes {
                let mut out = Vec::with_capacity(total);
                out.extend_from_slice(&self.blob_bytes);
                out.extend_from_slice(&header_enc);
                out.extend_from_slice(&(header_enc.len() as u32).to_le_bytes());
                return Ok((
                    FinalizedPack {
                        bytes: out,
                        entries: self.entries.clone(),
                    },
                    carry,
                ));
            }

            if self.entries.len() <= 1 {
                return Err(Error::InvalidConfig {
                    message: format!(
                        "single blob too large for pack (hard_max_bytes={hard_max_bytes})"
                    ),
                });
            }

            let last = self.entries.pop().expect("len > 1");
            let start = last.offset as usize;
            let len = last.len as usize;
            if start + len != self.blob_bytes.len() {
                return Err(Error::Integrity {
                    message: "pack builder state invalid (non-tail pop)".to_string(),
                });
            }
            let blob = self.blob_bytes.split_off(start);
            if blob.len() != len {
                return Err(Error::Integrity {
                    message: "pack builder state invalid (len mismatch)".to_string(),
                });
            }
            carry.insert(
                0,
                PackBlob {
                    chunk_hash: last.chunk_hash,
                    blob,
                },
            );
        }
    }

    pub fn reset(&mut self) {
        self.blob_bytes.clear();
        self.entries.clear();
    }
}

pub fn pack_payload_end(bytes: &[u8]) -> Result<usize> {
    if bytes.len() < HEADER_TRAILER_BYTES {
        return Err(Error::Integrity {
            message: "pack too small".to_string(),
        });
    }
    let trailer = &bytes[bytes.len() - HEADER_TRAILER_BYTES..];
    let header_len = u32::from_le_bytes(trailer.try_into().expect("len=4")) as usize;
    if header_len > bytes.len() - HEADER_TRAILER_BYTES {
        return Err(Error::Integrity {
            message: "pack header length out of bounds".to_string(),
        });
    }
    Ok(bytes.len() - HEADER_TRAILER_BYTES - header_len)
}

#[cfg(test)]
pub fn read_pack_header(master_key: &[u8; 32], bytes: &[u8]) -> Result<PackHeader> {
    use crate::crypto::decrypt_framed;

    let payload_end = pack_payload_end(bytes)?;
    let header_enc = &bytes[payload_end..bytes.len() - HEADER_TRAILER_BYTES];
    let header_json = decrypt_framed(master_key, PACK_HEADER_AAD, header_enc)?;
    let header: PackHeader =
        serde_json::from_slice(&header_json).map_err(|e| Error::Integrity {
            message: format!("invalid pack header json: {e}"),
        })?;

    if header.version != 1 {
        return Err(Error::InvalidConfig {
            message: format!("unsupported pack header version: {}", header.version),
        });
    }
    if header.hash_alg != "blake3" {
        return Err(Error::InvalidConfig {
            message: format!("unsupported pack hash_alg: {}", header.hash_alg),
        });
    }
    if header.enc_alg != "xchacha20poly1305" {
        return Err(Error::InvalidConfig {
            message: format!("unsupported pack enc_alg: {}", header.enc_alg),
        });
    }

    Ok(header)
}

pub fn extract_pack_blob(bytes: &[u8], offset: u64, len: u64) -> Result<&[u8]> {
    let payload_end = pack_payload_end(bytes)? as u64;
    let end = offset.checked_add(len).ok_or_else(|| Error::Integrity {
        message: "pack slice overflow".to_string(),
    })?;
    if end > payload_end {
        return Err(Error::Integrity {
            message: "pack slice out of bounds".to_string(),
        });
    }
    let offset = offset as usize;
    let end = end as usize;
    Ok(&bytes[offset..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_header_round_trip() {
        let key = [7u8; 32];
        let mut b = PackBuilder::new();
        b.push_blob(PackBlob {
            chunk_hash: "a".repeat(64),
            blob: vec![1, 2, 3],
        })
        .unwrap();
        b.push_blob(PackBlob {
            chunk_hash: "b".repeat(64),
            blob: vec![4, 5],
        })
        .unwrap();

        let (pack, carry) = b.finalize_fit(&key, PACK_MAX_BYTES).unwrap();
        assert!(carry.is_empty());

        let hdr = read_pack_header(&key, &pack.bytes).unwrap();
        assert_eq!(hdr.version, 1);
        assert_eq!(hdr.entries.len(), 2);
        assert_eq!(hdr.entries[0].offset, 0);
        assert_eq!(hdr.entries[0].len, 3);
        assert_eq!(hdr.entries[1].offset, 3);
        assert_eq!(hdr.entries[1].len, 2);
    }

    #[test]
    fn pack_finalize_pops_until_fit() {
        let key = [7u8; 32];
        let mut b = PackBuilder::new();
        for i in 0..20 {
            b.push_blob(PackBlob {
                chunk_hash: format!("{i:064x}"),
                blob: vec![0u8; 80],
            })
            .unwrap();
        }

        let hard_max = 512;
        let (pack, carry) = b.finalize_fit(&key, hard_max).unwrap();
        assert!(pack.bytes.len() <= hard_max);
        assert!(!carry.is_empty());
    }
}
