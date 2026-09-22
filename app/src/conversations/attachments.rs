//! Session-staged image attachments.
//!
//! The database owns attachment reference metadata and the filesystem owns the
//! normalised image bytes. A message stores only immutable references, so an
//! attachment survives compaction and can be shared by a fork. Bytes are
//! content-addressed by their SHA-256 digest, which lets several references
//! share one object without a mutable alias.
//!
//! Decoding happens once, before an attachment is staged. The stored object is
//! a re-encoded raster with no source metadata, so a later reader never trusts
//! a filename or a client-supplied MIME type.

use std::io::Cursor;
use std::path::PathBuf;

use rand::rand_core::TryRng;
use rand::rngs::SysRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::id::ConversationIdError;
use crate::hex;

/// Largest accepted upload. The check happens before any decode allocation.
pub(crate) const MAXIMUM_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;
/// Most images that one message can reference.
pub(crate) const MAXIMUM_ATTACHMENTS_PER_MESSAGE: usize = 8;
/// Aggregate normalised bytes for one message.
pub(crate) const MAXIMUM_MESSAGE_ATTACHMENT_BYTES: u64 = 24 * 1024 * 1024;
/// Strict decoded pixel ceiling. A larger image is rejected, never downscaled.
pub(crate) const MAXIMUM_DECODED_PIXELS: u64 = 40_000_000;
/// Strict decoded edge ceiling.
pub(crate) const MAXIMUM_DECODED_EDGE: u32 = 12_000;
/// Unclaimed staging rows older than this expire. Bytes are removed only when
/// no reference row remains.
pub(crate) const STAGING_TTL_MS: u64 = 60 * 60 * 1000;

const OBJECT_DIRECTORY: &str = "objects";

#[derive(Clone, Copy, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct AttachmentId([u8; 16]);

impl AttachmentId {
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

impl std::fmt::Display for AttachmentId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_hex())
    }
}

impl std::fmt::Debug for AttachmentId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AttachmentId(")?;
        formatter.write_str(&self.as_hex())?;
        formatter.write_str(")")
    }
}

/// The raster formats that Power Plant accepts and re-encodes. SVG and any
/// animated container are not in this set and never reach storage.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AttachmentFormat {
    Png,
    Jpeg,
    Webp,
}

impl AttachmentFormat {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Webp => "webp",
        }
    }

    pub(crate) fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }

    pub(crate) fn image_format(self) -> image::ImageFormat {
        match self {
            Self::Png => image::ImageFormat::Png,
            Self::Jpeg => image::ImageFormat::Jpeg,
            Self::Webp => image::ImageFormat::WebP,
        }
    }

    fn from_image_format(format: image::ImageFormat) -> Option<Self> {
        match format {
            image::ImageFormat::Png => Some(Self::Png),
            image::ImageFormat::Jpeg => Some(Self::Jpeg),
            image::ImageFormat::WebP => Some(Self::Webp),
            _ => None,
        }
    }
}

/// One immutable reference to a stored object. The message keeps this metadata
/// and never the bytes, so compaction and forks copy references without reading
/// or duplicating image data.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AttachmentRef {
    pub(crate) id: AttachmentId,
    pub(crate) sha256: String,
    pub(crate) format: AttachmentFormat,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) byte_length: u64,
}

impl AttachmentRef {
    pub(crate) fn valid(&self) -> bool {
        self.sha256.len() == 64
            && self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            && self.width > 0
            && self.height > 0
            && self.width <= MAXIMUM_DECODED_EDGE
            && self.height <= MAXIMUM_DECODED_EDGE
            && u64::from(self.width).saturating_mul(u64::from(self.height))
                <= MAXIMUM_DECODED_PIXELS
            && self.byte_length > 0
            && self.byte_length <= MAXIMUM_ATTACHMENT_BYTES as u64
    }
}

/// Estimated image input tokens from decoded dimensions. The encoded byte
/// length never substitutes for the decoded patch count.
pub(crate) fn estimated_image_tokens(width: u32, height: u32) -> u64 {
    let patches = (u64::from(width).div_ceil(512))
        .saturating_mul(u64::from(height).div_ceil(512))
        .max(1);
    patches.saturating_mul(170).saturating_add(85)
}

