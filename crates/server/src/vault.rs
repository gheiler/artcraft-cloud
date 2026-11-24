//! Secrets kept in the database (the mail server's admin credential, each mailbox's password)
//! are sealed with ChaCha20-Poly1305 under a key derived from the install's `secret.key`
//! (in the data directory, owner-readable only). A copy of `db.sqlite` alone does not reveal them.
//! Each value is bound to where it belongs (`context`), so one cannot be swapped for another.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::RngCore;

use crate::error::Error;

const VERSION: u8 = 1;

fn cipher(secret: &[u8]) -> Result<ChaCha20Poly1305, Error> {
    use hmac::Mac;
    let mut mac = <hmac::Hmac<sha2::Sha256> as hmac::Mac>::new_from_slice(secret).map_err(|_| Error::Invalid("secret".into()))?;
    mac.update(b"artcraft-cloud vault v1");
    let key = mac.finalize().into_bytes();
    Ok(ChaCha20Poly1305::new(Key::from_slice(&key)))
}

/// `version || nonce (12) || ciphertext+tag`.
pub fn seal(secret: &[u8], context: &str, plain: &str) -> Result<Vec<u8>, Error> {
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let sealed = cipher(secret)?
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain.as_bytes(), aad: context.as_bytes() })
        .map_err(|_| Error::Invalid("could not seal a secret".into()))?;
    let mut out = Vec::with_capacity(1 + 12 + sealed.len());
    out.push(VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

pub fn open(secret: &[u8], context: &str, sealed: &[u8]) -> Result<String, Error> {
    if sealed.len() < 1 + 12 + 16 || sealed[0] != VERSION {
        return Err(Error::Invalid("a stored secret is damaged".into()));
    }
    let plain = cipher(secret)?
        .decrypt(Nonce::from_slice(&sealed[1..13]), Payload { msg: &sealed[13..], aad: context.as_bytes() })
        .map_err(|_| Error::Invalid("a stored secret cannot be read with this secret.key".into()))?;
    String::from_utf8(plain).map_err(|_| Error::Invalid("a stored secret is damaged".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_secrets_open_only_with_their_key_and_context() {
        let key = [7u8; 32];
        let sealed = seal(&key, "mailbox:3", "s3cret").unwrap();
        assert!(!sealed.windows(6).any(|w| w == b"s3cret"));
        assert_eq!(open(&key, "mailbox:3", &sealed).unwrap(), "s3cret");
        assert!(open(&key, "mailbox:4", &sealed).is_err(), "bound to its context");
        assert!(open(&[8u8; 32], "mailbox:3", &sealed).is_err(), "bound to the key");
        let mut bent = sealed.clone();
        let last = bent.len() - 1;
        bent[last] ^= 1;
        assert!(open(&key, "mailbox:3", &bent).is_err(), "tampering is caught");
        assert_ne!(seal(&key, "mailbox:3", "s3cret").unwrap(), sealed, "fresh nonce each time");
    }
}
