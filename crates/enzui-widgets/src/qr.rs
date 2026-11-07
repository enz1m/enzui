//! `enz_qr` \u2014 a themed QR code widget.
//!
//! Wraps iced's `qr_code::QRCode` with:
//!   * a testable module matrix ([`EnzQrData::matrix`], [`EnzQrData::is_dark`])
//!     that is not exposed by iced's `Data` (its `contents`/`width` are private),
//!     so a round-trip test can assert on the produced grid rather than only
//!     "it rendered something";
//!   * a contrast-safe style that pins near-black modules on a near-white
//!     quiet-zone plate in both themes, drawn from theme tokens
//!     ([`enzui_theme::Palette::light`]'s `text` and `surface`) rather than
//!     hardcoded RGB. A theme-tinted QR code is unscannable, and the bounds /
//!     overlap audits cannot see that;
//!   * an accessible node ([`Role::Image`]) whose description carries the
//!     payload so AT can announce what the code contains.
//!
//! # Dark mode \u2014 pinned dark-on-light
//!
//! Both themes render the code near-black on near-white.
//! A dark-theme foreground on a dark background (or vice-versa) is not just
//! ugly \u2014 it fails scanner acquisition entirely, and neither the bounds nor
//! the overlap audit can see the failure. The colours come from the
//! light preset palette so they still track a token, not a literal.
//!
//! # Payload ceiling
//!
//! QR versions cap out (\u2248 2,953 alphanumeric bytes at ECC Low). Rather than
//! panic or silently render a wrong code, [`EnzQrData::try_new`] returns an
//! [`EnzQrError`] and the caller decides how to degrade. An adopter typically
//! renders a short fallback line ("Payload too long for a QR code") in place
//! of the code.

use enzui_core::semantics::{Role, Semantics};
use iced::widget::qr_code;
use iced::{Color, Element, Length};

/// Encoded QR payload plus the module matrix it represents.
///
/// Two parallel forms live inside:
///   * `iced_data` \u2014 iced's opaque [`qr_code::Data`], consumed by the render;
///   * `matrix` + `width` \u2014 the public view of the dark/light modules, so a
///     test can round-trip and a caller can inspect geometry without reaching
///     into vendored iced (its `Data` fields are private).
///
/// Both are built from the same `qrcode::QrCode`, so they cannot disagree.
pub struct EnzQrData {
    /// Payload as encoded (for the accessible description \u2014 not decoded from
    /// the matrix; that would require a scanner, which is out of scope).
    payload: String,
    /// iced's opaque encoded form (what actually paints).
    iced_data: qr_code::Data,
    /// Module grid, row-major: `matrix[row * width + column]`. `true` = dark.
    /// This is the same grid iced draws, exposed for testability. Excludes
    /// the quiet zone (iced adds its own 2-module quiet zone around this).
    matrix: Vec<bool>,
    /// Side length of the matrix in modules (not including the quiet zone).
    width: usize,
}

impl std::fmt::Debug for EnzQrData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnzQrData")
            .field("payload_len", &self.payload.len())
            .field("width", &self.width)
            .finish_non_exhaustive()
    }
}

/// An error building an [`EnzQrData`] from a payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnzQrError {
    /// The payload will not fit in any QR version at the widget's chosen error
    /// correction level.
    PayloadTooLong,
    /// The payload contained a character outside the encodable set.
    InvalidCharacter,
    /// Any other encoding failure from the underlying `qrcode` crate.
    Other(&'static str),
}

impl std::fmt::Display for EnzQrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnzQrError::PayloadTooLong => f.write_str("payload is too long to encode as a QR code"),
            EnzQrError::InvalidCharacter => {
                f.write_str("payload contains a character that cannot be encoded")
            }
            EnzQrError::Other(msg) => write!(f, "QR encoding failed: {msg}"),
        }
    }
}

impl std::error::Error for EnzQrError {}

