use rand::rand_core::TryRng;
use rand::rngs::SysRng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct RequestId([u8; 16]);

impl RequestId {
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

impl std::fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

impl std::fmt::Debug for RequestId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RequestId(")?;
        formatter.write_str(&self.as_hex())?;
        formatter.write_str(")")
    }
}

impl Serialize for RequestId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_hex())
    }
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| serde::de::Error::custom("invalid request identity"))
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
