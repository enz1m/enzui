//! Converts clipboard MIME payloads, errors and optional PNG images.
//!
//! Conversion performs no transport I/O. Untrusted PNG size and dimensions are
//! checked against [`DecodeLimits`] before allocating pixels.

use std::sync::Arc;

use enzui_clipboard::Kind as Wanted;
use enzui_clipboard::TransferLimits;
use enzui_clipboard::error::ClipboardError;
use enzui_clipboard::mime::MimeType;
use enzui_clipboard::payload::{Pasted, Payload, plain_text_from_html};
use enzui_core::decode_limits::DecodeLimits;
#[cfg(feature = "image")]
use enzui_core::decode_limits::{DecodeVerdict, RefusedVerdict};
use iced_core::clipboard::{Content, Error, Kind};

// The encoded ceiling is the transport's own, so a payload the transport
// already accepted is never refused for its size alone.
pub(crate) fn paste_decode_limits(transfer: &TransferLimits) -> DecodeLimits {
    DecodeLimits {
        max_encoded_bytes: u64::try_from(transfer.max_bytes).unwrap_or(u64::MAX),
        ..DecodeLimits::DEFAULT
    }
}

pub(crate) fn preferences_for(kind: Kind) -> Vec<MimeType> {
    let wanted = match kind {
        Kind::Text => Some(Wanted::Text),
        Kind::Html => Some(Wanted::Html),
        Kind::Files => Some(Wanted::Uris),
        #[cfg(feature = "image")]
        Kind::Image => Some(Wanted::Image),
        _ => None,
    };

    wanted
        .map(enzui_clipboard::Kind::preferences)
        .unwrap_or_default()
}

// The requested kind determines the output variant, regardless of other
// interpretations of the payload bytes.
pub(crate) fn map_pasted(
    pasted: Pasted,
    kind: Kind,
    limits: &DecodeLimits,
) -> Result<Content, Error> {
    #[cfg(not(feature = "image"))]
    let _ = limits;

    match kind {
        Kind::Text => pasted
            .text()
            .map(|text| Content::Text(text.to_owned()))
            .ok_or(Error::ConversionFailure),
        Kind::Html => pasted
            .text()
            .map(|html| Content::Html(html.to_owned()))
            .ok_or(Error::ConversionFailure),
        Kind::Files => match pasted.file_transfer() {
            Some(ft) => Ok(Content::Files(ft.into_paths())),
            None => Err(Error::ContentNotAvailable),
        },
        #[cfg(feature = "image")]
        Kind::Image => decode_image(pasted.bytes(), limits),
        _ => Err(Error::ContentNotAvailable),
    }
}

pub(crate) fn map_content(content: Content) -> Result<Payload, Error> {
    match content {
        Content::Text(text) => Ok(Payload::text(text)),
        Content::Html(html) => {
            let plain = plain_text_from_html(&html);
            Ok(Payload::html(html, plain))
        }
        Content::Files(paths) => Ok(Payload::file_transfer(
            paths,
            enzui_clipboard::FileIntent::Copy,
        )),
        #[cfg(feature = "image")]
        Content::Image(image) => Ok(Payload::image("image/png", encode_image(&image)?)),
        _ => Err(Error::ConversionFailure),
    }
}

pub(crate) fn translate(error: &ClipboardError) -> Error {
    match error {
        ClipboardError::NoSelection => Error::ContentNotAvailable,
        ClipboardError::MimeUnavailable { .. } => Error::ContentNotAvailable,
        ClipboardError::Unavailable { .. }
        | ClipboardError::StartupStalled { .. }
        | ClipboardError::NoDataDeviceManager
        | ClipboardError::NoPrimarySelection
        | ClipboardError::NoSeat
        | ClipboardError::UnknownSeat { .. }
        | ClipboardError::Disconnected
        | ClipboardError::Stopped => Error::ClipboardUnavailable,
        ClipboardError::ClaimNotHonoured { .. } => Error::ClipboardOccupied,
        ClipboardError::NotFocused => Error::Unknown {
            description: Arc::new(ClipboardError::NotFocused.to_string()),
        },
        other => Error::Unknown {
            description: Arc::new(other.to_string()),
        },
    }
}

