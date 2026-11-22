---
title: "Performance"
description: "Measured throughput and memory of triplify on large CSV, XLSX and Parquet inputs, and how to reproduce the benchmark."
permalink: /performance/
nav_order: 6
grapharch:
  subject: urn:triplify:website:page:performance
---

# Performance

Triplify streams: memory stays flat however large the input is. The most
expensive step, encoding the output, runs on every core, even when the input
is a single file.

## Measurements

These numbers are for triplify 0.1, measured on an Apple M3 Max with 16 cores,
writing to stdout unless the case says otherwise.

| Input | Output | Time | Peak memory | Throughput |
| --- | --- | --- | --- | --- |
| CSV, 5M rows × 10 columns (442 MB) | N-Quads | 11.5 s | 34 MiB | 18.7M quads/s |
| same | gzipped N-Quads (`.nq.gz`) | 26.5 s | 390 MiB | 8.1M quads/s |
| same, `--profile` | N-Quads | 20.2 s | 128 MiB | 10.6M quads/s |
| XLSX, 1M rows × 8 columns with formulas (42 MB) | N-Quads | 5.9 s | 190 MiB | 5.6M quads/s |
| Parquet, 10M rows × 8 typed columns, Zstd (216 MB) | N-Quads | 20.8 s | 92 MiB | 16.8M quads/s |
| same, `--profile` | N-Quads | 30.8 s | 302 MiB | 11.4M quads/s |
| 2,000 small CSV files in a git repository | N-Quads | 0.4 s | 52 MiB | 3.8M quads/s |

Each CSV row becomes about 43 quads: one `raw:Row`, plus four quads for each
`raw:Value`. The 5M-row CSV is therefore 215M quads, about 34 GB of N-Quads.

## How it is fast

- **One pass, streaming.** Each source is read once. Quads flow through
  bounded channels to the writer, so memory does not grow with input size.
- **Parallel encoding.** Producers hand structured batches to a pool of
  encoder threads. N-Quads is a set of lines, so batches can be encoded and
  written in any order, and one large file uses all cores. The encoder writes
  bytes directly and produces exactly the bytes of oxrdf's N-Quads form.
- **Parallel gzip.** `.gz` output is compressed on all cores into a single
  standard gzip member, as `pigz` does, so any gzip reader can decompress it.
- **Cheap identifiers.** Row and column IRIs are minted once and shared
  between quads. A cell's UUIDv8 hashes only its own digits, starting from a
  precomputed SHA-256 state for the view.
- **Typed fast paths.** XLSX is read cell by cell, and Parquet in Arrow record
  batches.

TriG and `--canonical` output are serialized by a single writer. Canonical
output also holds the whole dataset in memory, so for large inputs use
N-Quads or N-Triples.

## Reproduce

```sh
scripts/bench.sh            # full size; generates the inputs once
scripts/bench.sh 0.1        # a tenth of the size
scripts/bench.sh 1.0 csv parquet
```

`examples/bench_data.rs` generates deterministic inputs under
`target/bench-data/<scale>`. The script reports wall time, peak resident
memory and quads per second for each case.
