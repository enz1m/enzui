//! Runtime-loaded libheif decoder for HEIC / HEVC-branded HEIF still images.
//!
//! This is the optional native-decoder route, with one
//! deliberate difference from using `libheif-rs` directly: `libheif-rs` finds
//! libheif through `pkg-config` and links it into the executable. That makes an
//! enabled build fail to start when the shared library disappears, so it cannot
//! uphold this crate's `Unsupported` degradation contract. This module loads the
//! small C API surface it uses at the point of decode instead. A missing library,
//! missing HEVC decoder plugin, or feature-off build is therefore a typed
//! [`DecodeError::Unsupported`]; malformed content is
//! [`DecodeError::DecodeFailed`].
//!
//! The tradeoff is explicit. libheif itself is LGPL, but HEIC normally carries
//! HEVC, whose patent-pool/licensing situation is why distributions often make
//! the codec optional. It is also native C/C++ decoder attack surface reached
//! from untrusted bytes. Consequently the Cargo feature is off by default.
//!
//! The freedesktop thumbnailer route was rejected here because it would execute
//! arbitrary registered helper binaries. Doing that safely requires a real
//! sandbox (one read-only input bind, no network, deadline and output cap), not
//! a subprocess call hidden in an image decoder. Declining all support was also
//! rejected: runtime loading gives hosts that install libheif useful support
//! while preserving honest absence everywhere else.

#![allow(unsafe_code, reason = "libheif is loaded and called through FFI")]

use super::{DecodeError, StaticFormat};
use enzui_core::decode_limits::{DecodeLimits, RefusedVerdict};
use image::{DynamicImage, RgbaImage};
use libloading::Library;
use std::ffi::{c_char, c_int, c_void};
use std::ptr;

#[repr(C)]
#[derive(Clone, Copy)]
struct HeifError {
    code: c_int,
    subcode: c_int,
    message: *const c_char,
}

type ContextAlloc = unsafe extern "C" fn() -> *mut c_void;
type ContextFree = unsafe extern "C" fn(*mut c_void);
type Init = unsafe extern "C" fn(*mut c_void) -> HeifError;
type Deinit = unsafe extern "C" fn();
type ReadMemory =
    unsafe extern "C" fn(*mut c_void, *const c_void, usize, *const c_void) -> HeifError;
type PrimaryHandle = unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> HeifError;
type HandleRelease = unsafe extern "C" fn(*const c_void);
type HandleDimension = unsafe extern "C" fn(*const c_void) -> c_int;
type DecodeImage =
    unsafe extern "C" fn(*const c_void, *mut *mut c_void, c_int, c_int, *const c_void) -> HeifError;
type ImageRelease = unsafe extern "C" fn(*const c_void);
type ImagePlane = unsafe extern "C" fn(*const c_void, c_int, *mut c_int) -> *const u8;
type ImageDimension = unsafe extern "C" fn(*const c_void, c_int) -> c_int;

const HEIF_OK: c_int = 0;
const HEIF_UNSUPPORTED_FEATURE: c_int = 4;
const HEIF_PLUGIN_LOADING_ERROR: c_int = 11;
const HEIF_NO_MATCHING_DECODER: c_int = 6003;
const HEIF_COLORSPACE_RGB: c_int = 1;
const HEIF_CHROMA_INTERLEAVED_RGBA: c_int = 11;
const HEIF_CHANNEL_INTERLEAVED: c_int = 10;

struct Api {
    _library: Library,
    init: Init,
    deinit: Deinit,
    context_alloc: ContextAlloc,
    context_free: ContextFree,
    read_memory: ReadMemory,
    primary_handle: PrimaryHandle,
    handle_release: HandleRelease,
    handle_width: HandleDimension,
    handle_height: HandleDimension,
    decode_image: DecodeImage,
    image_release: ImageRelease,
    image_plane: ImagePlane,
    image_width: ImageDimension,
    image_height: ImageDimension,
}

impl Api {
    fn load() -> Result<Self, DecodeError> {
        Self::load_from(&["libheif.so.1", "libheif.so", "libheif.dylib", "libheif.dll"])
    }