/// A decoded, metadata-free raster. Its bytes are ready to write to an object.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct NormalisedImage {
    pub(crate) format: AttachmentFormat,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) bytes: Vec<u8>,
}

impl NormalisedImage {
    pub(crate) fn sha256(&self) -> String {
        hex::encode(&Sha256::digest(&self.bytes))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AttachmentError {
    Missing,
    Empty,
    TooLarge,
    Unsupported,
    Animated,
    Malformed,
    Dimensions,
    Count,
    Aggregate,
    Persist,
    Foreign,
    Consumed,
}

impl AttachmentError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Missing => "That image is no longer staged. Add it again.",
            Self::Empty => "The selected image file is empty.",
            Self::TooLarge => "Each image must be 8 MiB or smaller.",
            Self::Unsupported => "Use a PNG, JPEG or WebP image.",
            Self::Animated => "Animated images are not supported.",
            Self::Malformed => "That image file is damaged or unreadable.",
            Self::Dimensions => "That image has dimensions that are too large.",
            Self::Count => "One message can hold at most eight images.",
            Self::Aggregate => "Those images together are too large for one message.",
            Self::Persist => "Power Plant could not store the image.",
            Self::Foreign | Self::Consumed => {
                "That image is not available for this conversation. Add it again."
            }
        }
    }
}

impl std::fmt::Display for AttachmentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for AttachmentError {}

/// Filesystem side of the attachment lifecycle. The database remains the
/// authority for ownership; this type only reads and writes object bytes.
pub(crate) struct AttachmentStore {
    root: PathBuf,
}

impl AttachmentStore {
    pub(crate) fn open(root: PathBuf) -> Result<Self, AttachmentError> {
        let objects = root.join(OBJECT_DIRECTORY);
        crate::storage::ensure_private_dir(&objects).map_err(|_| AttachmentError::Persist)?;
        Ok(Self { root: objects })
    }

    fn object_path(&self, sha256: &str) -> PathBuf {
        self.root.join(sha256)
    }

    /// Repeated writes preserve content identity at the digest path.
    pub(crate) fn write_object(&self, sha256: &str, bytes: &[u8]) -> Result<(), AttachmentError> {
        if bytes.is_empty()
            || bytes.len() > MAXIMUM_ATTACHMENT_BYTES
            || sha256 != hex::encode(&Sha256::digest(bytes))
        {
            return Err(AttachmentError::Malformed);
        }
        crate::storage::write_private(&self.object_path(sha256), bytes)
            .map_err(|_| AttachmentError::Persist)
    }

    pub(crate) fn load(&self, reference: &AttachmentRef) -> Result<Vec<u8>, AttachmentError> {
        if !reference.valid() {
            return Err(AttachmentError::Malformed);
        }
        let bytes = crate::storage::read_private_bounded(
            &self.object_path(&reference.sha256),
            MAXIMUM_ATTACHMENT_BYTES,
        )
        .map_err(|_| AttachmentError::Missing)?;
        if bytes.len() as u64 != reference.byte_length
            || hex::encode(&Sha256::digest(&bytes)) != reference.sha256
        {
            return Err(AttachmentError::Missing);
        }
        Ok(bytes)
    }

