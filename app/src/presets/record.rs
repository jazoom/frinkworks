use rand::{rand_core::TryRng, rngs::SysRng};
use serde::{Deserialize, Serialize};

use crate::{conversations::ConversationId, execution::ExecutionSettings};

pub(crate) const PRESET_RECORD_VERSION: u32 = 1;
pub(crate) const MAXIMUM_PRESETS: usize = 128;
pub(crate) const MAXIMUM_PRESET_NAME_BYTES: usize = 80;

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct PresetId([u8; 16]);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PresetProvenance {
    Conversation { id: ConversationId, title: String },
    Draft,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PresetRecord {
    pub(crate) id: PresetId,
    pub(crate) revision: u32,
    pub(crate) name: String,
    pub(crate) settings: ExecutionSettings,
    pub(crate) provenance: PresetProvenance,
    pub(crate) created_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PresetError {
    Random,
    Persist,
    Corrupt,
    Full,
    Missing,
    Stale,
    Name,
    Preview,
    PreviewFull,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(super) struct PresetFile {
    pub(super) record_version: u32,
    pub(super) id: String,
    pub(super) revision: u32,
    pub(super) name: String,
    pub(super) settings: crate::execution::ExecutionSettingsFile,
    pub(super) provenance: PresetProvenanceFile,
    pub(super) created_at_ms: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(super) enum PresetProvenanceFile {
    Conversation { id: String, title: String },
    Draft,
}

impl PresetId {
    pub(crate) fn generate() -> Result<Self, PresetError> {
        let mut bytes = [0_u8; 16];
        SysRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| PresetError::Random)?;
        Ok(Self(bytes))
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        crate::hex::decode(value).map(Self)
    }

    pub(crate) fn as_hex(self) -> String {
        crate::hex::encode(&self.0)
    }
}

impl std::fmt::Display for PresetId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

impl std::fmt::Debug for PresetId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "PresetId({self})")
    }
}

impl PresetError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Random => "Frinkworks cannot create a preset identifier. Try again.",
            Self::Persist => "Frinkworks cannot store the preset. Try again.",
            Self::Corrupt => "The preset store is unreadable.",
            Self::Full => "Delete a preset before you save another one.",
            Self::Missing => "That preset is no longer available.",
            Self::Stale => "That preset changed. Reload it before you save or delete it.",
            Self::Name => "Enter a preset name of 1 to 80 bytes without control characters.",
            Self::Preview => "That preset preview is stale or already used. Preview it again.",
            Self::PreviewFull => "Too many preset previews are active. Try again after 30 minutes.",
        }
    }
}

impl PresetRecord {
    pub(super) fn to_file(&self) -> PresetFile {
        PresetFile {
            record_version: PRESET_RECORD_VERSION,
            id: self.id.as_hex(),
            revision: self.revision,
            name: self.name.clone(),
            settings: self.settings.to_file(),
            provenance: match &self.provenance {
                PresetProvenance::Conversation { id, title } => {
                    PresetProvenanceFile::Conversation {
                        id: id.as_hex(),
                        title: title.clone(),
                    }
                }
                PresetProvenance::Draft => PresetProvenanceFile::Draft,
            },
            created_at_ms: self.created_at_ms,
        }
    }

    pub(super) fn from_file(file: PresetFile) -> Result<Self, PresetError> {
        if file.record_version != PRESET_RECORD_VERSION || file.revision == 0 {
            return Err(PresetError::Corrupt);
        }
        let name = normalise_name(&file.name)?;
        let provenance = match file.provenance {
            PresetProvenanceFile::Conversation { id, title } => PresetProvenance::Conversation {
                id: ConversationId::parse(&id).ok_or(PresetError::Corrupt)?,
                title,
            },
            PresetProvenanceFile::Draft => PresetProvenance::Draft,
        };
        Ok(Self {
            id: PresetId::parse(&file.id).ok_or(PresetError::Corrupt)?,
            revision: file.revision,
            name,
            settings: ExecutionSettings::from_file(file.settings).ok_or(PresetError::Corrupt)?,
            provenance,
            created_at_ms: file.created_at_ms,
        })
    }
}

pub(super) fn normalise_name(raw: &str) -> Result<String, PresetError> {
    let name = raw.trim();
    if name.is_empty()
        || name.len() > MAXIMUM_PRESET_NAME_BYTES
        || name.chars().any(char::is_control)
    {
        return Err(PresetError::Name);
    }
    Ok(name.to_owned())
}

pub(crate) fn suggested_name(settings: &ExecutionSettings) -> String {
    settings
        .directories
        .first()
        .and_then(|grant| grant.host_path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Untitled preset".to_owned())
}