    fn load_from(names: &[&str]) -> Result<Self, DecodeError> {
        let mut library = None;
        for name in names {
            // SAFETY: loading a library runs its initializers. This is precisely
            // the opt-in native-code boundary documented by the `heif` feature.
            // No symbol is called until every expected ABI entry is resolved.
            if let Ok(candidate) = unsafe { Library::new(name) } {
                library = Some(candidate);
                break;
            }
        }
        let library = library.ok_or(DecodeError::Unsupported(StaticFormat::Heif))?;

        macro_rules! symbol {
            ($name:literal, $ty:ty) => {{
                // SAFETY: these signatures are the stable libheif C API. The
                // library remains owned by `Api` for longer than every copied
                // function pointer and all objects obtained through it.
                unsafe {
                    *library
                        .get::<$ty>(concat!($name, "\0").as_bytes())
                        .map_err(|_| DecodeError::Unsupported(StaticFormat::Heif))?
                }
            }};
        }

        let init = symbol!("heif_init", Init);
        let deinit = symbol!("heif_deinit", Deinit);
        let context_alloc = symbol!("heif_context_alloc", ContextAlloc);
        let context_free = symbol!("heif_context_free", ContextFree);
        let read_memory = symbol!("heif_context_read_from_memory_without_copy", ReadMemory);
        let primary_handle = symbol!("heif_context_get_primary_image_handle", PrimaryHandle);
        let handle_release = symbol!("heif_image_handle_release", HandleRelease);
        let handle_width = symbol!("heif_image_handle_get_width", HandleDimension);
        let handle_height = symbol!("heif_image_handle_get_height", HandleDimension);
        let decode_image = symbol!("heif_decode_image", DecodeImage);
        let image_release = symbol!("heif_image_release", ImageRelease);
        let image_plane = symbol!("heif_image_get_plane_readonly", ImagePlane);
        let image_width = symbol!("heif_image_get_width", ImageDimension);
        let image_height = symbol!("heif_image_get_height", ImageDimension);

        Ok(Self {
            _library: library,
            init,
            deinit,
            context_alloc,
            context_free,
            read_memory,
            primary_handle,
            handle_release,
            handle_width,
            handle_height,
            decode_image,
            image_release,
            image_plane,
            image_width,
            image_height,
        })
    }
}

struct OwnedNative {
    ptr: *mut c_void,
    release: unsafe extern "C" fn(*const c_void),
}

impl Drop for OwnedNative {
    fn drop(&mut self) {
        // SAFETY: each non-null pointer is returned by libheif and paired with
        // its documented release function exactly once through this guard.
        unsafe { (self.release)(self.ptr) };
    }
}

struct OwnedContext {
    ptr: *mut c_void,
    release: ContextFree,
}

impl Drop for OwnedContext {
    fn drop(&mut self) {
        // SAFETY: the pointer came from `heif_context_alloc` and this guard is
        // its sole owner.
        unsafe { (self.release)(self.ptr) };
    }
}

struct Initialized(Deinit);

impl Drop for Initialized {
    fn drop(&mut self) {
        // SAFETY: paired with the one successful `heif_init` call below and
        // dropped before the owning `Library`.
        unsafe { (self.0)() };
    }
}

fn classify(error: HeifError) -> Result<(), DecodeError> {
    if error.code == HEIF_OK {
        Ok(())
    } else if error.code == HEIF_UNSUPPORTED_FEATURE
        || error.code == HEIF_PLUGIN_LOADING_ERROR
        || error.subcode == HEIF_NO_MATCHING_DECODER
    {
        Err(DecodeError::Unsupported(StaticFormat::Heif))
    } else {
        Err(DecodeError::DecodeFailed(StaticFormat::Heif))
    }
}

