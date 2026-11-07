//! The libarchive read API, loaded at run time.
//!
//! Linking libarchive would make a host without it unable to start the app;
//! loading it with `dlopen` at first use keeps its absence an
//! [`ArchiveError::CodecUnavailable`](super::ArchiveError::CodecUnavailable).
//! Only explicit formats and filters are registered, never the `_all`
//! variants, and a filter whose registration answers `ARCHIVE_WARN` (it
//! would run an external program) counts as missing.

#![allow(unsafe_code, reason = "libarchive is loaded and called through FFI")]

use enzui_security::secret::{Secret, SecretText};
use libloading::Library;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr::NonNull;

const ARCHIVE_OK: c_int = 0;
const ARCHIVE_EOF: c_int = 1;
const ARCHIVE_WARN: c_int = -20;
const AE_IFMT: u32 = 0o170_000;
const AE_IFREG: u32 = 0o100_000;
const AE_IFDIR: u32 = 0o040_000;
const AE_IFLNK: u32 = 0o120_000;
const BLOCK: usize = 64 * 1024;

type Handle = *mut c_void;
type New = unsafe extern "C" fn() -> Handle;
type Support = unsafe extern "C" fn(Handle) -> c_int;
type Passphrase = unsafe extern "C" fn(Handle, *const c_char) -> c_int;
type OpenFilenames = unsafe extern "C" fn(Handle, *const *const c_char, usize) -> c_int;
type NextHeader = unsafe extern "C" fn(Handle, *mut Handle) -> c_int;
type DataBlock = unsafe extern "C" fn(Handle, *mut *const c_void, *mut usize, *mut i64) -> c_int;
type FilterBytes = unsafe extern "C" fn(Handle, c_int) -> i64;
type EntryString = unsafe extern "C" fn(Handle) -> *const c_char;
type EntryU32 = unsafe extern "C" fn(Handle) -> u32;
type EntryInt = unsafe extern "C" fn(Handle) -> c_int;
type EntryI64 = unsafe extern "C" fn(Handle) -> i64;
type Version = unsafe extern "C" fn() -> *const c_char;
type SetData = unsafe extern "C" fn(Handle, *mut c_void) -> c_int;
type ReadCallback = unsafe extern "C" fn(Handle, *mut c_void, *mut *const c_void) -> isize;
type SeekCallback = unsafe extern "C" fn(Handle, *mut c_void, i64, c_int) -> i64;
type SkipCallback = unsafe extern "C" fn(Handle, *mut c_void, i64) -> i64;
type SetRead = unsafe extern "C" fn(Handle, ReadCallback) -> c_int;
type SetSeek = unsafe extern "C" fn(Handle, SeekCallback) -> c_int;
type SetSkip = unsafe extern "C" fn(Handle, SkipCallback) -> c_int;

/// A reader format: what to register for one [`super::ArchiveFormat`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Format {
    Zip,
    Tar,
    SevenZip,
    /// RAR4 and RAR5.
    Rar,
    /// One compressed stream.
    Raw,
}

/// A decompression filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Filter {
    Gzip,
    Bzip2,
    Xz,
    Zstd,
}

/// The resolved libarchive entry points and what this build registers.
pub(super) struct Codecs {
    _library: Library,
    read_new: New,
    read_free: Support,
    format_zip: Support,
    format_tar: Support,
    format_7zip: Support,
    format_rar: Support,
    format_rar5: Support,
    format_raw: Support,
    filter_gzip: Support,
    filter_bzip2: Support,
    filter_xz: Support,
    filter_zstd: Support,
    add_passphrase: Passphrase,
    open_filenames: OpenFilenames,
    set_callback_data: SetData,
    set_read_callback: SetRead,
    set_seek_callback: SetSeek,
    set_skip_callback: SetSkip,
    open1: Support,
    next_header: NextHeader,
    data_block: DataBlock,
    filter_bytes: FilterBytes,
    error_string: EntryString,
    entry_pathname: EntryString,
    entry_pathname_utf8: EntryString,
    entry_hardlink: EntryString,
    entry_symlink: EntryString,
    entry_filetype: EntryU32,
    entry_perm: EntryU32,
    entry_is_encrypted: EntryInt,
    entry_size_is_set: EntryInt,
    entry_size: EntryI64,
    version: Version,
    formats: Vec<Format>,
    filters: Vec<Filter>,
    /// A `C.UTF-8` character-type locale, as an address: libarchive
    /// converts names to the thread's locale, and the process's default `C`
    /// locale cannot hold a UTF-8 ZIP name. Never freed.
    utf8_locale: usize,
}

