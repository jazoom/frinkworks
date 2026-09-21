use rand::rand_core::TryRng;
use rand::rngs::SysRng;

use crate::hex;

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct ConversationId([u8; 16]);

impl ConversationId {
    pub(crate) fn generate() -> Result<Self, ConversationIdError> {
        let mut bytes = [0_u8; 16];
        SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| ConversationIdError::RandomUnavailable)?;
        Ok(Self(bytes))
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        hex::decode(value).map(Self)
    }

    pub(crate) fn as_hex(&self) -> String {
        hex::encode(&self.0)
    }
}

impl std::fmt::Display for ConversationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

impl std::fmt::Debug for ConversationId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ConversationId(")?;
        formatter.write_str(&self.as_hex())?;
        formatter.write_str(")")
    }
}

#[derive(Debug)]
pub(crate) enum ConversationIdError {
    RandomUnavailable,
}

// A message keeps the identity that the transcript and model history both reference.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct MessageId([u8; 16]);

impl MessageId {
    pub(crate) fn generate() -> Result<Self, ConversationIdError> {
        let mut bytes = [0_u8; 16];
        SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| ConversationIdError::RandomUnavailable)?;
        Ok(Self(bytes))
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        hex::decode(value).map(Self)
    }

    pub(crate) fn as_hex(&self) -> String {
        hex::encode(&self.0)
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

impl std::fmt::Debug for MessageId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MessageId(")?;
        formatter.write_str(&self.as_hex())?;
        formatter.write_str(")")
    }
}

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct CheckpointId([u8; 16]);

impl CheckpointId {
    pub(crate) fn generate() -> Result<Self, ConversationIdError> {
        let mut bytes = [0_u8; 16];
        SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| ConversationIdError::RandomUnavailable)?;
        Ok(Self(bytes))
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        hex::decode(value).map(Self)
    }

    pub(crate) fn as_hex(&self) -> String {
        hex::encode(&self.0)
    }
}

impl std::fmt::Display for CheckpointId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

impl std::fmt::Debug for CheckpointId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CheckpointId(")?;
        formatter.write_str(&self.as_hex())?;
        formatter.write_str(")")
    }
}

impl std::fmt::Display for ConversationIdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("system random source unavailable")
    }
}

impl std::error::Error for ConversationIdError {}

#[cfg(test)]
mod tests;
