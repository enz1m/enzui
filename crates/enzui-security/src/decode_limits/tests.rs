use super::*;

// Start from unbounded limits so each test isolates one ceiling.
fn only(f: impl FnOnce(&mut DecodeLimits)) -> DecodeLimits {
    let mut l = DecodeLimits::UNLIMITED;
    f(&mut l);
    l
}

#[test]
fn max_pixel_product_fits_u64_so_no_pixel_overflow_verdict_is_needed() {
    let max = DecodeLimits::pixels(u32::MAX, u32::MAX);
    assert_eq!(max, 18_446_744_065_119_617_025);
    assert!(max < u64::MAX, "u32::MAX^2 must fit in u64");
    assert_eq!(u64::MAX - max, 8_589_934_590);
}

#[test]
fn rgba_product_can_overflow_u64_and_is_refused_not_wrapped() {
    let pixels = DecodeLimits::pixels(u32::MAX, u32::MAX);
    assert_eq!(
        DecodeLimits::rgba_bytes(pixels),
        None,
        "the RGBA product must be reported as overflow, not wrapped"
    );
    let wrapped = pixels.wrapping_mul(RGBA_BYTES_PER_PIXEL);
    assert_eq!(wrapped, 18_446_744_039_349_813_252);
    assert!(
        wrapped < pixels,
        "an unchecked RGBA multiply wraps BELOW the pixel count, so it would be allowed"
    );

    let limits = DecodeLimits::UNLIMITED;
    assert_eq!(
        limits.check_decoded(u32::MAX, u32::MAX),
        DecodeVerdict::RgbaOverflow { pixels },
        "an RGBA size that cannot be represented must be refused"
    );
}

#[test]
fn encoded_limit_allows_under_and_at_refuses_over() {
    let limits = only(|l| l.max_encoded_bytes = 1_000);

    assert_eq!(limits.check_encoded(999), DecodeVerdict::Allow);
    assert_eq!(
        limits.check_encoded(1_000),
        DecodeVerdict::Allow,
        "the limit is INCLUSIVE: a payload exactly at max_encoded_bytes is allowed"
    );
    assert_eq!(
        limits.check_encoded(1_001),
        DecodeVerdict::EncodedTooLarge {
            bytes: 1_001,
            limit: 1_000,
        },
        "one byte over the limit must be refused, naming the limit"
    );
}
