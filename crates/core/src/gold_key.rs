use base64::Engine;

use crate::{Error, Result};

pub const GOLD_KEY_PREFIX: &str = "TBK1:";
pub const GOLD_KEY_FORMAT: &str = "tbk1";

pub fn encode_gold_key(master_key: &[u8; 32]) -> String {
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(master_key);
    format!("{GOLD_KEY_PREFIX}{b64}")
}

pub fn decode_gold_key(s: &str) -> Result<[u8; 32]> {
    let rest = s.trim().strip_prefix(GOLD_KEY_PREFIX).ok_or_else(|| Error::InvalidConfig {
        message: "invalid gold key (missing TBK1: prefix)".to_string(),
    })?;

    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(rest.as_bytes())
        .map_err(|e| Error::InvalidConfig {
            message: format!("invalid gold key (bad base64url): {e}"),
        })?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| Error::InvalidConfig {
        message: "invalid gold key (wrong length)".to_string(),
    })?;
    Ok(arr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tbk1_round_trip() {
        let key = [7u8; 32];
        let s = encode_gold_key(&key);
        assert!(s.starts_with(GOLD_KEY_PREFIX));
        let parsed = decode_gold_key(&s).unwrap();
        assert_eq!(parsed, key);
    }
}

