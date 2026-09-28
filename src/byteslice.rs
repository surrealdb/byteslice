//! Immutable zero-copy byte slice with Small String Optimization (SSO)
//! and prefix-accelerated comparison (German String design).

use alloc::alloc::{alloc, dealloc, handle_alloc_error, Layout};
use alloc::vec::Vec;
use core::mem::ManuallyDrop;
use core::ops::Deref;
use core::sync::atomic::{AtomicU64, Ordering};

#[cfg(target_pointer_width = "64")]
const INLINE_CAPACITY: usize = 20;

#[cfg(target_pointer_width = "32")]
const INLINE_CAPACITY: usize = 16;

const PREFIX_SIZE: usize = 4;

/// The longest slice: lengths and header offsets are stored as `u32`, and a
/// heap slice's `header_offset` (header size plus sub-slice start) must fit.
const MAX_LEN: usize = u32::MAX as usize - core::mem::size_of::<HeapHeader>();

#[repr(C)]
struct HeapHeader {
    ref_count: AtomicU64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ShortRepr {
    len: u32,
    data: [u8; INLINE_CAPACITY],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LongRepr {
    len: u32,
    prefix: [u8; PREFIX_SIZE],
    data_ptr: *const u8,
    original_len: u32,
    header_offset: u32,
}

#[repr(C)]
union ViewRepr {
    short: ManuallyDrop<ShortRepr>,
    long: ManuallyDrop<LongRepr>,
}

/// An immutable, zero-copy byte slice with Small String Optimization (SSO)
/// and prefix-accelerated comparison.
#[repr(C)]
pub struct ByteSlice {
    repr: ViewRepr,
}

#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Send for ByteSlice {}
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl Sync for ByteSlice {}

impl Default for ByteSlice {
    fn default() -> Self {
        Self::empty()
    }
}

impl ByteSlice {
    /// Creates a new empty slice.
    #[inline]
    pub const fn empty() -> Self {
        Self {
            repr: ViewRepr {
                short: ManuallyDrop::new(ShortRepr {
                    len: 0,
                    data: [0; INLINE_CAPACITY],
                }),
            },
        }
    }

    /// Returns true if the slice data is stored inline (no heap allocation).
    #[inline]
    pub fn is_inline(&self) -> bool {
        self.len() <= INLINE_CAPACITY
    }

    /// Returns the length of the slice in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        unsafe { self.repr.short.len as usize }
    }

    /// Returns true if the slice is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the inline 4-byte prefix for accelerated comparisons.
    #[inline]
    pub fn prefix(&self) -> &[u8] {
        let prefix_len = PREFIX_SIZE.min(self.len());
        unsafe { &self.repr.short.data[..prefix_len] }
    }

    #[inline(always)]
    fn prefix_u32(&self) -> u32 {
        unsafe {
            let ptr = (self as *const Self as *const u8).add(4).cast::<u32>();
            u32::from_be(ptr.read_unaligned())
        }
    }

    /// Creates a new slice from an existing byte slice.
    /// Inlines values <= 20 bytes (16 on 32-bit targets) with zero allocations.
    ///
    /// # Panics
    ///
    /// Panics if the input is longer than `u32::MAX - 8` bytes (just under 4GB).
    #[allow(clippy::cast_possible_truncation)]
    pub fn from_slice(src: &[u8]) -> Self {
        let src_len = src.len();
        assert!(src_len <= MAX_LEN, "slice length exceeds 4GB limit");

        if src_len <= INLINE_CAPACITY {
            let mut data = [0u8; INLINE_CAPACITY];
            data[..src_len].copy_from_slice(src);
            Self {
                repr: ViewRepr {
                    short: ManuallyDrop::new(ShortRepr {
                        len: src_len as u32,
                        data,
                    }),
                },
            }
        } else {
            let mut prefix = [0u8; PREFIX_SIZE];
            prefix.copy_from_slice(&src[..PREFIX_SIZE]);

            let header_size = core::mem::size_of::<HeapHeader>();
            let alignment = core::mem::align_of::<HeapHeader>();
            let total_size = header_size + src_len;
            let layout = Layout::from_size_align(total_size, alignment).expect("valid layout");

            unsafe {
                let heap_ptr = alloc(layout);
                if heap_ptr.is_null() {
                    handle_alloc_error(layout);
                }

                // Initialize atomic ref_count to 1
                let header: *mut HeapHeader = heap_ptr.cast();
                (*header).ref_count = AtomicU64::new(1);

                // Copy payload after header
                let payload_ptr = heap_ptr.add(header_size);
                core::ptr::copy_nonoverlapping(src.as_ptr(), payload_ptr, src_len);

                Self {
                    repr: ViewRepr {
                        long: ManuallyDrop::new(LongRepr {
                            len: src_len as u32,
                            prefix,
                            data_ptr: payload_ptr,
                            original_len: src_len as u32,
                            header_offset: header_size as u32,
                        }),
                    },
                }
            }
        }
    }

    /// Creates a borrowed, zero-allocation `ByteSlice` view over arbitrary bytes.
    ///
    /// Short views (up to 20 bytes, or 16 on 32-bit targets) are inline
    /// copies. Longer views point into `src`. Clones and sub-slices of a view
    /// are owned copies, which may outlive `src`.
    ///
    /// Prefer [`with_borrowed`](Self::with_borrowed), which is safe.
    ///
    /// # Safety
    ///
    /// The returned `ByteSlice` itself must not outlive `src`.
    ///
    /// # Panics
    ///
    /// Panics if `src` is longer than `u32::MAX - 8` bytes (just under 4GB).
    #[inline]
    pub unsafe fn from_borrowed_unchecked(src: &[u8]) -> Self {
        let src_len = src.len();
        assert!(src_len <= MAX_LEN, "slice length exceeds 4GB limit");
        if src_len <= INLINE_CAPACITY {
            let mut data = [0u8; INLINE_CAPACITY];
            data[..src_len].copy_from_slice(src);
            Self {
                repr: ViewRepr {
                    short: ManuallyDrop::new(ShortRepr {
                        len: src_len as u32,
                        data,
                    }),
                },
            }
        } else {
            let mut prefix = [0u8; PREFIX_SIZE];
            prefix.copy_from_slice(&src[..PREFIX_SIZE]);
            Self {
                repr: ViewRepr {
                    long: ManuallyDrop::new(LongRepr {
                        len: src_len as u32,
                        prefix,
                        data_ptr: src.as_ptr(),
                        original_len: 0,
                        header_offset: 0,
                    }),
                },
            }
        }
    }

    /// Executes a closure with a borrowed, zero-allocation `ByteSlice` view over arbitrary bytes.
    ///
    /// The view is only reachable as `&ByteSlice` inside the closure, so it
    /// cannot outlive `src`. Anything that produces an owned `ByteSlice` from
    /// it (`clone`, `slice`) copies the bytes, so that result can outlive
    /// `src`:
    ///
    /// ```
    /// use byteslice::ByteSlice;
    ///
    /// let src = vec![7u8; 64];
    /// let owned = ByteSlice::with_borrowed(&src, |view| view.clone());
    /// drop(src);
    /// assert_eq!(owned.as_slice(), &[7u8; 64][..]);
    /// ```
    ///
    /// The view itself cannot leave the closure:
    ///
    /// ```compile_fail
    /// use byteslice::ByteSlice;
    ///
    /// let src = vec![7u8; 64];
    /// let view = ByteSlice::with_borrowed(&src, |view| view);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `src` is longer than `u32::MAX - 8` bytes (just under 4GB).
    #[inline]
    pub fn with_borrowed<F, R>(src: &[u8], f: F) -> R
    where
        F: FnOnce(&ByteSlice) -> R,
    {
        // SAFETY: `view` is dropped at the end of this function, before
        // `src`'s borrow ends, and the closure only sees `&view`.
        let view = unsafe { Self::from_borrowed_unchecked(src) };
        f(&view)
    }

    /// Creates a slice from static bytes.
    #[inline]
    pub fn from_static(src: &'static [u8]) -> Self {
        Self::from_slice(src)
    }

    /// Creates a slice by copying the bytes of a `bytes::Bytes` buffer.
    #[inline]
    pub fn from_bytes(b: &bytes::Bytes) -> Self {
        Self::from_slice(b.as_ref())
    }

    /// Returns a sub-range of this slice. Short sub-slices (up to 20 bytes,
    /// or 16 on 32-bit targets) are inline copies; longer ones share this
    /// slice's heap allocation. A sub-slice of a borrowed view copies its
    /// bytes into a new allocation.
    ///
    /// # Panics
    ///
    /// Panics if the slice bounds are invalid or out of range.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn slice(&self, range: impl core::ops::RangeBounds<usize>) -> Self {
        use core::ops::Bound;

        let self_len = self.len();
        let begin = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n.checked_add(1).expect("out of range"),
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&n) => n.checked_add(1).expect("out of range"),
            Bound::Excluded(&n) => n,
            Bound::Unbounded => self_len,
        };

        assert!(begin <= end && end <= self_len, "slice bounds out of range");
        let sub_len = end - begin;

        if sub_len <= INLINE_CAPACITY {
            // Fast path: target is small enough to inline
            let mut data = [0u8; INLINE_CAPACITY];
            data[..sub_len].copy_from_slice(&self.as_slice()[begin..end]);
            Self {
                repr: ViewRepr {
                    short: ManuallyDrop::new(ShortRepr {
                        len: sub_len as u32,
                        data,
                    }),
                },
            }
        } else if let Some(heap_header) = self.heap_header() {
            // Subslice is long: share heap allocation with incremented atomic ref_count
            heap_header.ref_count.fetch_add(1, Ordering::Relaxed);

            let mut prefix = [0u8; PREFIX_SIZE];
            let sub_bytes = &self.as_slice()[begin..end];
            prefix.copy_from_slice(&sub_bytes[..PREFIX_SIZE]);

            unsafe {
                Self {
                    repr: ViewRepr {
                        long: ManuallyDrop::new(LongRepr {
                            len: sub_len as u32,
                            prefix,
                            data_ptr: self.repr.long.data_ptr.add(begin),
                            original_len: self.repr.long.original_len,
                            // Cannot overflow: `from_slice` bounds the length
                            // by `MAX_LEN`.
                            header_offset: self
                                .repr
                                .long
                                .header_offset
                                .checked_add(begin as u32)
                                .expect("header offset overflow"),
                        }),
                    },
                }
            }
        } else {
            // A borrowed view has no heap allocation to share, and the result
            // may outlive the borrow: copy the bytes.
            Self::from_slice(&self.as_slice()[begin..end])
        }
    }

    #[inline(always)]
    pub fn as_slice(&self) -> &[u8] {
        let len = self.len();
        if self.is_inline() {
            unsafe { &self.repr.short.data[..len] }
        } else {
            unsafe { core::slice::from_raw_parts(self.repr.long.data_ptr, len) }
        }
    }

    /// The shared allocation's header, or `None` for an inline slice or a
    /// borrowed view (a long view from `from_borrowed_unchecked`, which has
    /// `header_offset == 0` and points into memory it does not own).
    ///
    /// Every refcount access goes through this check, in every build profile:
    /// for a borrowed view, the "header" would be the caller's own bytes.
    #[inline(always)]
    fn heap_header(&self) -> Option<&HeapHeader> {
        if self.is_inline() || unsafe { self.repr.long.header_offset == 0 } {
            return None;
        }
        // SAFETY: a long slice with a non-zero `header_offset` was created by
        // `from_slice` (or sliced from one), so `data_ptr - header_offset` is
        // the start of its live allocation, which begins with a `HeapHeader`.
        unsafe {
            let header_ptr = self
                .repr
                .long
                .data_ptr
                .sub(self.repr.long.header_offset as usize)
                .cast::<HeapHeader>();
            Some(&*header_ptr)
        }
    }

    /// Returns current reference count (1 for inlined data).
    pub fn ref_count(&self) -> u64 {
        self.heap_header()
            .map_or(1, |header| header.ref_count.load(Ordering::Acquire))
    }
}