impl From<qrcode::types::QrError> for EnzQrError {
    fn from(e: qrcode::types::QrError) -> Self {
        use qrcode::types::QrError;
        match e {
            QrError::DataTooLong => EnzQrError::PayloadTooLong,
            QrError::InvalidCharacter => EnzQrError::InvalidCharacter,
            QrError::UnsupportedCharacterSet => EnzQrError::InvalidCharacter,
            QrError::InvalidVersion => EnzQrError::Other("invalid version"),
            QrError::InvalidEciDesignator => EnzQrError::Other("invalid ECI"),
        }
    }
}

// iced's `qr_code::Error` covers the same set of underlying `qrcode` errors.
// Map through the same discriminant names.
impl From<qr_code::Error> for EnzQrError {
    fn from(e: qr_code::Error) -> Self {
        match e {
            qr_code::Error::DataTooLong => EnzQrError::PayloadTooLong,
            qr_code::Error::InvalidCharacter => EnzQrError::InvalidCharacter,
            qr_code::Error::UnsupportedCharacterSet => EnzQrError::InvalidCharacter,
            qr_code::Error::InvalidVersion => EnzQrError::Other("invalid version"),
            qr_code::Error::InvalidEciDesignator => EnzQrError::Other("invalid ECI"),
        }
    }
}

impl EnzQrData {
    /// Encode `payload` with the default (medium) error-correction level and
    /// the smallest version that fits.
    ///
    /// Returns [`EnzQrError::PayloadTooLong`] when the payload exceeds the
    /// largest QR version at ECC M. The caller decides how to degrade rather
    /// than getting a panic or a silently wrong code.
    ///
    /// # Errors
    ///
    /// [`EnzQrError::PayloadTooLong`] when the payload does not fit the largest version.
    pub fn try_new(payload: impl Into<String>) -> Result<Self, EnzQrError> {
        let payload = payload.into();

        // Encode twice from the same input: once for OUR matrix (accessible for
        // tests), once for iced (opaque `Data`). Both call the same encoder in
        // vendored iced's `qrcode` crate, so producing them together binds the
        // matrix we assert on to the matrix the widget will paint. If iced's
        // `qr_code::Data::new` diverged from `qrcode::QrCode::new` we would
        // need a shared source of truth, but they are the same call in
        // `iced/widget/src/qr_code.rs`.
        let code = qrcode::QrCode::new(payload.as_bytes())?;
        let width = code.width();
        let matrix: Vec<bool> = code
            .to_colors()
            .into_iter()
            .map(|c| c == qrcode::Color::Dark)
            .collect();

        let iced_data = qr_code::Data::new(payload.as_bytes())?;

        Ok(Self {
            payload,
            iced_data,
            matrix,
            width,
        })
    }

    /// The payload this code encodes.
    pub fn payload(&self) -> &str {
        &self.payload
    }

    /// Side length of the module grid, excluding the 2-module quiet zone.
    pub fn width(&self) -> usize {
        self.width
    }

    /// The module matrix, row-major (`true` = dark). Same grid iced paints,
    /// exposed for tests / callers that need to sample it.
    pub fn matrix(&self) -> &[bool] {
        &self.matrix
    }

    /// Is the module at `(row, column)` dark? Returns `false` for coordinates
    /// outside the grid (the quiet zone).
    pub fn is_dark(&self, row: usize, column: usize) -> bool {
        if row >= self.width || column >= self.width {
            return false;
        }
        self.matrix[row * self.width + column]
    }

    /// Access to the wrapped iced `qr_code::Data`, for callers that want to
    /// build the iced widget directly \u2014 the recommended path is [`enz_qr`],
    /// which pairs the render with the accessible semantics.
    pub fn iced_data(&self) -> &qr_code::Data {
        &self.iced_data
    }
}

// ---------------------------------------------------------------------------
// The widget builder
// ---------------------------------------------------------------------------