/// The process's libarchive, loaded on first use.
pub(super) fn codecs() -> Result<&'static Codecs, String> {
    static LOADED: std::sync::OnceLock<Result<Codecs, String>> = std::sync::OnceLock::new();
    LOADED
        .get_or_init(Codecs::load)
        .as_ref()
        .map_err(Clone::clone)
}

impl Codecs {
    fn load() -> Result<Self, String> {
        // SAFETY: loading libarchive runs only its ELF initialisers, which
        // have no preconditions.
        let library = ["libarchive.so.13", "libarchive.so"]
            .into_iter()
            .find_map(|name| unsafe { Library::new(name) }.ok())
            .ok_or_else(|| "libarchive is not installed".to_owned())?;
        macro_rules! symbol {
            ($name:literal) => {
                // SAFETY: each symbol is read with the signature libarchive
                // 3 documents for it in archive.h / archive_entry.h, and the
                // copied function pointer never outlives `_library`, which
                // `Codecs` owns for the life of the process.
                *unsafe { library.get(concat!($name, "\0").as_bytes()) }
                    .map_err(|_| format!("libarchive lacks {}", $name))?
            };
        }
        let mut codecs = Self {
            read_new: symbol!("archive_read_new"),
            read_free: symbol!("archive_read_free"),
            format_zip: symbol!("archive_read_support_format_zip"),
            format_tar: symbol!("archive_read_support_format_tar"),
            format_7zip: symbol!("archive_read_support_format_7zip"),
            format_rar: symbol!("archive_read_support_format_rar"),
            format_rar5: symbol!("archive_read_support_format_rar5"),
            format_raw: symbol!("archive_read_support_format_raw"),
            filter_gzip: symbol!("archive_read_support_filter_gzip"),
            filter_bzip2: symbol!("archive_read_support_filter_bzip2"),
            filter_xz: symbol!("archive_read_support_filter_xz"),
            filter_zstd: symbol!("archive_read_support_filter_zstd"),
            add_passphrase: symbol!("archive_read_add_passphrase"),
            open_filenames: symbol!("archive_read_open_filenames"),
            set_callback_data: symbol!("archive_read_set_callback_data"),
            set_read_callback: symbol!("archive_read_set_read_callback"),
            set_seek_callback: symbol!("archive_read_set_seek_callback"),
            set_skip_callback: symbol!("archive_read_set_skip_callback"),
            open1: symbol!("archive_read_open1"),
            next_header: symbol!("archive_read_next_header"),
            data_block: symbol!("archive_read_data_block"),
            filter_bytes: symbol!("archive_filter_bytes"),
            error_string: symbol!("archive_error_string"),
            entry_pathname: symbol!("archive_entry_pathname"),
            entry_pathname_utf8: symbol!("archive_entry_pathname_utf8"),
            entry_hardlink: symbol!("archive_entry_hardlink"),
            entry_symlink: symbol!("archive_entry_symlink"),
            entry_filetype: symbol!("archive_entry_filetype"),
            entry_perm: symbol!("archive_entry_perm"),
            entry_is_encrypted: symbol!("archive_entry_is_encrypted"),
            entry_size_is_set: symbol!("archive_entry_size_is_set"),
            entry_size: symbol!("archive_entry_size"),
            version: symbol!("archive_version_string"),
            _library: library,
            formats: Vec::new(),
            filters: Vec::new(),
            // SAFETY: a fresh locale object from a static name; null when
            // the C library lacks C.UTF-8, which leaves the thread's locale.
            utf8_locale: unsafe {
                libc::newlocale(
                    libc::LC_CTYPE_MASK,
                    c"C.UTF-8".as_ptr(),
                    std::ptr::null_mut(),
                )
            } as usize,
        };
        codecs.formats = [
            Format::Zip,
            Format::Tar,
            Format::SevenZip,
            Format::Rar,
            Format::Raw,
        ]
        .into_iter()
        .filter(|&format| codecs.probe(|handle| codecs.register_format(handle, format)))
        .collect();
        codecs.filters = [Filter::Gzip, Filter::Bzip2, Filter::Xz, Filter::Zstd]
            .into_iter()
            .filter(|&filter| codecs.probe(|handle| codecs.register_filter(handle, filter)))
            .collect();
        Ok(codecs)
    }

    /// Whether `register` answers `ARCHIVE_OK` on a throwaway reader.
    fn probe(&self, register: impl Fn(Handle) -> c_int) -> bool {
        // SAFETY: a fresh handle is used once and freed here.
        unsafe {
            let handle = (self.read_new)();
            if handle.is_null() {
                return false;
            }
            let ok = register(handle) == ARCHIVE_OK;
            (self.read_free)(handle);
            ok
        }
    }

