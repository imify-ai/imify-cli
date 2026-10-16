//! JWT payload decoding and the claim checks on ID tokens received directly from the
//! token endpoint.
//!
//! The signature is not verified: the token arrives over a direct TLS connection to the
//! token endpoint (OIDC Core 1.0, section 3.1.3.7).

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::{Error, LoginFailure};

/// Latest accepted `exp`, in seconds from now. It keeps every accepted expiry far below
/// `i64::MAX`, the limit of `SystemTime` on Unix.
const MAX_LIFETIME_SECS: u64 = 24 * 60 * 60;

/// Clock difference tolerated on `exp`, in seconds.
const EXPIRY_LEEWAY_SECS: u64 = 60;

/// The claim values an ID token must carry.
#[derive(Debug)]
pub(crate) struct Expected<'a> {
    pub(crate) issuer: &'a str,
    pub(crate) client_id: &'a str,
    /// Checked on the code exchange; `None` on a refresh.
    pub(crate) nonce: Option<&'a str>,
    /// The `sub` of the login being refreshed; `None` on the code exchange and for
    /// credentials stored without one.
    pub(crate) subject: Option<&'a str>,
    pub(crate) now: SystemTime,
}

/// The claims of an ID token that passed [`check`].
#[derive(Debug)]
pub(crate) struct Checked {
    /// Unix seconds, from the `exp` claim.
    pub(crate) expires_at: u64,
    /// The `sub` claim.
    pub(crate) subject: String,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: Audience,
    azp: Option<String>,
    exp: u64,
    nonce: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn contains(&self, client_id: &str) -> bool {
        match self {
            Self::One(audience) => audience == client_id,
            Self::Many(audiences) => audiences.iter().any(|audience| audience == client_id),
        }
    }

    fn has_several_entries(&self) -> bool {
        matches!(self, Self::Many(audiences) if audiences.len() > 1)
    }
}

/// Decodes the payload of `token` and checks its claims against `expected`.
///
/// `aud` must contain the client id; with more than one entry, `azp` must equal it.
/// `exp` may lie up to 60 s in the past and at most one day in the future.
pub(crate) fn check(token: &str, expected: &Expected<'_>) -> Result<Checked, Error> {
    let claims = decode(token)?;
    if claims.iss != expected.issuer {
        return Err(invalid("the ID token was issued by another issuer"));
    }
    if !claims.aud.contains(expected.client_id) {
        return Err(invalid(
            "the ID token audience does not contain the client id",
        ));
    }
    if claims.aud.has_several_entries() && claims.azp.as_deref() != Some(expected.client_id) {
        return Err(invalid(
            "the ID token has several audiences and its azp is not the client id",
        ));
    }
    let now = expected
        .now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    if claims.exp.saturating_add(EXPIRY_LEEWAY_SECS) <= now {
        return Err(invalid("the ID token has expired"));
    }
    if claims.exp > now.saturating_add(MAX_LIFETIME_SECS) {
        return Err(invalid("the ID token expires more than one day from now"));
    }
    if let Some(nonce) = expected.nonce
        && claims.nonce.as_deref() != Some(nonce)
    {
        return Err(invalid("the ID token nonce does not match this login"));
    }
    if let Some(subject) = expected.subject
        && claims.sub != subject
    {
        return Err(invalid(
            "the ID token belongs to another user than the login",
        ));
    }
    Ok(Checked {
        expires_at: claims.exp,
        subject: claims.sub,
    })
}

fn decode(token: &str) -> Result<Claims, Error> {
    payload(token).map_err(|error| match error {
        PayloadError::NotJwt => invalid("the ID token is not a JWT"),
        PayloadError::NotBase64Url => invalid("the ID token payload is not base64url"),
        PayloadError::Claims => invalid("the ID token payload lacks iss, sub, aud or exp"),
    })
}

/// Why [`payload`] cannot decode a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PayloadError {
    /// The token does not have three dot-separated segments.
    NotJwt,
    /// The payload segment is not base64url.
    NotBase64Url,
    /// The payload is not JSON of the shape `T`.
    Claims,
}

/// Decodes the payload segment of the JWT `token` as `T`, without verifying the signature.
pub(crate) fn payload<T: DeserializeOwned>(token: &str) -> Result<T, PayloadError> {
    let mut parts = token.split('.');
    let (Some(_header), Some(payload), Some(_signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(PayloadError::NotJwt);
    };
    let payload = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| PayloadError::NotBase64Url)?;
    serde_json::from_slice(&payload).map_err(|_| PayloadError::Claims)
}

fn invalid(message: &str) -> Error {
    Error::Login {
        reason: LoginFailure::InvalidIdToken,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests;
