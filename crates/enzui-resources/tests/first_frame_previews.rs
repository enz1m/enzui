#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_core::decode_limits::{DecodeLimits, DecodeVerdict};
use enzui_resources::{DecodeError, first_frame::*};
use image::{Frame, Rgba, RgbaImage, codecs::gif::GifEncoder};

fn gif() -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        encoder
            .encode_frame(Frame::new(RgbaImage::from_pixel(
                4,
                2,
                Rgba([255, 0, 0, 255]),
            )))
            .unwrap();
        encoder
            .encode_frame(Frame::new(RgbaImage::from_pixel(
                4,
                2,
                Rgba([0, 0, 255, 255]),
            )))
            .unwrap();
    }
    bytes
}

fn apng(separate_default: bool) -> Vec<u8> {
    apng_metadata(separate_default, None, 0)
}

fn apng_metadata(separate_default: bool, orientation: Option<u16>, text_bytes: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 4, 2);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_animated(2, 0).unwrap();
        encoder.set_sep_def_img(separate_default).unwrap();
        let mut writer = encoder.write_header().unwrap();
        if let Some(orientation) = orientation {
            let mut exif = vec![
                b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0, 0x12, 1, 3, 0, 1, 0, 0, 0,
            ];
            exif.extend_from_slice(&orientation.to_le_bytes());
            exif.extend_from_slice(&[0; 6]);
            writer.write_chunk(png::chunk::eXIf, &exif).unwrap();
        }
        if text_bytes != 0 {
            let mut text = b"Comment\0".to_vec();
            text.resize(text.len() + text_bytes, b'x');
            writer.write_chunk(png::chunk::tEXt, &text).unwrap();
        }
        if separate_default {
            writer
                .write_image_data(&[0, 255, 0, 255].repeat(8))
                .unwrap();
        }
        writer
            .write_image_data(&[255, 0, 0, 255].repeat(8))
            .unwrap();
        writer
            .write_image_data(&[0, 0, 255, 255].repeat(8))
            .unwrap();
    }
    bytes
}

#[test]
fn first_frame_admission_precedes_decoding_and_cancellation_discards_pixels() {
    let limits = DecodeLimits {
        max_encoded_bytes: 1,
        ..DecodeLimits::DEFAULT
    };
    assert!(matches!(
        decode(b"GIF89a", &limits, || false),
        Err(DecodeError::Limit(_))
    ));
    for bytes in [gif(), apng(false)] {
        let limits = DecodeLimits {
            max_width: 3,
            ..DecodeLimits::DEFAULT
        };
        assert!(
            matches!(decode_resized(&bytes, &limits, 1, 1, || false), Err(DecodeError::Limit(verdict)) if matches!(verdict.verdict(), DecodeVerdict::WidthTooLarge { .. }))
        );
        let limits = DecodeLimits {
            max_encoded_bytes: bytes.len() as u64 - 1,
            ..DecodeLimits::DEFAULT
        };
        assert!(matches!(
            decode(&bytes, &limits, || false),
            Err(DecodeError::Limit(_))
        ));
        assert_eq!(
            decode(&bytes, &DecodeLimits::DEFAULT, || true).unwrap_err(),
            DecodeError::Cancelled
        );
        let checks = std::sync::atomic::AtomicUsize::new(0);
        assert_eq!(
            decode(&bytes, &DecodeLimits::DEFAULT, || checks
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                >= 2)
            .unwrap_err(),
            DecodeError::Cancelled
        );
    }
}