fn plane_copy_plan(
    output_width: u32,
    output_height: u32,
    plane_width: c_int,
    plane_height: c_int,
    stride: c_int,
) -> Result<(usize, usize), DecodeError> {
    let plane_width =
        u32::try_from(plane_width).map_err(|_| DecodeError::DecodeFailed(StaticFormat::Heif))?;
    let plane_height =
        u32::try_from(plane_height).map_err(|_| DecodeError::DecodeFailed(StaticFormat::Heif))?;
    if plane_width < output_width || plane_height < output_height {
        return Err(DecodeError::DecodeFailed(StaticFormat::Heif));
    }

    let row_bytes = usize::try_from(u64::from(output_width) * 4)
        .map_err(|_| DecodeError::DecodeFailed(StaticFormat::Heif))?;
    let stride =
        usize::try_from(stride).map_err(|_| DecodeError::DecodeFailed(StaticFormat::Heif))?;
    if stride < row_bytes {
        return Err(DecodeError::DecodeFailed(StaticFormat::Heif));
    }
    let rows = usize::try_from(output_height)
        .map_err(|_| DecodeError::DecodeFailed(StaticFormat::Heif))?;
    Ok((row_bytes, rows))
}

pub(super) fn decode(
    bytes: &[u8],
    limits: &DecodeLimits,
    cancelled: &impl Fn() -> bool,
) -> Result<DynamicImage, DecodeError> {
    with_primary(bytes, limits, cancelled, |api, handle, width, height| {
        let mut image_ptr = ptr::null_mut();
        // SAFETY: all pointers are live; RGB/RGBA/interleaved are stable C enum
        // values, and null requests default decoding options.
        classify(unsafe {
            (api.decode_image)(
                handle.ptr,
                &mut image_ptr,
                HEIF_COLORSPACE_RGB,
                HEIF_CHROMA_INTERLEAVED_RGBA,
                ptr::null(),
            )
        })?;
        if image_ptr.is_null() {
            return Err(DecodeError::DecodeFailed(StaticFormat::Heif));
        }
        let image = OwnedNative {
            ptr: image_ptr,
            release: api.image_release,
        };
        if cancelled() {
            return Err(DecodeError::Cancelled);
        }

        let rgba_bytes =
            DecodeLimits::rgba_bytes(DecodeLimits::pixels(width, height)).ok_or_else(|| {
                DecodeError::Limit(RefusedVerdict::rgba_overflow(DecodeLimits::pixels(
                    width, height,
                )))
            })?;
        let buffer_len = usize::try_from(rgba_bytes)
            .map_err(|_| DecodeError::DecodeFailed(StaticFormat::Heif))?;

        let mut stride = 0;
        // SAFETY: `image` is live and the dimension queries are read-only.
        let plane_width = unsafe { (api.image_width)(image.ptr, HEIF_CHANNEL_INTERLEAVED) };
        // SAFETY: as above.
        let plane_height = unsafe { (api.image_height)(image.ptr, HEIF_CHANNEL_INTERLEAVED) };
        // SAFETY: `image` is live and `stride` is writable storage for the
        // plane query.
        let plane = unsafe { (api.image_plane)(image.ptr, HEIF_CHANNEL_INTERLEAVED, &mut stride) };
        if plane.is_null() {
            return Err(DecodeError::DecodeFailed(StaticFormat::Heif));
        }
        let (row_bytes, rows) = plane_copy_plan(width, height, plane_width, plane_height, stride)?;
        let stride =
            usize::try_from(stride).map_err(|_| DecodeError::DecodeFailed(StaticFormat::Heif))?;

        let mut pixels = vec![0; buffer_len];
        for row in 0..rows {
            let source_offset = row
                .checked_mul(stride)
                .ok_or(DecodeError::DecodeFailed(StaticFormat::Heif))?;
            let destination_offset = row
                .checked_mul(row_bytes)
                .ok_or(DecodeError::DecodeFailed(StaticFormat::Heif))?;
            // SAFETY: libheif guarantees the queried plane has `plane_height`
            // rows of `stride` bytes. `plane_copy_plan` proved that extent covers
            // every copied output row and that the visible prefix fits the stride;
            // destination ranges are bounded by the prechecked RGBA size.
            let source = unsafe { std::slice::from_raw_parts(plane.add(source_offset), row_bytes) };
            pixels[destination_offset..destination_offset + row_bytes].copy_from_slice(source);
        }

        let rgba = RgbaImage::from_raw(width, height, pixels)
            .ok_or(DecodeError::DecodeFailed(StaticFormat::Heif))?;
        Ok(DynamicImage::ImageRgba8(rgba))
    })
}