    fn register_format(&self, handle: Handle, format: Format) -> c_int {
        // SAFETY: `handle` is a live reader not yet opened.
        unsafe {
            match format {
                Format::Zip => (self.format_zip)(handle),
                Format::Tar => (self.format_tar)(handle),
                Format::SevenZip => (self.format_7zip)(handle),
                Format::Rar => match (self.format_rar)(handle) {
                    ARCHIVE_OK => (self.format_rar5)(handle),
                    other => other,
                },
                Format::Raw => (self.format_raw)(handle),
            }
        }
    }

    fn register_filter(&self, handle: Handle, filter: Filter) -> c_int {
        // SAFETY: `handle` is a live reader not yet opened.
        unsafe {
            match filter {
                Filter::Gzip => (self.filter_gzip)(handle),
                Filter::Bzip2 => (self.filter_bzip2)(handle),
                Filter::Xz => (self.filter_xz)(handle),
                Filter::Zstd => (self.filter_zstd)(handle),
            }
        }
    }

    pub(super) fn has_format(&self, format: Format) -> bool {
        self.formats.contains(&format)
    }

    pub(super) fn has_filter(&self, filter: Filter) -> bool {
        self.filters.contains(&filter)
    }

    pub(super) fn version(&self) -> String {
        // SAFETY: libarchive returns a static NUL-terminated string.
        unsafe { CStr::from_ptr((self.version)()) }
            .to_string_lossy()
            .into_owned()
    }
}

/// Runs libarchive calls on this thread under the UTF-8 locale, restoring
/// the previous one when dropped.
struct Utf8Locale(libc::locale_t);

impl Utf8Locale {
    fn enter(codecs: &Codecs) -> Option<Self> {
        let locale = codecs.utf8_locale as libc::locale_t;
        // SAFETY: `locale` is a live locale object that is never freed.
        (!locale.is_null()).then(|| Self(unsafe { libc::uselocale(locale) }))
    }
}

impl Drop for Utf8Locale {
    fn drop(&mut self) {
        // SAFETY: restores the thread's previous locale handle.
        unsafe { libc::uselocale(self.0) };
    }
}

/// The parts of a byte-split archive (`name.7z.001`, ...) served to
/// libarchive as one seekable stream: its formats seek to a header at the
/// end, which a file-by-file reader cannot reach.
struct Concat {
    parts: Vec<(std::fs::File, u64)>,
    total: u64,
    position: u64,
    buffer: Vec<u8>,
}

impl Concat {
    fn open(parts: &[PathBuf]) -> std::io::Result<Self> {
        let parts = parts
            .iter()
            .map(|path| {
                let file = std::fs::File::open(path)?;
                let len = file.metadata()?.len();
                Ok((file, len))
            })
            .collect::<std::io::Result<Vec<_>>>()?;
        Ok(Self {
            total: parts.iter().map(|(_, len)| len).sum(),
            parts,
            position: 0,
            buffer: vec![0; BLOCK],
        })
    }

    fn read(&mut self) -> std::io::Result<usize> {
        use std::os::unix::fs::FileExt;
        let mut start = 0;
        for (file, len) in &self.parts {
            if self.position < start + len {
                let within = self.position - start;
                let want = usize::try_from((len - within).min(BLOCK as u64)).unwrap_or(BLOCK);
                let read = file.read_at(&mut self.buffer[..want], within)?;
                self.position += read as u64;
                return Ok(read);
            }
            start += len;
        }
        Ok(0)
    }

    fn seek(&mut self, offset: i64, whence: c_int) -> i64 {
        let base = match whence {
            libc::SEEK_SET => 0,
            libc::SEEK_CUR => i128::from(self.position),
            libc::SEEK_END => i128::from(self.total),
            _ => return -30,
        };
        let target = base + i128::from(offset);
        if target < 0 {
            return -30;
        }
        self.position = u64::try_from(target).unwrap_or(u64::MAX).min(self.total);
        i64::try_from(self.position).unwrap_or(i64::MAX)
    }

    fn skip(&mut self, request: i64) -> i64 {
        let request = u64::try_from(request).unwrap_or(0);
        let skipped = request.min(self.total - self.position);
        self.position += skipped;
        i64::try_from(skipped).unwrap_or(i64::MAX)
    }
}

