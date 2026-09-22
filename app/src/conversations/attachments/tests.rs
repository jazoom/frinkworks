use std::io::Cursor;

use super::*;

impl AttachmentStore {
    pub(crate) fn in_memory() -> Self {
        let directory = tempfile::tempdir().expect("temporary attachments");
        Self::open(directory.keep()).expect("attachment store")
    }
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .expect("encode png");
    bytes
}

#[test]
fn normalise_accepts_a_png_and_reports_decoded_dimensions() {
    let store = AttachmentStore::in_memory();
    let normalised = store.normalise(&png(8, 4)).expect("png");
    assert_eq!(normalised.format, AttachmentFormat::Png);
    assert_eq!((normalised.width, normalised.height), (8, 4));
    assert!(!normalised.bytes.is_empty());
}

#[test]
fn normalise_rejects_svg_and_gif() {
    let store = AttachmentStore::in_memory();
    assert_eq!(
        store.normalise(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"),
        Err(AttachmentError::Unsupported)
    );
    let gif = b"GIF89a\x01\x00\x01\x00\x00\x00\x00;";
    assert_eq!(
        store.normalise(gif).expect_err("gif"),
        AttachmentError::Unsupported
    );
}

#[test]
fn normalise_rejects_an_animated_png_container() {
    let store = AttachmentStore::in_memory();
    let mut bytes = png(4, 4);
    bytes.splice(
        33..33,
        [0, 0, 0, 8].into_iter().chain(*b"acTL").chain([0; 12]),
    );
    assert_eq!(
        store.normalise(&bytes).expect_err("animated"),
        AttachmentError::Animated
    );
}

#[test]
fn normalise_rejects_oversized_input_before_decoding() {
    let store = AttachmentStore::in_memory();
    let bytes = vec![0_u8; MAXIMUM_ATTACHMENT_BYTES + 1];
    assert_eq!(
        store.normalise(&bytes).expect_err("large"),
        AttachmentError::TooLarge
    );
}

#[test]
fn normalise_rejects_an_excessive_edge_even_with_few_pixels() {
    let store = AttachmentStore::in_memory();
    assert_eq!(
        store.normalise(&png(MAXIMUM_DECODED_EDGE + 1, 1)),
        Err(AttachmentError::Dimensions)
    );
}

#[test]
fn object_round_trip_verifies_the_digest() {
    let store = AttachmentStore::in_memory();
    let normalised = store.normalise(&png(4, 4)).expect("png");
    let sha256 = normalised.sha256();
    store
        .write_object(&sha256, &normalised.bytes)
        .expect("write");
    let reference = AttachmentRef {
        id: AttachmentId::generate().expect("id"),
        sha256: sha256.clone(),
        format: normalised.format,
        width: normalised.width,
        height: normalised.height,
        byte_length: normalised.bytes.len() as u64,
    };
    assert_eq!(store.load(&reference).expect("load"), normalised.bytes);
    let mut forged = reference.clone();
    forged.sha256 = "0".repeat(64);
    assert_eq!(store.load(&forged), Err(AttachmentError::Missing));
}

#[test]
fn animation_marker_in_a_chunk_body_is_not_animation() {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend_from_slice(&[0, 0, 0, 4]);
    bytes.extend_from_slice(b"tEXtacTL");
    bytes.extend_from_slice(&[0; 4]);
    assert!(!animated(&bytes, image::ImageFormat::Png));
}
