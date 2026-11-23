---
title: "Usage"
description: "Command-line reference for triplify: input paths, output formats chosen by file name, gzip compression, RDFC-1.0 canonical output and column profiles."
permalink: /usage/
nav_order: 3
grapharch:
  subject: urn:triplify:website:page:usage
---

# Usage

One command, a list of paths and an optional output file.

## Command-line reference

This is the output of `triplify --help`:

```text
Triplify turns any tabular data dump (CSV, XLSX, Parquet, ...) into raw RDF using the EKGF Raw Data Ontology, with DCAT, PROV and git provenance, without any mapping

Usage: triplify [OPTIONS] [PATHS]...

Arguments:
  [PATHS]...  Files, directories (walked, honouring .gitignore), S3 objects (s3://bucket/key) or S3 prefixes (s3://bucket/prefix/) [default: .]

Options:
  -o, --output <OUTPUT>  Output file or S3 object: .nq, .nt (graphs merged) or .trig, optionally .gz. Defaults to N-Quads on stdout
      --canonical        Emit RDFC-1.0 canonical N-Quads/N-Triples (buffers the dataset in memory)
      --profile          Profile every column: exact value/null/distinct counts, most frequent value, an inferred datatype with its conforming count, and min/max/mean/std/percentiles. Costs memory per column and time
  -v, --verbose...       Increase logging verbosity
  -q, --quiet...         Decrease logging verbosity
  -h, --help             Print help
  -V, --version          Print version
```

## Inputs

Give any mix of files and directories; with none, triplify reads the current
directory. Directories are walked in parallel. The walk honours `.gitignore`,
`.git/info/exclude` and your global Git excludes, and skips hidden files and
directories.

Files are selected by extension, ignoring case:

| Extension | Read as |
| --- | --- |
| `.csv` | comma-separated text |
| `.tsv`, `.tab` | tab-separated text |
| `.xlsx`, `.xlsm`, `.xlsb` | Excel workbook |
| `.xls` | Excel 97-2003 workbook |
| `.ods` | OpenDocument spreadsheet |
| `.parquet` | Apache Parquet |

Other files are ignored, and so are Excel lock files such as `~$Book.xlsx`.
XLSX worksheets are read cell by cell, as a stream. Parquet files are read in
batches of record rows, and every compression codec is supported (Snappy, Zstd,
LZ4, Gzip, Brotli).

## Output

The output file name selects the syntax. Add `.gz` to compress it with gzip.

| Output | Meaning |
| --- | --- |
| *(none)* | N-Quads on stdout |
| `*.nq`, `*.nq.gz` | N-Quads, one named graph per source plus one for the run |
| `*.nt`, `*.nt.gz` | N-Triples, the union of all graphs |
| `*.trig`, `*.trig.gz` | TriG |

Any other extension is an error. Output always streams, except with
`--canonical`.

## Amazon S3

Inputs and the output can be S3 objects. An input ending in `/` is a prefix:
every object below it with a supported extension is triplified.

```sh
triplify s3://dumps/crm/2026-11-23/ -o s3://lake/raw/crm-2026-11-23.nq.gz
triplify s3://dumps/erp/orders.parquet ./local/rates.xlsx -o raw.nq
```

- **Credentials, region and endpoint** come from the standard AWS chain:
  - environment variables (`AWS_ACCESS_KEY_ID`, `AWS_REGION`, ...);
  - `~/.aws` profiles, including SSO;
  - ECS task roles, Lambda, and EC2 instance metadata.

  `AWS_ENDPOINT_URL` selects an S3-compatible store, which is then addressed
  path-style.
- **Streaming:**
  - CSV and TSV objects stream straight from S3;
  - Parquet is read with ranged GETs, which fetch the footer and then only
    the column chunks it needs;
  - workbooks (XLSX, XLS, ODS) are buffered in memory, because a workbook is a
    zip archive with its index at the end.

  Nothing is written to local disk.
- **One read when S3 has the hash.** IRIs are minted from the SHA-256 of the
  content. If the object has a full-object SHA-256 checksum, for example
  because it was uploaded with `--checksum-algorithm SHA256`, triplify uses
  it and reads the object once. Otherwise it streams the object once to hash
  it and a second time to read it.
- **Consistent reads.** Both reads use `If-Match` on the ETag, and the
  version id when the bucket is versioned, so an object that changes
  in between fails the run instead of mixing two versions. The bytes are
  re-hashed while they are read and must match.
- **Output** to `s3://…` is a streaming multipart upload, including parallel
  gzip. The object is published only if the whole run succeeded. If any
  source fails, the upload is aborted: no partial object, and no S3 event
  for downstream steps.
- **Metadata** about every object is captured as well: version, ETag,
  checksums, encryption, Object Lock, tags and user metadata. See
  [Output model](../output-model/#s3-objects). Tags need
  `s3:GetObjectTagging`; without that permission a warning is logged and the
  tags are left out.

IAM permissions:
- reading needs `s3:GetObject` and `s3:ListBucket`, plus
  `s3:GetObjectVersion`, `s3:GetObjectTagging` and `s3:GetObjectVersionTagging`
  for the full metadata;
- writing needs `s3:PutObject` and `s3:AbortMultipartUpload`.

## Canonical output

`--canonical` writes [RDFC-1.0](https://www.w3.org/TR/rdf-canon/) canonical
N-Quads or N-Triples, so that two runs over the same input can be compared
byte for byte. This mode buffers the whole dataset in memory. It does not
support TriG.

```sh
triplify ./dumps --canonical -o raw.nq
```

## Column profiles

`--profile` adds data-quality measures to every column: counts, the most
frequent value, an inferred datatype with its evidence, and numeric statistics.
They are computed in the same streaming pass, which is far cheaper than
computing them in SPARQL afterwards. They do cost memory for each column, so
they are opt-in. See [Output model](../output-model/#column-profiles) for the
measures and the inference policy.

```sh
triplify ./dumps --profile -o raw.nq.gz
```

## Logging and exit status

Logs go to stderr. By default only warnings are shown; `-v` adds progress per
file, `-vv` and `-vvv` add debugging detail, `-q` shows only errors and `-qq`
nothing. The `RUST_LOG` environment variable overrides these flags.

A file that cannot be read is reported as a warning, and the other files are
still triplified. The command then exits with a nonzero status that says how
many sources failed.

## Examples

```sh
# Every table under ./dumps, as compressed N-Quads
triplify ./dumps -o raw.nq.gz

# Two specific files, all graphs merged into N-Triples
triplify orders.csv rates.xlsx -o raw.nt

# The current directory, to stdout
triplify > raw.nq
```