// A budget refusal reaches the program as `Error::Unknown` naming the failing
// limit; malformed or unsupported bytes stay `Error::ConversionFailure`.
#[cfg(feature = "image")]
fn decode_image(encoded: &[u8], limits: &DecodeLimits) -> Result<Content, Error> {
    use image::{ImageDecoder, ImageFormat, ImageReader, Limits};
    use std::io::{BufReader, Cursor};

    let encoded_bytes = u64::try_from(encoded.len()).unwrap_or(u64::MAX);
    admit(limits.check_encoded(encoded_bytes))?;

    let mut reader =
        ImageReader::with_format(BufReader::new(Cursor::new(encoded)), ImageFormat::Png);
    // The declared size must reach this budget rather than the image crate's
    // generic limit error; `into_decoder` allocates no pixel buffer.
    reader.no_limits();
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| Error::ConversionFailure)?;

    let (width, height) = decoder.dimensions();
    admit(limits.check(encoded_bytes, width, height))?;

    let mut decoder_limits = Limits::no_limits();
    decoder_limits.max_image_width = Some(limits.max_width);
    decoder_limits.max_image_height = Some(limits.max_height);
    decoder_limits.max_alloc = Some(limits.max_rgba_bytes);
    decoder
        .set_limits(decoder_limits)
        .map_err(|_| Error::ConversionFailure)?;

    let decoded = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| Error::ConversionFailure)?
        .into_rgba8();

    admit_rgba(decoded, limits)
}

// A decoder that yields more bytes than its header's layout is refused here
// rather than published.
#[cfg(feature = "image")]
fn admit_rgba(decoded: image::RgbaImage, limits: &DecodeLimits) -> Result<Content, Error> {
    use iced_core::Size;
    use iced_core::clipboard::Image;

    let (width, height) = decoded.dimensions();
    admit(limits.check_decoded(width, height))?;

    let pixels = decoded.into_raw();
    let rgba_bytes = u64::try_from(pixels.len()).unwrap_or(u64::MAX);
    let layout_pixels = DecodeLimits::pixels(width, height);
    let layout_bytes = DecodeLimits::rgba_bytes(layout_pixels)
        .ok_or_else(|| refusal(RefusedVerdict::rgba_overflow(layout_pixels)))?;
    let admitted_bytes = layout_bytes.min(limits.max_rgba_bytes);
    if rgba_bytes > admitted_bytes {
        return Err(refusal(RefusedVerdict::rgba_too_large(
            rgba_bytes,
            admitted_bytes,
        )));
    }

    Ok(Content::Image(Image {
        rgba: iced_core::Bytes::from(pixels),
        size: Size::new(width, height),
    }))
}

#[cfg(feature = "image")]
fn admit(verdict: DecodeVerdict) -> Result<(), Error> {
    match RefusedVerdict::new(verdict) {
        None => Ok(()),
        Some(refused) => Err(refusal(refused)),
    }
}

#[cfg(feature = "image")]
fn refusal(refused: RefusedVerdict) -> Error {
    Error::Unknown {
        description: Arc::new(format!("clipboard image refused: {}", refused.reason())),
    }
}

#[cfg(feature = "image")]
fn encode_image(image: &iced_core::clipboard::Image) -> Result<Vec<u8>, Error> {
    let buffer: image::RgbaImage =
        image::ImageBuffer::from_raw(image.size.width, image.size.height, image.rgba.to_vec())
            .ok_or(Error::ConversionFailure)?;

    let mut encoded = Vec::new();
    buffer
        .write_to(
            &mut std::io::Cursor::new(&mut encoded),
            image::ImageFormat::Png,
        )
        .map_err(|_| Error::ConversionFailure)?;

    Ok(encoded)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn budget() -> DecodeLimits {
        paste_decode_limits(&TransferLimits::default())
    }

    #[test]
    fn html_paste_returns_content_html() {
        let pasted = Pasted::new(
            MimeType::new("text/html;charset=utf-8"),
            b"<b>bold</b>".to_vec(),
        );
        let content = map_pasted(pasted, Kind::Html, &budget()).expect("html maps");
        assert_eq!(content, Content::Html(String::from("<b>bold</b>")));
    }

    #[cfg(feature = "image")]
    #[test]
    fn image_content_still_round_trips_through_png() {
        let image = iced_core::clipboard::Image {
            rgba: iced_core::Bytes::from(vec![0x11, 0x22, 0x33, 0x44]),
            size: iced_core::Size::new(1, 1),
        };
        let payload = map_content(Content::Image(image.clone())).expect("image encodes");
        let mime = MimeType::new("image/png");
        let encoded = payload.bytes_for(&mime).expect("PNG representation exists");
        let pasted = Pasted::new(mime, encoded.to_vec());

        assert_eq!(
            map_pasted(pasted, Kind::Image, &budget()),
            Ok(Content::Image(image))
        );
    }
}