impl Deref for ByteSlice {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl AsRef<[u8]> for ByteSlice {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl core::borrow::Borrow<[u8]> for ByteSlice {
    #[inline]
    fn borrow(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Clone for ByteSlice {
    #[inline]
    fn clone(&self) -> Self {
        if self.is_inline() {
            unsafe {
                Self {
                    repr: ViewRepr {
                        short: self.repr.short,
                    },
                }
            }
        } else if let Some(header) = self.heap_header() {
            header.ref_count.fetch_add(1, Ordering::Relaxed);
            unsafe {
                Self {
                    repr: ViewRepr {
                        long: self.repr.long,
                    },
                }
            }
        } else {
            // A borrowed view: the clone may outlive the borrow, so it owns a
            // copy of the bytes.
            Self::from_slice(self.as_slice())
        }
    }
}

impl Drop for ByteSlice {
    fn drop(&mut self) {
        // Inline slices and borrowed views own no allocation.
        let Some(header) = self.heap_header() else {
            return;
        };
        if header.ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            unsafe {
                let header_size = core::mem::size_of::<HeapHeader>();
                let alignment = core::mem::align_of::<HeapHeader>();
                let total_size = header_size + self.repr.long.original_len as usize;
                let layout = Layout::from_size_align(total_size, alignment).expect("valid layout");
                let heap_ptr =
                    (self.repr.long.data_ptr as *mut u8).sub(self.repr.long.header_offset as usize);
                dealloc(heap_ptr, layout);
            }
        }
    }
}

impl PartialEq for ByteSlice {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }

