//! Feature-gated PDF page rendering through a system `libpdfium`.
//!
//! Inputs are admitted before parsing, raster dimensions are limit-checked before native
//! allocation, and missing or incompatible runtime libraries produce
//! [`PreviewError::Unsupported`].

use std::{
    path::Path,
    sync::{Mutex, MutexGuard, OnceLock, PoisonError},
};

use pdfium_render::prelude::{
    PdfDocument, PdfPageIndex, PdfPageRenderRotation, PdfRenderConfig, Pdfium, PdfiumError,
};

use crate::decode_limits::{DecodeLimits, RefusedVerdict};

use super::{
    DEFAULT_PDF_PREVIEW_WIDTH, PreviewError, RasterTarget, RenderedPreview, plan_raster_target,
};

/// The identifier after the dynamic loader's `undefined symbol: `.
fn missing_symbol(text: &str) -> Option<String> {
    let (_, rest) = text.rsplit_once("undefined symbol: ")?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        .then_some(name)
}

/// Classifies a Pdfium binding failure as unavailable-library or missing-symbol support.
pub fn classify_bind_error(e: &PdfiumError) -> PreviewError {
    if let PdfiumError::LoadLibraryError(lib_err) = e {
        // `libloading` exposes DlOpen versus DlSym only through this error’s Debug form.
        let debug = format!("{lib_err:?}");
        if debug.starts_with("DlSym") || debug.starts_with("DlSymUnknown") {
            // Native loader messages name the symbol after `undefined symbol: `.
            // Depending on the libloading version that text is in the
            // Display form, the source chain or only the Debug form.
            let mut texts = vec![format!("{lib_err}"), debug];
            let mut source = std::error::Error::source(lib_err);
            while let Some(cause) = source {
                texts.push(cause.to_string());
                source = cause.source();
            }
            let found = texts.iter().find_map(|text| missing_symbol(text));
            let symbol = found.as_deref().unwrap_or("<unknown>");
            return PreviewError::Unsupported {
                what: format!(
                    "libpdfium was found but is missing `{symbol}` \
                     — it is an incompatible or reduced build \
                     (LibreOffice's `libpdfiumlo.so` is one); \
                     install a full libpdfium"
                ),
            };
        }
        return PreviewError::Unsupported {
            what: format!(
                "libpdfium is not available on this host: {lib_err} \
                 (install `libpdfium.so` or set LD_LIBRARY_PATH \
                 to a directory containing it)"
            ),
        };
    }
    if matches!(e, PdfiumError::PdfiumLibraryBindingsAlreadyInitialized) {
        // The library loaded successfully once; a later bind attempt is a caller reuse
        // defect, so it must not be reported as a missing or broken installation.
        return PreviewError::Unsupported {
            what: "libpdfium is already initialized in this process; \
                   the initialized binding must be reused instead of rebound"
                .into(),
        };
    }
    PreviewError::Unsupported {
        what: format!(
            "libpdfium is not available on this host: {e} \
             (install `libpdfium.so` or set LD_LIBRARY_PATH \
             to a directory containing it)"
        ),
    }
}

static PDFIUM: OnceLock<Result<Pdfium, PreviewError>> = OnceLock::new();

// pdfium-render 0.9.3 offers no per-call FFI synchronisation, so concurrent FPDF calls are
// undefined behaviour even behind its `thread_safe` feature. Poisoning is ignored because
// the guarded state is call ordering, not Rust data a panicking render could invalidate.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