#[cfg(feature = "local-images")]
pub(super) fn probe(
    bytes: &[u8],
    limits: &DecodeLimits,
    cancelled: &impl Fn() -> bool,
) -> Result<(u32, u32), DecodeError> {
    with_primary(bytes, limits, cancelled, |_, _, width, height| {
        Ok((width, height))
    })
}

#[cfg(feature = "local-images")]
pub(super) fn available() -> bool {
    let Ok(api) = Api::load() else {
        return false;
    };
    // SAFETY: the optional library owns the resolved C function for this call.
    let Ok(have_decoder) = (unsafe {
        api._library
            .get::<unsafe extern "C" fn(c_int) -> c_int>(b"heif_have_decoder_for_format\0")
    }) else {
        return false;
    };
    // SAFETY: null selects default initialization; the guard pairs successful initialization.
    if classify(unsafe { (api.init)(ptr::null_mut()) }).is_err() {
        return false;
    }
    let _initialized = Initialized(api.deinit);
    // SAFETY: HEVC is compression enum value 1 in the libheif C API.
    unsafe { have_decoder(1) != 0 }
}

fn with_primary<T>(
    bytes: &[u8],
    limits: &DecodeLimits,
    cancelled: &impl Fn() -> bool,
    use_handle: impl FnOnce(&Api, &OwnedNative, u32, u32) -> Result<T, DecodeError>,
) -> Result<T, DecodeError> {
    let api = Api::load()?;

    // SAFETY: null requests libheif's default initialization parameters.
    classify(unsafe { (api.init)(ptr::null_mut()) })?;
    let _initialized = Initialized(api.deinit);

    // SAFETY: no arguments; ownership is immediately captured by the guard.
    let context_ptr = unsafe { (api.context_alloc)() };
    if context_ptr.is_null() {
        return Err(DecodeError::DecodeFailed(StaticFormat::Heif));
    }
    let context = OwnedContext {
        ptr: context_ptr,
        release: api.context_free,
    };

    // SAFETY: `bytes` outlives the context and remains immutable. The final
    // null is the API-required default reading options pointer.
    classify(unsafe {
        (api.read_memory)(context.ptr, bytes.as_ptr().cast(), bytes.len(), ptr::null())
    })?;

    let mut handle_ptr = ptr::null_mut();
    // SAFETY: context is live and out-pointer is valid for one handle.
    classify(unsafe { (api.primary_handle)(context.ptr, &mut handle_ptr) })?;
    if handle_ptr.is_null() {
        return Err(DecodeError::DecodeFailed(StaticFormat::Heif));
    }
    let handle = OwnedNative {
        ptr: handle_ptr,
        release: api.handle_release,
    };

    // Header dimensions are queried and screened before `heif_decode_image`,
    // which is the call that allocates the decoded native pixel plane.
    // SAFETY: `handle` is live and the query is read-only.
    let width = unsafe { (api.handle_width)(handle.ptr) };
    // SAFETY: as above.
    let height = unsafe { (api.handle_height)(handle.ptr) };
    let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
        return Err(DecodeError::DecodeFailed(StaticFormat::Heif));
    };
    let verdict = limits.check_decoded(width, height);
    if let Some(refused) = RefusedVerdict::new(verdict) {
        return Err(DecodeError::Limit(refused));
    }
    if cancelled() {
        return Err(DecodeError::Cancelled);
    }

    use_handle(&api, &handle, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_plane_must_cover_every_output_row_before_copying() {
        assert!(plane_copy_plan(64, 64, 64, 32, 256).is_err());
        assert!(plane_copy_plan(64, 64, 32, 64, 256).is_err());
        assert_eq!(plane_copy_plan(64, 64, 64, 64, 256).unwrap(), (256, 64));
        assert!(plane_copy_plan(64, 64, 64, 64, 255).is_err());
        assert!(plane_copy_plan(64, 64, -1, 64, 256).is_err());
    }
}