        if self.is_inline() {
            return self.as_slice() == other.as_slice();
        }

        // Fast path: compare 4-byte prefixes before dereferencing heap pointers
        if self.prefix_u32() != other.prefix_u32() {
            return false;
        }

        self.as_slice() == other.as_slice()
    }
}

impl Eq for ByteSlice {}

impl PartialOrd for ByteSlice {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ByteSlice {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        // Fast path: check 32-bit big-endian prefix ordering first in a single CPU register instruction
        let p_self = self.prefix_u32();
        let p_other = other.prefix_u32();
        if p_self != p_other {
            return p_self.cmp(&p_other);
        }
        self.as_slice().cmp(other.as_slice())
    }
}

impl core::hash::Hash for ByteSlice {
    #[inline]
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

impl core::fmt::Debug for ByteSlice {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match core::str::from_utf8(self.as_slice()) {
            Ok(s) => write!(f, "ByteSlice({s:?})"),
            Err(_) => write!(f, "ByteSlice({:?})", self.as_slice()),
        }
    }
}

impl From<&[u8]> for ByteSlice {
    #[inline]
    fn from(s: &[u8]) -> Self {
        Self::from_slice(s)
    }
}

impl From<&str> for ByteSlice {
    #[inline]
    fn from(s: &str) -> Self {
        Self::from_slice(s.as_bytes())
    }
}

impl From<Vec<u8>> for ByteSlice {
    #[inline]
    fn from(v: Vec<u8>) -> Self {
        Self::from_slice(&v)
    }
}

impl From<bytes::Bytes> for ByteSlice {
    #[inline]
    fn from(b: bytes::Bytes) -> Self {
        Self::from_bytes(&b)
    }
}

impl PartialEq<bytes::Bytes> for ByteSlice {
    #[inline]
    fn eq(&self, other: &bytes::Bytes) -> bool {
        self.as_slice() == other.as_ref()
    }
}

impl PartialEq<ByteSlice> for bytes::Bytes {
    #[inline]
    fn eq(&self, other: &ByteSlice) -> bool {
        self.as_ref() == other.as_slice()
    }
}

impl PartialEq<Vec<u8>> for ByteSlice {
    #[inline]
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl PartialEq<&Vec<u8>> for ByteSlice {
    #[inline]
    fn eq(&self, other: &&Vec<u8>) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl PartialEq<ByteSlice> for Vec<u8> {
    #[inline]
    fn eq(&self, other: &ByteSlice) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl PartialEq<ByteSlice> for &Vec<u8> {
    #[inline]
    fn eq(&self, other: &ByteSlice) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl From<alloc::string::String> for ByteSlice {
    #[inline]
    fn from(s: alloc::string::String) -> Self {
        Self::from_slice(s.as_bytes())
    }
}

impl PartialEq<str> for ByteSlice {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.as_slice() == other.as_bytes()
    }
}

impl PartialEq<&str> for ByteSlice {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.as_slice() == other.as_bytes()
    }
}

impl PartialEq<ByteSlice> for &str {
    #[inline]
    fn eq(&self, other: &ByteSlice) -> bool {
        self.as_bytes() == other.as_slice()
    }
}

impl PartialEq<ByteSlice> for str {
    #[inline]
    fn eq(&self, other: &ByteSlice) -> bool {
        self.as_bytes() == other.as_slice()
    }
}

impl PartialEq<[u8]> for ByteSlice {
    #[inline]
    fn eq(&self, other: &[u8]) -> bool {
        self.as_slice() == other
    }
}

impl PartialEq<&[u8]> for ByteSlice {
    #[inline]
    fn eq(&self, other: &&[u8]) -> bool {
        self.as_slice() == *other
    }
}

impl PartialEq<ByteSlice> for &[u8] {
    #[inline]
    fn eq(&self, other: &ByteSlice) -> bool {
        *self == other.as_slice()
    }
}

impl PartialEq<ByteSlice> for [u8] {
    #[inline]
    fn eq(&self, other: &ByteSlice) -> bool {
        self == other.as_slice()
    }
}

impl<const N: usize> PartialEq<[u8; N]> for ByteSlice {
    #[inline]
    fn eq(&self, other: &[u8; N]) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<const N: usize> PartialEq<&[u8; N]> for ByteSlice {
    #[inline]
    fn eq(&self, other: &&[u8; N]) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<const N: usize> PartialEq<ByteSlice> for [u8; N] {
    #[inline]
    fn eq(&self, other: &ByteSlice) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<const N: usize> PartialEq<ByteSlice> for &[u8; N] {
    #[inline]
    fn eq(&self, other: &ByteSlice) -> bool {
        self.as_slice() == other.as_slice()
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for ByteSlice {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(self.as_slice())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for ByteSlice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ByteSliceVisitor;

        impl<'de> serde::de::Visitor<'de> for ByteSliceVisitor {
            type Value = ByteSlice;

            fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                formatter.write_str("a byte slice")
            }

            fn visit_bytes<E>(self, v: &[u8]) -> Result<ByteSlice, E>
            where
                E: serde::de::Error,
            {
                Ok(ByteSlice::from_slice(v))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut bytes = Vec::new();
                while let Some(byte) = seq.next_element()? {
                    bytes.push(byte);
                }
                Ok(ByteSlice::from_slice(&bytes))
            }
        }

        deserializer.deserialize_bytes(ByteSliceVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_borrowed_unchecked_view() {
        let long_data = b"a_very_long_byte_slice_that_exceeds_inline_capacity_and_is_borrowed!";
        let view = unsafe { ByteSlice::from_borrowed_unchecked(&long_data[..]) };
        assert_eq!(view.len(), long_data.len());
        assert_eq!(view.as_slice(), &long_data[..]);
        assert_eq!(view.ref_count(), 1);

        let owned = ByteSlice::from(&long_data[..]);
        assert_eq!(view, owned);
        assert_eq!(view.cmp(&owned), core::cmp::Ordering::Equal);

        let cloned = view.clone();
        assert_eq!(cloned.as_slice(), &long_data[..]);
        assert_eq!(cloned, view);
    }

    #[test]
    fn test_short_inline_slice() {
        let s = ByteSlice::from("hello world");
        assert_eq!(s.len(), 11);
        assert!(s.is_inline());
        assert_eq!(s.ref_count(), 1);
        assert_eq!(&*s, b"hello world");
        assert_eq!(s.prefix(), b"hell");

        let cloned = s.clone();
        assert_eq!(cloned.len(), 11);
        assert!(cloned.is_inline());
        assert_eq!(s, cloned);
    }

    #[test]
    fn test_long_heap_slice() {
        let long_str = "this is a very long string that definitely exceeds twenty bytes!";
        let s = ByteSlice::from(long_str);
        assert_eq!(s.len(), long_str.len());
        assert!(!s.is_inline());
        assert_eq!(s.ref_count(), 1);
        assert_eq!(&*s, long_str.as_bytes());
        assert_eq!(s.prefix(), &long_str.as_bytes()[..4]);

        let sub_long = s.slice(10..40);
        assert!(!sub_long.is_inline());
        assert_eq!(s.ref_count(), 2);
        assert_eq!(sub_long.len(), 30);
        assert_eq!(&*sub_long, &long_str.as_bytes()[10..40]);

        let sub_short = s.slice(0..10);
        assert!(sub_short.is_inline());
        assert_eq!(sub_short.len(), 10);
        assert_eq!(&*sub_short, &long_str.as_bytes()[0..10]);
        drop(sub_long);
        assert_eq!(s.ref_count(), 1);
    }

    #[test]
    fn test_prefix_accelerated_ordering() {
        let s1 = ByteSlice::from("apple_pie_delicious");
        let s2 = ByteSlice::from("banana_split_sweet");
        assert!(s1 < s2);
        assert_eq!(s1.prefix(), b"appl");
        assert_eq!(s2.prefix(), b"bana");
    }

    #[test]
    fn test_struct_size_is_24_bytes() {
        #[cfg(target_pointer_width = "64")]
        assert_eq!(core::mem::size_of::<ByteSlice>(), 24);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_serde_roundtrip() {
        let orig = ByteSlice::from("hello serialization");
        let json = serde_json::to_string(&orig).unwrap();
        let deserialized: ByteSlice = serde_json::from_str(&json).unwrap();
        assert_eq!(orig, deserialized);
    }
}

#[test]
fn test_empty_slice_properties() {
    let empty = ByteSlice::empty();
    assert_eq!(empty.len(), 0);
    assert!(empty.is_empty());
    assert!(empty.is_inline());
    assert_eq!(empty.prefix(), b"");
    assert_eq!(&*empty, b"");

    let default_slice = ByteSlice::default();
    assert_eq!(empty, default_slice);

    let from_empty_str = ByteSlice::from("");
    assert_eq!(empty, from_empty_str);

    let from_empty_slice = ByteSlice::from_slice(&[]);
    assert_eq!(empty, from_empty_slice);
}

#[test]
fn test_sso_boundary_conditions() {
    // Exactly 19 bytes (inlined)
    let b19 = ByteSlice::from("1234567890123456789");
    assert_eq!(b19.len(), 19);
    assert!(b19.is_inline());

    // Exactly 20 bytes (inlined on 64-bit)
    #[cfg(target_pointer_width = "64")]
    {
        let b20 = ByteSlice::from("12345678901234567890");
        assert_eq!(b20.len(), 20);
        assert!(b20.is_inline());
    }

    // Exactly 21 bytes (heap allocated on 64-bit)
    #[cfg(target_pointer_width = "64")]
    {
        let b21 = ByteSlice::from("123456789012345678901");
        assert_eq!(b21.len(), 21);
        assert!(!b21.is_inline());
        assert_eq!(b21.prefix(), b"1234");
    }
}

#[test]
fn test_slice_range_variations() {
    let original = ByteSlice::from("abcdefghijklmnopqrstuvwxyz_0123456789");
    assert!(!original.is_inline());

    // Unbounded
    let full = original.slice(..);
    assert_eq!(full, original);

    // Inclusive start, unbounded end
    let s1 = original.slice(5..);
    assert_eq!(&*s1, b"fghijklmnopqrstuvwxyz_0123456789");

    // Unbounded start, excluded end
    let s2 = original.slice(..10);
    assert_eq!(&*s2, b"abcdefghij");
    assert!(s2.is_inline()); // <= 20 bytes downgraded to inline

    // Inclusive start, included end
    let s3 = original.slice(0..=3);
    assert_eq!(&*s3, b"abcd");
    assert!(s3.is_inline());

    // Empty sub-slice
    let empty = original.slice(10..10);
    assert_eq!(empty.len(), 0);
    assert!(empty.is_inline());
}

#[test]
#[should_panic(expected = "slice bounds out of range")]
fn test_slice_out_of_bounds_end() {
    let s = ByteSlice::from("hello");
    let _ = s.slice(0..100);
}

#[test]
#[should_panic(expected = "slice bounds out of range")]
#[allow(clippy::reversed_empty_ranges)]
fn test_slice_start_greater_than_end() {
    let s = ByteSlice::from("hello");
    let _ = s.slice(4..2);
}

#[test]
fn test_hash_map_and_set_compatibility() {
    use alloc::collections::BTreeSet;

    let mut set = BTreeSet::new();
    set.insert(ByteSlice::from("apple"));
    set.insert(ByteSlice::from("banana"));
    set.insert(ByteSlice::from(
        "a_very_long_fruit_name_that_is_heap_allocated",
    ));

    assert!(set.contains(&ByteSlice::from("apple")));
    assert!(set.contains(&ByteSlice::from("banana")));
    assert!(set.contains(&ByteSlice::from(
        "a_very_long_fruit_name_that_is_heap_allocated"
    )));
    assert!(!set.contains(&ByteSlice::from("orange")));
}

#[test]
fn test_partial_ord_and_ord_consistency() {
    let s1 = ByteSlice::from("alpha");
    let s2 = ByteSlice::from("alphabet");
    let s3 = ByteSlice::from("beta");
    let s4 = ByteSlice::from("beta_very_long_string_exceeding_twenty_bytes_allocation");

    assert!(s1 < s2);
    assert!(s2 < s3);
    assert!(s3 < s4);
    assert_eq!(s1.cmp(&s1), core::cmp::Ordering::Equal);
    assert_eq!(s1.cmp(&s2), core::cmp::Ordering::Less);
    assert_eq!(s2.cmp(&s1), core::cmp::Ordering::Greater);
}

#[test]
fn test_bytes_interoperability() {
    let raw = b"interoperability test payload across bytes and byteslice";
    let b = bytes::Bytes::copy_from_slice(raw);
    let bs = ByteSlice::from(b);

    assert_eq!(&*bs, raw);
    assert_eq!(bs.prefix(), &raw[..4]);
}
