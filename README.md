# byteslice

A thin, immutable, zero-copy **24-byte** slice with Small String Optimization (SSO) and prefix-accelerated comparisons (German String design).

Designed as the core memory-efficient byte slice primitive for high-performance databases, key-value stores, and query engines (such as SurrealDB, SurrealKV, and SurrealMX).

## Examples

### Basic Usage

Slices up to 20 bytes are stored completely inline on the stack with zero heap allocation, zero pointer indirection, and zero atomic refcount overhead:

```rust
use byteslice::ByteSlice;

// Inlined on stack (zero heap allocation, zero atomic ops)
let short = ByteSlice::from("key_00000001");
assert!(short.is_inline());
assert_eq!(&*short, b"key_00000001");

// Long string: allocated on heap with atomic ref counting
let long = ByteSlice::from("a very long key that exceeds twenty bytes in length!");
assert!(!long.is_inline());
assert_eq!(long.prefix(), b"a ve");

// Zero-copy sub-slicing (automatically downgrades to inline if <= 20 bytes)
let sub = long.slice(0..10);
assert!(sub.is_inline());
```

### Prefix-Accelerated Comparisons

For allocations larger than 20 bytes, the first 4 bytes of data are stored directly inside the 24-byte struct as an inline prefix. When sorting, binary searching, or performing lookups, non-equal comparisons resolve immediately by comparing the 4-byte prefix in a single 32-bit CPU register check without dereferencing the heap pointer:

```rust
use byteslice::ByteSlice;

let s1 = ByteSlice::from("apple_pie_delicious");
let s2 = ByteSlice::from("banana_split_sweet");

// Compares "appl" vs "bana" inline without touching heap memory
assert!(s1 < s2);
```

## Architecture & Layout

`ByteSlice` is exactly **24 bytes** on 64-bit architectures (3 words, matching `Vec<u8>` and standard slices):

```text
Short Representation (len <= 20 bytes):
+----------------+-----------------------------------------------+
| len (4 bytes)  | inline data buffer (20 bytes)                 | = 24 bytes
+----------------+-----------------------------------------------+

Long Representation (len > 20 bytes):
+----------------+----------------+----------------+------------------+----------------+
| len (4 bytes)  | prefix (4B)    | *heap (8B)     | orig_len (4B)    | offset (4B)    | = 24 bytes
+----------------+----------------+----------------+------------------+----------------+
```

### Why byteslice?

1. **Small Footprint (24 bytes vs 32 bytes for `bytes::Bytes`)**:
   Fits neatly into cache lines and registers.
2. **Small String Optimization (SSO up to 20 bytes)**:
   Any slice up to 20 bytes is stored **completely inline on the stack**. No heap allocations, no pointer indirections, and no atomic reference counting penalties.
3. **Prefix-Accelerated Comparison (`prefix: [u8; 4]`)**:
   For long allocations, the first 4 bytes are cached directly inside the 24-byte struct. Sorting, binary searching, and comparisons compare the 4-byte prefix first in a single 32-bit register check, avoiding heap pointer dereferencing on >90% of non-equal comparisons.
4. **Smart Sub-slicing & Automatic Inline Downgrade**:
   Sub-slicing does not copy memory. Furthermore, if a sub-slice is <= 20 bytes, it **automatically downgrades to an inlined short representation** and decrements the parent refcount, completely avoiding memory retention leaks.

#### Original

This code is heavily inspired by [byteview](https://crates.io/crates/byteview) by [fjall-rs](https://github.com/fjall-rs), licensed under the Apache License 2.0 and MIT licenses.
