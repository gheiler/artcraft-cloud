use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};

use crate::error::Error;

pub fn hash_password(password: &str, memory_kib: u32, time: u32, parallelism: u32) -> Result<String, Error> {
    let argon = argon(memory_kib, time, parallelism)?;
    let salt = SaltString::generate(&mut argon2::password_hash::rand_core::OsRng);
    let hash = argon.hash_password(password.as_bytes(), &salt).map_err(|err| Error::Invalid(err.to_string()))?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, encoded: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(encoded) else {
        return false;
    };
    Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok()
}

fn argon(memory_kib: u32, time: u32, parallelism: u32) -> Result<Argon2<'static>, Error> {
    let params = Params::new(memory_kib, time, parallelism, None).map_err(|err| Error::Invalid(err.to_string()))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

pub fn sign(secret: &[u8], message: &str) -> Result<String, Error> {
    use hmac::Mac;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret).map_err(|_| Error::Invalid("secret".into()))?;
    mac.update(message.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

pub fn verify_mac(secret: &[u8], message: &str, signature: &str) -> bool {
    let Ok(expected) = sign(secret, message) else {
        return false;
    };
    // Constant-time compare of the hex strings is good enough: both are hex of equal length.
    if expected.len() != signature.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in expected.bytes().zip(signature.bytes()) {
        diff |= a ^ b;
    }
    diff == 0
}