/// A themed QR code widget wrapping iced's `qr_code::QRCode`.
///
/// Use [`enz_qr`] to construct.
#[must_use]
pub struct EnzQr<'a, Message> {
    data: &'a EnzQrData,
    /// Total side length in px, including the quiet zone. `None` = fall back to
    /// `cell_size` * (matrix + quiet zone).
    total_size: Option<f32>,
    /// Explicit per-module cell size in px. Ignored when `total_size` is set.
    cell_size: Option<f32>,
    _phantom: std::marker::PhantomData<&'a Message>,
}

/// Construct an [`EnzQr`] widget from encoded [`EnzQrData`].
pub fn enz_qr<Message>(data: &EnzQrData) -> EnzQr<'_, Message> {
    EnzQr {
        data,
        total_size: None,
        cell_size: None,
        _phantom: std::marker::PhantomData,
    }
}

impl<'a, Message: 'a> EnzQr<'a, Message> {
    /// Set the total side length of the rendered code in px, including the
    /// 2-module quiet zone iced adds around the matrix. Non-finite or
    /// non-positive input is ignored.
    pub fn total_size(mut self, px: f32) -> Self {
        if px.is_finite() && px > 0.0 {
            self.total_size = Some(px);
        }
        self
    }

    /// Set the per-module cell size in px. Ignored when `total_size` is set.
    /// Non-finite or non-positive input is ignored.
    pub fn cell_size(mut self, px: f32) -> Self {
        if px.is_finite() && px > 0.0 {
            self.cell_size = Some(px);
        }
        self
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        let payload_for_a11y = self.data.payload.clone();

        let mut widget = qr_code(&self.data.iced_data)
            // pin dark-on-light in both themes. A theme-tinted
            // QR code is unscannable and neither the bounds nor overlap audit
            // can see that. Colours still come from theme tokens \u2014 the light
            // preset \u2014 rather than `Color::from_rgb(0.0, 0.0, 0.0)` literals,
            // so a theme that retunes the light preset carries the QR with it.
            .style(|_theme: &iced::Theme| qr_code::Style {
                cell: qr_dark_cell(),
                background: qr_light_plate(),
            });

        if let Some(total) = self.total_size {
            widget = widget.total_size(total);
        } else if let Some(cell) = self.cell_size {
            widget = widget.cell_size(cell);
        }

        // Publish the payload as the accessible description on a Role::Image
        // leaf. The visual is a barcode with no readable text child; a
        // screen-reader hearing "image" with no context would announce nothing
        // useful. This is the terminal/leaf shape (see
        // `SemanticsContainer::leaf` docs): the wrapped canvas has no
        // publishable AT children, so `leaf` is correct here.
        let semantics = Semantics::new(Role::Image)
            .label(enzui_core::localization::text("enzui-qr-label"))
            .description(payload_for_a11y);

        let inner: Element<'a, Message> = iced::widget::container(widget)
            .width(Length::Shrink)
            .height(Length::Shrink)
            .into();
        crate::semantics_container::SemanticsContainer::leaf(inner, semantics).into()
    }
}

impl<'a, Message: 'a> From<EnzQr<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzQr<'a, Message>) -> Self {
        w.into_element()
    }
}

/// The near-black module colour for both themes.
///
/// Drawn from the light preset's `text` token (`#1D1D1F`) rather than a
/// literal, so a theme that retunes light text (a warm/cool retune, an
/// accessibility contrast preset) carries the QR modules with it \u2014 as long as
/// the light preset's text stays a dark colour, which is a light-preset
/// invariant.
pub fn qr_dark_cell() -> Color {
    enzui_theme::Palette::light().text
}

/// The near-white quiet-zone / background plate for both themes.
///
/// From the light preset's `surface` (`#FFFFFF`).
pub fn qr_light_plate() -> Color {
    enzui_theme::Palette::light().surface
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