unsafe extern "C" fn concat_read(
    _: Handle,
    data: *mut c_void,
    buffer: *mut *const c_void,
) -> isize {
    // SAFETY: `data` is the `Concat` the reader owns for the handle's life,
    // and libarchive calls back only from the thread driving the reader.
    let concat = unsafe { &mut *data.cast::<Concat>() };
    match concat.read() {
        Ok(read) => {
            // SAFETY: libarchive reads `read` bytes before calling again.
            unsafe { *buffer = concat.buffer.as_ptr().cast() };
            isize::try_from(read).unwrap_or(isize::MAX)
        }
        Err(_) => -1,
    }
}

unsafe extern "C" fn concat_seek(_: Handle, data: *mut c_void, offset: i64, whence: c_int) -> i64 {
    // SAFETY: as in `concat_read`.
    unsafe { &mut *data.cast::<Concat>() }.seek(offset, whence)
}

unsafe extern "C" fn concat_skip(_: Handle, data: *mut c_void, request: i64) -> i64 {
    // SAFETY: as in `concat_read`.
    unsafe { &mut *data.cast::<Concat>() }.skip(request)
}

/// A libarchive failure, with its message.
#[derive(Debug)]
pub(super) struct NativeError {
    pub message: String,
}

/// What an entry is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Kind {
    File,
    Directory,
    /// A device, FIFO or socket.
    Special,
}

/// One entry's header.
#[derive(Debug)]
pub(super) struct Entry {
    /// The name as stored, native bytes.
    pub path: PathBuf,
    pub kind: Kind,
    /// A symlink or hardlink.
    pub link: bool,
    /// Its data needs a passphrase.
    pub encrypted: bool,
    /// The apparent size the header declares, when it declares one.
    pub size: Option<u64>,
    /// Permission bits.
    pub mode: u32,
}

/// One open archive.
pub(super) struct Reader {
    codecs: &'static Codecs,
    handle: NonNull<c_void>,
    /// The part names libarchive reads; kept alive with the handle.
    _paths: Vec<CString>,
    /// The concatenated stream of a byte-split archive, which libarchive
    /// points into until the handle is freed.
    concat: Option<Box<Concat>>,
}

impl Drop for Reader {
    fn drop(&mut self) {
        // SAFETY: this owner frees its unique handle exactly once.
        unsafe { (self.codecs.read_free)(self.handle.as_ptr()) };
    }
}

