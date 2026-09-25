use sha2::{Digest, Sha256};

use crate::hex;

const PREFIX: &str = "sha256:";
const DIGEST_LEN: usize = 32;

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ObjectHash([u8; DIGEST_LEN]);

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ArtefactHash([u8; DIGEST_LEN]);

impl ObjectHash {
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        parse_digest(value).map(Self)
    }

    pub(crate) fn as_str(&self) -> String {
        format_digest(&self.0)
    }

    pub(crate) fn fanout(&self) -> (String, String) {
        let digest = hex::encode(&self.0);
        (digest[..2].to_owned(), digest[2..].to_owned())
    }
}

impl ArtefactHash {
    pub(crate) fn of(domain: &[u8], payload: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(domain);
        hasher.update(payload);
        Self(hasher.finalize().into())
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        parse_digest(value).map(Self)
    }

    pub(crate) fn as_str(&self) -> String {
        format_digest(&self.0)
    }

    pub(crate) fn short(&self) -> String {
        short_digest(&self.0)
    }
}

impl std::fmt::Debug for ObjectHash {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ObjectHash(")?;
        formatter.write_str(&self.as_str())?;
        formatter.write_str(")")
    }
}

impl std::fmt::Debug for ArtefactHash {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ArtefactHash(")?;
        formatter.write_str(&self.as_str())?;
        formatter.write_str(")")
    }
}

fn parse_digest(value: &str) -> Option<[u8; DIGEST_LEN]> {
    hex::decode(value.strip_prefix(PREFIX)?)
}

fn format_digest(bytes: &[u8; DIGEST_LEN]) -> String {
    let digest = hex::encode(bytes);
    let mut out = String::with_capacity(PREFIX.len() + digest.len());
    out.push_str(PREFIX);
    out.push_str(&digest);
    out
}

fn short_digest(bytes: &[u8; DIGEST_LEN]) -> String {
    hex::encode(bytes)[..8].to_owned()
}

#[cfg(test)]
mod tests;
