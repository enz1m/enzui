//! Stores short UTF-8 strings inline and spills longer strings without truncation.

/// Bytes storable inline, without touching the allocator.
pub const INLINE_CAPACITY: usize = 22;

/// An inline-or-spilled UTF-8 string for a grid cell.
///
/// Up to [`INLINE_CAPACITY`] bytes are stored inline; longer strings spill to
/// the heap without truncation. Reading it back does not re-validate UTF-8.
#[derive(Clone)]
pub struct InlineGrapheme(Repr);

// Private so `buf[..len]` can only be written by `InlineGrapheme::new`, from
// the bytes of a `&str`; `as_str` relies on that.
#[derive(Clone)]
enum Repr {
    Inline { buf: [u8; INLINE_CAPACITY], len: u8 },
    Spilled(Box<str>),
}

impl InlineGrapheme {
    /// An empty cell (blank). Allocation-free.
    pub const fn empty() -> Self {
        Self(Repr::Inline {
            buf: [0u8; INLINE_CAPACITY],
            len: 0,
        })
    }

    /// Store `s` exactly, inline when it fits and on the heap when it does not.
    pub fn new(s: &str) -> Self {
        let bytes = s.as_bytes();
        if bytes.len() <= INLINE_CAPACITY {
            let mut buf = [0u8; INLINE_CAPACITY];
            buf[..bytes.len()].copy_from_slice(bytes);
            Self(Repr::Inline {
                buf,
                len: bytes.len() as u8,
            })
        } else {
            Self(Repr::Spilled(s.into()))
        }
    }

    /// The stored string.
    pub fn as_str(&self) -> &str {
        match &self.0 {
            Repr::Inline { buf, len } => {
                let bytes = &buf[..usize::from(*len)];
                debug_assert!(
                    std::str::from_utf8(bytes).is_ok(),
                    "only `new` writes the buffer, from a `&str`"
                );
                #[allow(
                    unsafe_code,
                    reason = "skips re-validating UTF-8 copied whole from a `&str`"
                )]
                // SAFETY: `Repr` is private and only `new` writes `buf[..len]`,
                // copying it whole from a `&str`, so it is valid UTF-8.
                unsafe {
                    std::str::from_utf8_unchecked(bytes)
                }
            }
            Repr::Spilled(s) => s,
        }
    }

    /// Returns whether the stored string has no bytes.
    pub fn is_empty(&self) -> bool {
        match &self.0 {
            Repr::Inline { len, .. } => *len == 0,
            Repr::Spilled(s) => s.is_empty(),
        }
    }

    /// Length of the stored string in bytes.
    pub fn len(&self) -> usize {
        match &self.0 {
            Repr::Inline { len, .. } => usize::from(*len),
            Repr::Spilled(s) => s.len(),
        }
    }

    /// Returns whether the string is stored inline without allocation.
    pub fn is_inline(&self) -> bool {
        matches!(self.0, Repr::Inline { .. })
    }

    /// Heap bytes owned by the stored string, or zero when inline.
    pub fn heap_bytes(&self) -> usize {
        match &self.0 {
            Repr::Inline { .. } => 0,
            Repr::Spilled(s) => s.len(),
        }
    }
}

impl Default for InlineGrapheme {
    fn default() -> Self {
        Self::empty()
    }
}

impl From<&str> for InlineGrapheme {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<char> for InlineGrapheme {
    fn from(c: char) -> Self {
        let mut buf = [0u8; 4];
        Self::new(c.encode_utf8(&mut buf))
    }
}

impl std::ops::Deref for InlineGrapheme {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for InlineGrapheme {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq for InlineGrapheme {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for InlineGrapheme {}

impl PartialEq<str> for InlineGrapheme {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for InlineGrapheme {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl std::hash::Hash for InlineGrapheme {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl std::fmt::Debug for InlineGrapheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_str(), f)
    }
}

impl std::fmt::Display for InlineGrapheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
