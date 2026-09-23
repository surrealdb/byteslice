//! A thin, immutable, zero-copy 24-byte slice with Small String Optimization
//! and prefix-accelerated comparisons (German String design).

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod byteslice;

pub use byteslice::ByteSlice;
