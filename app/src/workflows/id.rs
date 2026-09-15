use rand::rand_core::TryRng;
use rand::rngs::SysRng;

use crate::hex;

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct HexId([u8; 16]);

impl HexId {
    fn generate() -> Result<Self, IdError> {
        let mut bytes = [0u8; 16];
        SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| IdError::RandomUnavailable)?;
        Ok(Self(bytes))
    }

    fn parse(value: &str) -> Option<Self> {
        hex::decode(value).map(Self)
    }

    fn as_hex(&self) -> String {
        hex::encode(&self.0)
    }
}

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub(crate) struct $name(HexId);

        impl $name {
            pub(crate) fn parse(value: &str) -> Option<Self> {
                HexId::parse(value).map(Self)
            }

            pub(crate) fn as_hex(&self) -> String {
                self.0.as_hex()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.as_hex())
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(stringify!($name))?;
                formatter.write_str("(")?;
                formatter.write_str(&self.as_hex())?;
                formatter.write_str(")")
            }
        }
    };
}

opaque_id!(WorkflowId);
opaque_id!(RunId);
opaque_id!(AttemptId);
opaque_id!(ArtefactId);
opaque_id!(GateId);

impl WorkflowId {
    pub(crate) fn generate() -> Result<Self, IdError> {
        Ok(Self(HexId::generate()?))
    }
}

impl RunId {
    pub(crate) fn generate() -> Result<Self, IdError> {
        Ok(Self(HexId::generate()?))
    }
}

impl AttemptId {
    pub(crate) fn generate() -> Result<Self, IdError> {
        Ok(Self(HexId::generate()?))
    }
}

impl ArtefactId {
    pub(crate) fn generate() -> Result<Self, IdError> {
        Ok(Self(HexId::generate()?))
    }
}

impl GateId {
    pub(crate) fn generate() -> Result<Self, IdError> {
        Ok(Self(HexId::generate()?))
    }
}

#[derive(Debug)]
pub(crate) enum IdError {
    RandomUnavailable,
}

impl std::fmt::Display for IdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("system random source unavailable")
    }
}

impl std::error::Error for IdError {}

#[cfg(test)]
mod tests;
