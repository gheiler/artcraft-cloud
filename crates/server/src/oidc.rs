//! Seam for a later OpenID Connect login.
//!
//! Local passwords are the only authenticator in this version. The server
//! refuses to start when `ARTCRAFT_OIDC_ISSUER` is set, so a deployment that
//! expects SSO does not silently fall open onto passwords alone. A future
//! `OidcAuthenticator` belongs beside [`LocalPassword`]: verify the issuer's
//! ID token, then map `sub` onto a row in `users`.

use crate::error::Error;

pub trait Authenticator {
    fn method(&self) -> &'static str;
}

pub struct LocalPassword;

impl Authenticator for LocalPassword {
    fn method(&self) -> &'static str {
        "local-password"
    }
}

/// Future OIDC client settings. Fields are read by the not-yet-implemented
/// callback; keeping them public documents the configuration the seam expects.
#[allow(dead_code)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
}

pub fn reject_if_configured(issuer: Option<&str>) -> Result<(), Error> {
    if issuer.is_some_and(|value| !value.is_empty()) {
        return Err(Error::Invalid(
            "OIDC SSO is not implemented yet (ARTCRAFT_OIDC_ISSUER is set). This version only has local accounts."
                .into(),
        ));
    }
    Ok(())
}

pub fn active_method() -> &'static str {
    LocalPassword.method()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oidc_is_a_hard_error_until_it_exists() {
        assert!(reject_if_configured(None).is_ok());
        assert!(reject_if_configured(Some("")).is_ok());
        assert!(reject_if_configured(Some("https://idp.example")).is_err());
        assert_eq!(active_method(), "local-password");
        let _ = OidcConfig {
            issuer: "https://idp.example".into(),
            client_id: "artcraft-cloud".into(),
            client_secret: "secret".into(),
        };
    }
}