fn render_lock() -> MutexGuard<'static, ()> {
    RENDER_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn bind() -> Result<&'static Pdfium, PreviewError> {
    PDFIUM
        // `Pdfium::default()` also searches the current directory; initialization must stay
        // restricted to the platform's system-library lookup policy.
        .get_or_init(|| {
            Pdfium::bind_to_system_library()
                .map(Pdfium::new)
                .map_err(|error| classify_bind_error(&error))
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn positive_f32_parts(value: f32) -> Option<(u64, i16)> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let bits = value.to_bits();
    let exponent_bits = i16::try_from((bits >> 23) & 0xff).ok()?;
    let fraction = u64::from(bits & 0x7f_ffff);
    if exponent_bits == 0 {
        Some((fraction, -149))
    } else {
        Some(((1_u64 << 23) | fraction, exponent_bits - 127 - 23))
    }
}

// Convert positive finite page dimensions to an exact integer ratio, refusing shifts that
// exceed `u64`.
fn exact_f32_aspect_ratio(width: f32, height: f32) -> Result<(u64, u64), PreviewError> {
    let (mut numerator, width_exponent) =
        positive_f32_parts(width).ok_or_else(|| PreviewError::DecodeFailed {
            message: format!("PDF page width must be positive and finite, got {width}"),
        })?;
    let (mut denominator, height_exponent) =
        positive_f32_parts(height).ok_or_else(|| PreviewError::DecodeFailed {
            message: format!("PDF page height must be positive and finite, got {height}"),
        })?;

    let common = gcd(numerator, denominator);
    numerator /= common;
    denominator /= common;
    let exponent_delta = width_exponent - height_exponent;
    if exponent_delta >= 0 {
        numerator = numerator
            .checked_shl(u32::try_from(exponent_delta).unwrap_or(u32::MAX))
            .ok_or_else(|| PreviewError::DecodeFailed {
                message: "PDF page aspect ratio overflows u64".into(),
            })?;
    } else {
        denominator = denominator
            .checked_shl(u32::try_from(-exponent_delta).unwrap_or(u32::MAX))
            .ok_or_else(|| PreviewError::DecodeFailed {
                message: "PDF page aspect ratio overflows u64".into(),
            })?;
    }
    let common = gcd(numerator, denominator);
    Ok((numerator / common, denominator / common))
}

// Invoke the rasterizer only after the planned positive dimensions satisfy all decode limits.
fn invoke_planned_rasterizer<T>(
    requested_width: u32,
    aspect_width: u64,
    aspect_height: u64,
    limits: &DecodeLimits,
    rasterize: impl FnOnce(RasterTarget) -> Result<T, PreviewError>,
) -> Result<T, PreviewError> {
    let target = plan_raster_target(requested_width, aspect_width, aspect_height, limits)?;
    rasterize(target)
}

/// Renders one PDF page to limit-checked RGBA8 pixels, using the default preview width as an
/// upper target.
///
/// The system `libpdfium` binding is initialized once per process and then reused. That
/// initialization is sticky in both directions: a failure is cached, so every later call
/// returns the same [`PreviewError::Unsupported`] without retrying the loader, and the
/// library is never searched for again. Renders are serialized process-wide, so concurrent
/// callers queue instead of rasterizing in parallel.
///
/// # Errors
///
/// Returns [`PreviewError`] for library binding, input, page-index, decoding, or limit
/// failures.
pub fn render_pdf_page(
    path: &Path,
    page: u32,
    limits: &DecodeLimits,
) -> Result<RenderedPreview, PreviewError> {
    render_pdf_page_at_width(path, page, DEFAULT_PDF_PREVIEW_WIDTH, limits)
}

/// Renders one PDF page at a requested width, reducing it as needed to satisfy decode limits.
///
/// This shares the once-initialized process binding, its sticky failure, and the process-wide
/// render serialization described on [`render_pdf_page`].
///
/// # Errors
///
/// Returns the same [`PreviewError`] categories as [`render_pdf_page`].
pub fn render_pdf_page_at_width(
    path: &Path,
    page: u32,
    requested_width: u32,
    limits: &DecodeLimits,
) -> Result<RenderedPreview, PreviewError> {
    // Pass Pdfium the descriptor whose encoded size was admitted, not a reopened pathname.
    let (checked, _) = super::open_checked_file(path, limits)?;

    // Held until the RGBA copy completes: binding retrieval, document load, page access and
    // rasterisation are all FPDF calls that must not interleave with another render.
    let _render_guard = render_lock();
    let pdfium = bind()?;

    // Map Pdfium document-open errors to DecodeFailed because file admission already handled I/O
    // access.
    let document: PdfDocument = pdfium
        .load_pdf_from_reader(checked.file, None)
        .map_err(|e| {
            let msg = e.to_string();
            PreviewError::DecodeFailed {
                message: format!("load {}: {msg}", path.display()),
            }
        })?;

    let pages = document.pages();
    let page_count: PdfPageIndex = pages.len();
    // Treat page indices above `i32::MAX` as out of range.
    let page_i32: PdfPageIndex = i32::try_from(page).unwrap_or(i32::MAX);
    let page_count_u32 = u32::try_from(page_count).unwrap_or(u32::MAX);
    if page_i32 >= page_count || page_count <= 0 {
        return Err(PreviewError::PageOutOfRange {
            requested: page,
            page_count: page_count_u32,
        });
    }
    let page_obj = pages
        .get(page_i32)
        .map_err(|e| PreviewError::DecodeFailed {
            message: format!("get page {page}: {e}"),
        })?;

    // Validate exact target dimensions before the callback can allocate a Pdfium bitmap.
    let (aspect_width, aspect_height) =
        exact_f32_aspect_ratio(page_obj.width().value, page_obj.height().value)?;
    let bitmap = invoke_planned_rasterizer(
        requested_width,
        aspect_width,
        aspect_height,
        limits,
        |target| {
            let target_width =
                i32::try_from(target.width()).map_err(|_| PreviewError::DecodeFailed {
                    message: format!("planned PDF width {} exceeds i32", target.width()),
                })?;
            let target_height =
                i32::try_from(target.height()).map_err(|_| PreviewError::DecodeFailed {
                    message: format!("planned PDF height {} exceeds i32", target.height()),
                })?;
            let render_config = PdfRenderConfig::new()
                .set_fixed_size(target_width, target_height)
                .rotate(PdfPageRenderRotation::None, false);
            page_obj
                .render_with_config(&render_config)
                .map_err(|e| PreviewError::DecodeFailed {
                    message: format!("render page {page}: {e}"),
                })
        },
    )?;

    let width_i32 = bitmap.width();
    let height_i32 = bitmap.height();
    // Reject non-positive dimensions returned across the Pdfium boundary.
    if width_i32 <= 0 || height_i32 <= 0 {
        return Err(PreviewError::DecodeFailed {
            message: format!(
                "pdfium returned a non-positive bitmap size: {width_i32}x{height_i32}"
            ),
        });
    }
    let width = u32::try_from(width_i32).map_err(|_| PreviewError::DecodeFailed {
        message: format!("pdfium bitmap width does not fit u32: {width_i32}"),
    })?;
    let height = u32::try_from(height_i32).map_err(|_| PreviewError::DecodeFailed {
        message: format!("pdfium bitmap height does not fit u32: {height_i32}"),
    })?;
    let verdict = limits.check_decoded(width, height);
    if let Some(refused) = RefusedVerdict::new(verdict) {
        return Err(PreviewError::LimitExceeded { verdict: refused });
    }

    let rgba = bitmap.as_rgba_bytes();
    RenderedPreview::new(width, height, rgba)
}