impl Reader {
    /// Opens `parts` in order as one archive of `format`; `joined` parts
    /// are byte splits of one file rather than RAR volumes.
    pub(super) fn open(
        parts: &[PathBuf],
        joined: bool,
        format: Format,
        filter: Option<Filter>,
        passphrase: Option<&SecretText>,
    ) -> Result<Self, NativeError> {
        let codecs = codecs().map_err(|message| NativeError { message })?;
        let paths = parts
            .iter()
            .map(|path| CString::new(path.as_os_str().as_bytes()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| NativeError {
                message: "a part name contains a NUL byte".into(),
            })?;
        // SAFETY: a fresh handle or null; ownership is established at once.
        let handle = NonNull::new(unsafe { (codecs.read_new)() }).ok_or_else(|| NativeError {
            message: "cannot create an archive reader".into(),
        })?;
        let _locale = Utf8Locale::enter(codecs);
        let mut reader = Self {
            codecs,
            handle,
            _paths: paths,
            concat: None,
        };
        let raw = reader.handle.as_ptr();
        if codecs.register_format(raw, format) != ARCHIVE_OK
            || filter.is_some_and(|filter| codecs.register_filter(raw, filter) != ARCHIVE_OK)
        {
            return Err(reader.error());
        }
        if let Some(passphrase) = passphrase {
            let bytes = passphrase.expose_secret().as_bytes();
            if bytes.contains(&0) {
                return Err(NativeError {
                    message: "the passphrase contains a NUL byte".into(),
                });
            }
            // A wiped buffer for the NUL-terminated copy libarchive needs.
            let mut terminated = Vec::with_capacity(bytes.len() + 1);
            terminated.extend_from_slice(bytes);
            terminated.push(0);
            let terminated = Secret::new(terminated);
            // SAFETY: the buffer is NUL-terminated and outlives the call;
            // libarchive copies the passphrase into the reader.
            if unsafe { (codecs.add_passphrase)(raw, terminated.as_bytes().as_ptr().cast()) }
                != ARCHIVE_OK
            {
                return Err(reader.error());
            }
        }
        let opened = if joined {
            let mut concat = Box::new(Concat::open(parts).map_err(|error| NativeError {
                message: error.to_string(),
            })?);
            let data: *mut Concat = &raw mut *concat;
            reader.concat = Some(concat);
            // SAFETY: the callbacks receive `data`, which the boxed `Concat`
            // in `reader` keeps at a fixed address until after the handle
            // is freed.
            unsafe {
                (codecs.set_read_callback)(raw, concat_read);
                (codecs.set_seek_callback)(raw, concat_seek);
                (codecs.set_skip_callback)(raw, concat_skip);
                (codecs.set_callback_data)(raw, data.cast());
                (codecs.open1)(raw)
            }
        } else {
            let mut pointers: Vec<*const c_char> =
                reader._paths.iter().map(|path| path.as_ptr()).collect();
            pointers.push(std::ptr::null());
            // SAFETY: a NULL-terminated array of NUL-terminated names, kept
            // alive in `_paths` for the reader's life.
            unsafe { (codecs.open_filenames)(raw, pointers.as_ptr(), BLOCK) }
        };
        if opened != ARCHIVE_OK {
            return Err(reader.error());
        }
        Ok(reader)
    }

    fn error(&self) -> NativeError {
        // SAFETY: the message belongs to the live reader and is copied
        // before the next call mutates it.
        let message = unsafe {
            let message = (self.codecs.error_string)(self.handle.as_ptr());
            if message.is_null() {
                "cannot read the archive".to_owned()
            } else {
                CStr::from_ptr(message).to_string_lossy().into_owned()
            }
        };
        NativeError { message }
    }

    /// The next entry's header, or `None` at the end.
    pub(super) fn next(&mut self) -> Result<Option<Entry>, NativeError> {
        let codecs = self.codecs;
        let _locale = Utf8Locale::enter(codecs);
        let mut entry: Handle = std::ptr::null_mut();
        // SAFETY: the entry and its strings are borrowed from the live
        // reader and copied before its next mutation.
        unsafe {
            match (codecs.next_header)(self.handle.as_ptr(), &raw mut entry) {
                ARCHIVE_EOF => return Ok(None),
                // A warning is a name that did not convert to the locale;
                // the raw or UTF-8 name below still names it.
                ARCHIVE_OK | ARCHIVE_WARN if !entry.is_null() => {}
                _ => return Err(self.error()),
            }
            let mut name = (codecs.entry_pathname)(entry);
            if name.is_null() {
                name = (codecs.entry_pathname_utf8)(entry);
            }
            if name.is_null() {
                return Err(self.error());
            }
            let path = PathBuf::from(std::ffi::OsStr::from_bytes(CStr::from_ptr(name).to_bytes()));
            let filetype = (codecs.entry_filetype)(entry) & AE_IFMT;
            let link = filetype == AE_IFLNK
                || !(codecs.entry_hardlink)(entry).is_null()
                || !(codecs.entry_symlink)(entry).is_null();
            let kind = match filetype {
                AE_IFREG => Kind::File,
                AE_IFDIR => Kind::Directory,
                _ => Kind::Special,
            };
            let size = ((codecs.entry_size_is_set)(entry) != 0)
                .then(|| u64::try_from((codecs.entry_size)(entry)).ok())
                .flatten();
            Ok(Some(Entry {
                path,
                kind,
                link,
                encrypted: (codecs.entry_is_encrypted)(entry) != 0,
                size,
                mode: (codecs.entry_perm)(entry),
            }))
        }
    }

    /// The current entry's next data block and its offset in the file, or
    /// `None` after its last block. Offsets skip over sparse holes.
    pub(super) fn block(&mut self) -> Result<Option<(u64, &[u8])>, NativeError> {
        let _locale = Utf8Locale::enter(self.codecs);
        let mut data: *const c_void = std::ptr::null();
        let mut len = 0usize;
        let mut offset = 0i64;
        // SAFETY: libarchive points `data` at `len` bytes it owns until the
        // next call on this reader, which `&mut self` excludes while the
        // returned slice lives.
        unsafe {
            match (self.codecs.data_block)(
                self.handle.as_ptr(),
                &raw mut data,
                &raw mut len,
                &raw mut offset,
            ) {
                ARCHIVE_EOF => Ok(None),
                ARCHIVE_OK | ARCHIVE_WARN => {
                    let offset = u64::try_from(offset).map_err(|_| NativeError {
                        message: "negative data offset".into(),
                    })?;
                    if len == 0 || data.is_null() {
                        Ok(Some((offset, &[])))
                    } else {
                        Ok(Some((offset, std::slice::from_raw_parts(data.cast(), len))))
                    }
                }
                _ => Err(self.error()),
            }
        }
    }

    /// Archive bytes read from the parts so far.
    pub(super) fn consumed(&self) -> u64 {
        // SAFETY: a read-only query of the live reader; -1 names the raw
        // input below every filter.
        u64::try_from(unsafe { (self.codecs.filter_bytes)(self.handle.as_ptr(), -1) }).unwrap_or(0)
    }
}
