//! Session bearer-token wire format, validation and hashing.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::rand_core::TryRng;
use rand::rngs::SysRng;
use sha2::{Digest, Sha256};

const TOKEN_BYTES: usize = 64;
const TOKEN_LENGTH: usize = 86;
const HASH_BYTES: usize = 32;

#[derive(Clone)]
pub(crate) struct ValidatedToken {
    value: String,
}

impl ValidatedToken {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        if value.len() != TOKEN_LENGTH {
            return None;
        }

        let decoded = URL_SAFE_NO_PAD.decode(value).ok()?;
        if decoded.len() != TOKEN_BYTES || URL_SAFE_NO_PAD.encode(decoded) != value {
            return None;
        }

        Some(Self {
            value: value.to_owned(),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
    }
}

impl std::fmt::Debug for ValidatedToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ValidatedToken(<redacted>)")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SessionId {
    digest: [u8; HASH_BYTES],
}

impl SessionId {
    pub(crate) fn from_validated(token: &ValidatedToken) -> Self {
        Self {
            digest: Sha256::digest(token.as_str().as_bytes()).into(),
        }
    }

    /// Stable opaque key for staging ownership. It never exposes the token.
    pub(crate) fn as_hex(&self) -> String {
        crate::hex::encode(&self.digest)
    }
}

impl std::fmt::Debug for SessionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SessionId(<redacted>)")
    }
}

pub(crate) struct NewToken {
    raw: ValidatedToken,
    id: SessionId,
}

impl NewToken {
    pub(crate) fn raw(&self) -> &ValidatedToken {
        &self.raw
    }

    pub(crate) fn id(&self) -> SessionId {
        self.id
    }
}

pub(crate) fn generate() -> Result<NewToken, TokenError> {
    let mut bytes = [0u8; TOKEN_BYTES];
    SysRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| TokenError::RandomUnavailable)?;

    let value = URL_SAFE_NO_PAD.encode(bytes);
    let raw = ValidatedToken::parse(&value).ok_or(TokenError::HashingFailed)?;
    let id = SessionId::from_validated(&raw);
    Ok(NewToken { raw, id })
}

#[derive(Debug)]
pub(crate) enum TokenError {
    RandomUnavailable,
    HashingFailed,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RandomUnavailable => formatter.write_str("system random source unavailable"),
            Self::HashingFailed => formatter.write_str("token hashing failed"),
        }
    }
}

impl std::error::Error for TokenError {}

#[cfg(test)]
mod tests;
