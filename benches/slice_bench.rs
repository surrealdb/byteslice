use bytes::Bytes;
use byteslice::ByteSlice;
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_cloning(c: &mut Criterion) {
    let mut group = c.benchmark_group("clone_small_16b");
    let raw_small = b"key_000000000001";
    let bs_small = ByteSlice::from(&raw_small[..]);
    let bytes_small = Bytes::copy_from_slice(&raw_small[..]);
    let vec_small = raw_small.to_vec();

    group.bench_function("ByteSlice", |b| {
        b.iter(|| black_box(bs_small.clone()));
    });
    group.bench_function("bytes::Bytes", |b| {
        b.iter(|| black_box(bytes_small.clone()));
    });
    group.bench_function("Vec<u8>", |b| {
        b.iter(|| black_box(vec_small.clone()));
    });
    group.finish();

    let mut group = c.benchmark_group("clone_long_64b");
    let raw_long = b"key_000000000001_very_long_string_payload_exceeding_inline_capacity_here!";
    let bs_long = ByteSlice::from(&raw_long[..]);
    let bytes_long = Bytes::copy_from_slice(&raw_long[..]);
    let vec_long = raw_long.to_vec();

    group.bench_function("ByteSlice", |b| {
        b.iter(|| black_box(bs_long.clone()));
    });
    group.bench_function("bytes::Bytes", |b| {
        b.iter(|| black_box(bytes_long.clone()));
    });
    group.bench_function("Vec<u8>", |b| {
        b.iter(|| black_box(vec_long.clone()));
    });
    group.finish();
}

fn bench_slicing_and_drop(c: &mut Criterion) {
    let raw_long = b"key_000000000001_very_long_string_payload_exceeding_inline_capacity_here!";
    let bs_long = ByteSlice::from(&raw_long[..]);

    let mut group = c.benchmark_group("slice_long_64b");
    group.bench_function("ByteSlice", |b| {
        b.iter(|| black_box(black_box(&bs_long).slice(4..60)));
    });
    group.finish();

    let mut group = c.benchmark_group("clone_and_drop_long_64b");
    group.bench_function("ByteSlice", |b| {
        b.iter(|| drop(black_box(black_box(&bs_long).clone())));
    });
    group.finish();
}

fn bench_borrowed_lookup(c: &mut Criterion) {
    // The `with_borrowed` pattern: compare a borrowed view against a stored key
    // without allocating.
    let raw_long = b"key_000000000001_very_long_string_payload_exceeding_inline_capacity_here!";
    let stored = ByteSlice::from(&raw_long[..]);
    let probe = raw_long.to_vec();

    let mut group = c.benchmark_group("with_borrowed_eq_long_64b");
    group.bench_function("ByteSlice", |b| {
        b.iter(|| ByteSlice::with_borrowed(black_box(&probe), |k| black_box(*k == stored)));
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_cloning,
    bench_slicing_and_drop,
    bench_borrowed_lookup
);
criterion_main!(benches);