    pub(crate) fn remove_object(&self, sha256: &str) {
        if sha256.len() == 64 && sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            let _ = crate::storage::remove_private(&self.object_path(sha256));
        }
    }

    /// Decode, bound and re-encode one upload. The returned bytes carry no
    /// source metadata and use a format from the accepted set.
    pub(crate) fn normalise(&self, input: &[u8]) -> Result<NormalisedImage, AttachmentError> {
        if input.is_empty() {
            return Err(AttachmentError::Empty);
        }
        if input.len() > MAXIMUM_ATTACHMENT_BYTES {
            return Err(AttachmentError::TooLarge);
        }
        let guessed = image::guess_format(input).map_err(|_| AttachmentError::Unsupported)?;
        let format =
            AttachmentFormat::from_image_format(guessed).ok_or(AttachmentError::Unsupported)?;
        if animated(input, guessed) {
            return Err(AttachmentError::Animated);
        }
        let mut reader = image::ImageReader::new(Cursor::new(input))
            .with_guessed_format()
            .map_err(|_| AttachmentError::Malformed)?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAXIMUM_DECODED_EDGE);
        limits.max_image_height = Some(MAXIMUM_DECODED_EDGE);
        limits.max_alloc = Some(MAXIMUM_DECODED_PIXELS.saturating_mul(4));
        reader.limits(limits);
        let (width, height) = reader
            .into_dimensions()
            .map_err(|_| AttachmentError::Dimensions)?;
        if width == 0
            || height == 0
            || width > MAXIMUM_DECODED_EDGE
            || height > MAXIMUM_DECODED_EDGE
            || u64::from(width).saturating_mul(u64::from(height)) > MAXIMUM_DECODED_PIXELS
        {
            return Err(AttachmentError::Dimensions);
        }
        let mut reader = image::ImageReader::with_format(Cursor::new(input), guessed);
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(MAXIMUM_DECODED_EDGE);
        limits.max_image_height = Some(MAXIMUM_DECODED_EDGE);
        limits.max_alloc = Some(MAXIMUM_DECODED_PIXELS.saturating_mul(8));
        reader.limits(limits);
        let image = reader.decode().map_err(|_| AttachmentError::Malformed)?;
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), format.image_format())
            .map_err(|_| AttachmentError::Malformed)?;
        if bytes.is_empty() || bytes.len() > MAXIMUM_ATTACHMENT_BYTES {
            return Err(AttachmentError::TooLarge);
        }
        Ok(NormalisedImage {
            format,
            width,
            height,
            bytes,
        })
    }
}

/// PNG and WebP can carry animation. Reject an animated container instead of
/// silently storing only its first frame. JPEG has no animation.
fn animated(bytes: &[u8], format: image::ImageFormat) -> bool {
    match format {
        image::ImageFormat::Png => container_has_chunk(bytes, 8, true, &[b"acTL"]),
        image::ImageFormat::WebP => container_has_chunk(bytes, 12, false, &[b"ANIM", b"ANMF"]),
        _ => false,
    }
}

fn container_has_chunk(bytes: &[u8], mut offset: usize, png: bool, kinds: &[&[u8; 4]]) -> bool {
    while let Some(header) = bytes.get(offset..offset.saturating_add(8)) {
        let (length, kind) = if png {
            (
                u32::from_be_bytes(header[..4].try_into().expect("four bytes")),
                &header[4..],
            )
        } else {
            (
                u32::from_le_bytes(header[4..].try_into().expect("four bytes")),
                &header[..4],
            )
        };
        if kinds.iter().any(|expected| kind == *expected) {
            return true;
        }
        let overhead = if png { 12 } else { 8 + (length as usize % 2) };
        let Some(next) = offset
            .checked_add(length as usize)
            .and_then(|end| end.checked_add(overhead))
        else {
            return false;
        };
        offset = next;
    }
    false
}

/// Parse one `attachment_N` form value. An empty value means the field was
/// absent, which is not an error.
pub(crate) fn parse_attachment_id(value: &str) -> Option<AttachmentId> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    AttachmentId::parse(value)
}

pub(crate) fn prepare_turns(
    state: &crate::state::AppState,
    connection: &crate::providers::ProviderConnection,
    turns: &mut [crate::providers::ChatTurn],
) -> Result<(), &'static str> {
    if turns.iter().all(|turn| turn.images.is_empty()) {
        return Ok(());
    }
    if state
        .models_dev
        .supports_images(connection.kind, &connection.model)
        != Some(true)
    {
        return Err(
            "The selected model has no known image input support. Choose a model with image input.",
        );
    }
    for image in turns.iter_mut().flat_map(|turn| &mut turn.images) {
        image.bytes = state
            .conversations
            .attachment_store()
            .load(&image.reference)
            .map_err(|error| error.message())?;
        image.format = image.reference.format;
        image.width = image.reference.width;
        image.height = image.reference.height;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
