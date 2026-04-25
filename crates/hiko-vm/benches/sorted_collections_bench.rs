use criterion::{BatchSize, BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use smallvec::SmallVec;
use sortedvec::sortedvec;

#[derive(Clone, Debug)]
struct BenchEntry {
    key: u64,
    payload: u64,
}

sortedvec! {
    struct SortedBenchEntries {
        fn derive_key(entry: &BenchEntry) -> u64 { entry.key }
    }
}

fn entries(count: usize) -> Vec<BenchEntry> {
    (0..count as u64)
        .map(|payload| BenchEntry {
            key: payload
                .wrapping_mul(6_364_136_223_846_793_005)
                .rotate_left(17),
            payload,
        })
        .collect()
}

fn checksum(entries: &[BenchEntry]) -> u64 {
    entries.iter().fold(0, |acc, entry| {
        acc.wrapping_add(entry.key ^ entry.payload.rotate_left(7))
    })
}

fn bench_sorted_collection_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("sorted_collection_build");

    for count in [4, 8, 16, 32, 64, 256] {
        let input = entries(count);

        group.bench_with_input(
            BenchmarkId::new("vec_collect_sort", count),
            &input,
            |b, input| {
                b.iter_batched(
                    || input.clone(),
                    |mut entries| {
                        entries.sort_unstable_by_key(|entry| entry.key);
                        black_box(checksum(&entries))
                    },
                    BatchSize::SmallInput,
                )
            },
        );

        group.bench_with_input(
            BenchmarkId::new("smallvec_collect_sort", count),
            &input,
            |b, input| {
                b.iter_batched(
                    || input.clone(),
                    |entries| {
                        let mut entries: SmallVec<[BenchEntry; 16]> = entries.into_iter().collect();
                        entries.sort_unstable_by_key(|entry| entry.key);
                        black_box(checksum(&entries))
                    },
                    BatchSize::SmallInput,
                )
            },
        );

        group.bench_with_input(
            BenchmarkId::new("sortedvec_from_vec", count),
            &input,
            |b, input| {
                b.iter_batched(
                    || input.clone(),
                    |entries| {
                        let entries = SortedBenchEntries::from(entries);
                        let entries: &Vec<BenchEntry> = entries.as_ref();
                        black_box(checksum(entries))
                    },
                    BatchSize::SmallInput,
                )
            },
        );

        group.bench_with_input(
            BenchmarkId::new("sortedvec_incremental_insert", count),
            &input,
            |b, input| {
                b.iter_batched(
                    || input.clone(),
                    |entries| {
                        let mut sorted = SortedBenchEntries::default();
                        for entry in entries {
                            sorted.insert(entry);
                        }
                        let entries: &Vec<BenchEntry> = sorted.as_ref();
                        black_box(checksum(entries))
                    },
                    BatchSize::SmallInput,
                )
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_sorted_collection_build);
criterion_main!(benches);
