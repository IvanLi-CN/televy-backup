use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{AeadCore, AeadInPlace, KeyInit, OsRng},
};

use crate::{Error, Result};

pub const FRAMING_VERSION: u8 = 0x01;
pub const NONCE_LEN: usize = 24;

pub fn encrypt_framed(master_key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(master_key.into());
    let nonce: XNonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);

    let mut buffer = plaintext.to_vec();
    cipher
        .encrypt_in_place(&nonce, aad, &mut buffer)
        .map_err(|_| Error::Crypto)?;

    let mut out = Vec::with_capacity(1 + NONCE_LEN + buffer.len());
    out.push(FRAMING_VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&buffer);
    Ok(out)
}

#[allow(dead_code)]
pub fn decrypt_framed(master_key: &[u8; 32], aad: &[u8], framed: &[u8]) -> Result<Vec<u8>> {
    if framed.len() < 1 + NONCE_LEN {
        return Err(Error::Crypto);
    }
    if framed[0] != FRAMING_VERSION {
        return Err(Error::Crypto);
    }

    let cipher = XChaCha20Poly1305::new(master_key.into());
    let nonce = XNonce::from_slice(&framed[1..1 + NONCE_LEN]);

    let mut buffer = framed[1 + NONCE_LEN..].to_vec();
    cipher
        .decrypt_in_place(nonce, aad, &mut buffer)
        .map_err(|_| Error::Crypto)?;
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framed_round_trip() {
        let key = [1u8; 32];
        let aad = b"aad";
        let msg = b"hello";

        let enc = encrypt_framed(&key, aad, msg).unwrap();
        let dec = decrypt_framed(&key, aad, &enc).unwrap();
        assert_eq!(dec, msg);

        assert!(decrypt_framed(&key, b"wrong", &enc).is_err());
    }
}
