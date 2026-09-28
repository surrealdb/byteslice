//! Borrowed views must never escape their borrow.
//!
//! `ByteSlice::with_borrowed` hands out a zero-copy view over bytes the caller
//! owns. Anything that produces an owned `ByteSlice` from that view (`clone`,
//! `slice`) must copy the bytes, and must never treat the caller's memory as a
//! heap allocation. Run under Miri, where a dangling read, a write through a
//! shared reference, or a foreign deallocation is reported as undefined
//! behaviour:
//!
//! ```text
//! cargo +nightly miri test --test borrowed_views
//! ```

use byteslice::ByteSlice;

/// Longer than the inline capacity, so views of it are not copied inline.
fn long_source() -> Vec<u8> {
    (0..64u8).collect()
}

#[test]
fn clone_of_a_borrowed_view_owns_its_bytes() {
    let src = long_source();
    let expected = src.clone();
    let owned = ByteSlice::with_borrowed(&src, |view| view.clone());
    drop(src);
    // Reading after the source is freed is a use-after-free if the clone
    // still points into it.
    assert_eq!(owned.as_slice(), &expected[..]);
    assert_eq!(owned.ref_count(), 1);
}

#[test]
fn slice_of_a_borrowed_view_owns_its_bytes() {
    let src = long_source();
    let expected = src.clone();
    let (whole, head, tail) = ByteSlice::with_borrowed(&src, |view| {
        (view.slice(..), view.slice(..40), view.slice(8..60))
    });
    drop(src);
    assert_eq!(whole.as_slice(), &expected[..]);
    assert_eq!(head.as_slice(), &expected[..40]);
    assert_eq!(tail.as_slice(), &expected[8..60]);
    // Dropping a slice taken at an offset must free only its own allocation,
    // never the caller's buffer.
    drop(tail);
    drop(head);
    drop(whole);
}

#[test]
fn slicing_a_borrowed_view_never_writes_to_the_source() {
    let src = long_source();
    let before = src.clone();
    ByteSlice::with_borrowed(&src, |view| {
        let a = view.slice(0..40);
        let b = view.slice(3..50);
        let c = view.clone();
        drop((a, b, c));
    });
    assert_eq!(src, before, "the source bytes were modified");
}

/// The inline capacity: 20 bytes on 64-bit targets, 16 on 32-bit.
fn inline_capacity() -> usize {
    (0..64usize)
        .take_while(|&n| ByteSlice::from(&[0u8; 64][..n]).is_inline())
        .last()
        .unwrap()
}

#[test]
fn short_borrowed_views_are_inline_copies() {
    let cap = inline_capacity();
    let src = long_source();
    let (at_capacity, over) = ByteSlice::with_borrowed(&src[..cap + 1], |view| {
        assert!(!view.is_inline());
        (view.slice(..cap), view.slice(..cap + 1))
    });
    let inline = ByteSlice::with_borrowed(&src[..cap], |view| {
        assert!(view.is_inline());
        view.clone()
    });
    let expected = src.clone();
    drop(src);
    assert!(at_capacity.is_inline());
    assert_eq!(at_capacity.as_slice(), &expected[..cap]);
    assert!(!over.is_inline());
    assert_eq!(over.as_slice(), &expected[..cap + 1]);
    assert_eq!(inline.as_slice(), &expected[..cap]);
}

#[test]
fn owned_copies_of_a_view_share_one_allocation() {
    let src = long_source();
    let owned = ByteSlice::with_borrowed(&src, |view| view.clone());
    drop(src);
    // Once owned, clones and slices share the new allocation as usual.
    let clone = owned.clone();
    let sub = owned.slice(10..50);
    assert_eq!(owned.ref_count(), 3);
    drop(clone);
    drop(sub);
    assert_eq!(owned.ref_count(), 1);
}

#[test]
fn unchecked_view_copies_outlive_the_source() {
    let src = long_source();
    let expected = src.clone();
    // SAFETY: the view itself is dropped before `src`.
    let view = unsafe { ByteSlice::from_borrowed_unchecked(&src) };
    let clone = view.clone();
    let sub = view.slice(5..45);
    drop(view);
    drop(src);
    assert_eq!(clone.as_slice(), &expected[..]);
    assert_eq!(sub.as_slice(), &expected[5..45]);
}

#[test]
fn borrowed_views_compare_like_owned_values() {
    let src = long_source();
    let owned = ByteSlice::from(&src[..]);
    ByteSlice::with_borrowed(&src, |view| {
        assert_eq!(*view, owned);
        assert_eq!(view.cmp(&owned), core::cmp::Ordering::Equal);
        assert_eq!(view.ref_count(), 1);
        assert_eq!(view.as_slice(), &src[..]);
    });
}

#[test]
fn views_can_be_sent_to_other_threads_as_owned_copies() {
    let src = long_source();
    let expected = src.clone();
    let owned = ByteSlice::with_borrowed(&src, |view| view.slice(2..62));
    drop(src);
    let handle = std::thread::spawn(move || owned.as_slice().to_vec());
    assert_eq!(handle.join().unwrap(), &expected[2..62]);
}
