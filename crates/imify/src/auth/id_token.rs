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

/// The claim values an ID token must carry.
#[derive(Debug)]
pub(crate) struct Expected<'a> {
    pub(crate) issuer: &'a str,
    pub(crate) client_id: &'a str,
    /// Checked on the code exchange; `None` on a refresh.
    pub(crate) nonce: Option<&'a str>,
    pub(crate) now: SystemTime,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: Audience,
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
}

/// Decodes the payload of `token`, checks its claims against `expected`, and returns its
/// expiry in Unix seconds.
pub(crate) fn check(token: &str, expected: &Expected<'_>) -> Result<u64, Error> {
    let claims = decode(token)?;
    if claims.iss != expected.issuer {
        return Err(invalid("the ID token was issued by another issuer"));
    }
    if !claims.aud.contains(expected.client_id) {
        return Err(invalid(
            "the ID token audience does not contain the client id",
        ));
    }
    let now = expected
        .now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    if claims.exp <= now {
        return Err(invalid("the ID token has expired"));
    }
    if let Some(nonce) = expected.nonce
        && claims.nonce.as_deref() != Some(nonce)
    {
        return Err(invalid("the ID token nonce does not match this login"));
    }
    Ok(claims.exp)
}

fn decode(token: &str) -> Result<Claims, Error> {
    payload(token).map_err(|error| match error {
        PayloadError::NotJwt => invalid("the ID token is not a JWT"),
        PayloadError::NotBase64Url => invalid("the ID token payload is not base64url"),
        PayloadError::Claims => invalid("the ID token payload lacks iss, aud or exp"),
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
