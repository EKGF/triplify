# Changelog

## Unreleased

### Features

- `--no-header`: the first row of CSV/TSV files and worksheets is data and
  columns get no labels. Parquet always uses its schema's names.

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
