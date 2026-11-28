# Changelog

## 0.2.1 (2026-11-28)

### Features

- **`--no-header`** for CSV/TSV files and worksheets without a header row
  (#16): the first row is data, rows count from 0, and columns are
  identified by `raw:columnNumber` only, without labels. Parquet always
  uses its schema's names.

### Performance

- **gzip output takes about half the CPU** (#18): `.gz` is compressed with
  zlib-ng instead of zlib, still at level 6 and still standard gzip, and
  the files come out about 4% smaller. On one vCPU, a 500k-row CSV to
  `.nq.gz` takes 11.4 s instead of 19.3 s.

### Distribution

- **crates.io:** releases are published there through trusted publishing (#20).
- **cargo-binstall:** each release carries `triplify-<target>.zip` for
  Linux (x86-64, ARM64), macOS on Apple silicon and Windows (x86-64, ARM64),
  so `cargo binstall triplify` downloads instead of compiling (#20, #22).
- **Homebrew:** each release carries a `triplify.rb` formula for
  `brew install ekgf/tap/triplify` (#23).

## 0.2.0 (2026-11-27)

### Features

- **Amazon S3 inputs and outputs** (#12). Arguments and `-o` accept
  `s3://bucket/key` objects and `s3://bucket/prefix/` listings, using the
  standard AWS credential chain; `AWS_ENDPOINT_URL` selects an S3-compatible
  store. CSV/TSV stream from S3, Parquet uses ranged GETs, workbooks are
  buffered in memory, and nothing touches local disk. Reads pin the version
  and use `If-Match`, so an object changed mid-run fails the run. Each
  object's metadata is captured as `raw:S3Object`: version, ETag, content
  headers, storage class, S3 checksums, encryption, Object Lock, tags and
  user metadata. Output is a streaming multipart upload, published only if
  every source succeeded.

### Performance

- **2.4–8× faster** on large inputs, with byte-identical output (#11).
  Stdout is buffered, IRIs are shared between quads, N-Quads/N-Triples are
  encoded on all cores, and `.gz` output is compressed in parallel into a
  single standard gzip member. A benchmark harness (`scripts/bench.sh`) and
  a Performance page were added.

### Platforms

- **No more Intel macOS wheel** (#13). Wheels cover Linux and Windows on
  x86-64 and ARM64, and macOS on Apple silicon. Intel Macs can build from a
  checkout.

## 0.1.0 (2026-11-18)

The first release: CSV, TSV, XLSX/XLSM/XLSB, XLS, ODS and Parquet captured
as EKGF raw RDF, with DCAT, SPDX, git and PROV metadata, formulas,
`--profile` column profiles, deterministic `urn:uuid` IRIs, and N-Quads,
N-Triples or TriG output, optionally gzipped or RDFC-1.0 canonical.
