#![allow(
    clippy::cast_possible_truncation,
    reason = "test fixtures write known small lengths"
)]

//! Hostile-payload and static decode tests.

use super::*;
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};

fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(rgba, width, height, ExtendedColorType::Rgba8)
        .expect("encode PNG fixture");
    out
}

#[test]
fn apng_control_chunk_is_rejected_before_decode() {
    let base = png(1, 1, &[0, 0, 0, 255]);
    let ihdr_end = 8 + 12 + 13;
    let mut animated = Vec::new();
    animated.extend_from_slice(&base[..ihdr_end]);
    animated.extend_from_slice(&chunk(*b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]));
    animated.extend_from_slice(&base[ihdr_end..]);
    assert_eq!(
        decode_static(&animated, &DecodeLimits::DEFAULT, || false).expect_err("APNG excluded"),
        DecodeError::Animated
    );
}

fn chunk(kind: [u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(&kind);
    out.extend_from_slice(data);
    let mut crc_input = kind.to_vec();
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    out
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg() & 0xedb8_8320;
            crc = (crc >> 1) ^ mask;
        }
    }
    !crc
}

fn tagged_png(rgba: &[u8], profile: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = PngEncoder::new(&mut out);
    encoder
        .set_icc_profile(profile)
        .expect("PNG takes a profile");
    encoder
        .write_image(rgba, 1, 1, ExtendedColorType::Rgba8)
        .expect("encode PNG fixture");
    out
}

fn first_pixel(asset: &DecodedAsset) -> [u8; 4] {
    let Handle::Rgba { pixels, .. } = &asset.handle else {
        panic!("decoded to RGBA");
    };
    [pixels[0], pixels[1], pixels[2], pixels[3]]
}

#[test]
fn an_embedded_wide_gamut_profile_is_converted_to_srgb() {
    let colour = [200, 100, 50, 255];
    let p3 = moxcms::ColorProfile::new_display_p3()
        .encode()
        .expect("encode P3");
    let srgb = moxcms::ColorProfile::new_srgb()
        .encode()
        .expect("encode sRGB");
    let decode = |bytes: Vec<u8>| {
        first_pixel(&decode_static(&bytes, &DecodeLimits::DEFAULT, || false).expect("decodes"))
    };

    let converted = decode(tagged_png(&colour, p3));
    assert!(
        converted[0] > colour[0] && converted[2] < colour[2],
        "a Display P3 orange is more saturated in sRGB: {converted:?}"
    );
    assert_eq!(converted[3], 255, "alpha is untouched");
    assert_eq!(
        decode(tagged_png(&colour, srgb)),
        colour,
        "an sRGB profile changes nothing"
    );
    assert_eq!(
        decode(tagged_png(&colour, b"not a profile".to_vec())),
        colour,
        "an unreadable profile leaves the pixels as decoded"
    );
    assert_eq!(decode(png(1, 1, &colour)), colour, "untagged stays as is");
}
