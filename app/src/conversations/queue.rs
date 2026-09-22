use rand::rand_core::TryRng;
use rand::rngs::SysRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::execution::ExecutionSettings;
use crate::hex;
use crate::sessions::JobId;

use super::id::ConversationIdError;

#[cfg(test)]
mod tests;

pub(crate) const MAXIMUM_QUEUE_ITEMS: usize = 16;
pub(crate) const MAXIMUM_QUEUE_ITEM_BYTES: usize = super::MAXIMUM_MESSAGE_BYTES;
pub(crate) const MAXIMUM_QUEUE_BYTES: usize = 128 * 1024;

#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct QueueItemId([u8; 16]);

impl QueueItemId {
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

impl std::fmt::Display for QueueItemId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

impl std::fmt::Debug for QueueItemId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("QueueItemId(")?;
        formatter.write_str(&self.as_hex())?;
        formatter.write_str(")")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum QueueDelivery {
    FollowUp,
    Steering,
}

impl QueueDelivery {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "follow-up" => Some(Self::FollowUp),
            "steering" => Some(Self::Steering),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::FollowUp => "follow-up",
            Self::Steering => "steering",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct QueueItem {
    pub(crate) id: QueueItemId,
    pub(crate) text: String,
    /// Frozen command provenance. The expanded text stays in `text`.
    pub(crate) input: Option<super::InputProvenance>,
    /// Immutable image references that the delivery moves to its message.
    pub(crate) attachments: Vec<super::AttachmentRef>,
    pub(crate) delivery: QueueDelivery,
    pub(crate) settings_digest: [u8; 32],
    pub(crate) job: Option<JobId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConversationQueue {
    pub(crate) revision: u32,
    pub(crate) items: Vec<QueueItem>,
}

impl Default for ConversationQueue {
    fn default() -> Self {
        Self {
            revision: 1,
            items: Vec::new(),
        }
    }
}

impl ConversationQueue {
    pub(crate) fn next_follow_up(&self) -> Option<&QueueItem> {
        self.items
            .iter()
            .find(|item| item.delivery == QueueDelivery::FollowUp)
    }

    pub(crate) fn next_steering(&self, job: JobId) -> Option<&QueueItem> {
        self.items
            .iter()
            .find(|item| item.delivery == QueueDelivery::Steering && item.job == Some(job))
    }
}

pub(crate) fn launch_digest(settings: &ExecutionSettings) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(crate::execution::settings_digest(settings));
    for field in [
        settings.model.provider.as_str(),
        settings.model.model.as_str(),
        settings
            .model
            .thinking
            .as_ref()
            .map_or("", |thinking| thinking.as_str()),
        settings.instructions.as_str(),
    ] {
        digest.update((field.len() as u64).to_le_bytes());
        digest.update(field.as_bytes());
    }
    digest.finalize().into()
}

pub(crate) fn valid_queue(queue: &ConversationQueue) -> bool {
    if queue.revision == 0 || queue.items.len() > MAXIMUM_QUEUE_ITEMS {
        return false;
    }
    let mut total = 0usize;
    let mut seen = Vec::new();
    for item in &queue.items {
        let empty_with_images = item.text.trim().is_empty() && !item.attachments.is_empty();
        if seen.contains(&item.id)
            || (!empty_with_images && item.text.is_empty())
            || !super::history::valid_attachments(&item.attachments)
            || !item
                .input
                .as_ref()
                .is_none_or(|input| input.valid() && input.expanded == item.text)
            || item.text.len() > MAXIMUM_QUEUE_ITEM_BYTES
            || item
                .text
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
        {
            return false;
        }
        total = total.saturating_add(item.text.len());
        if total > MAXIMUM_QUEUE_BYTES {
            return false;
        }
        seen.push(item.id);
    }
    true
}
