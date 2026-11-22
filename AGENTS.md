# Triplify contributor guidance

## Principle

Capture, never map. Readers report physical structure (views, columns, rows,
cells) and the source's own value types. Never infer datatypes from text,
never pick key columns, and never rename anything. Mapping happens later, in
SPARQL and SHACL.

## Architecture

`walk` (parallel `ignore` walker) → `source` (one streaming pass for SHA-256 and
the git blob, plus `git` origin) → `reader::{csv, spreadsheet}` → `reader::TableBuilder`
→ `emit` (the only place that knows the raw ontology) → bounded channel of
quad batches → `writer` (a single task: N-Quads, N-Triples or TriG, optional gzip,
optional RDFC-1.0).

A new input format is a new `SourceKind` variant plus a reader that feeds a
`TableBuilder`.

## Conventions

- Rust 2024, pinned nightly (`rust-toolchain.toml`); rustfmt and clippy config
  copied from GraphArch.
- Use strict newtypes (`BaseIri`, `ContentDigest`, `ViewNumber`, `ColumnNumber`,
  `RowNumber`) rather than bare strings or integers.
- Every emitted vocabulary term is a constant in `src/vocab.rs`. A new `raw:`
  term must first be added, with SHACL shapes and tests, to `ekgf-raw.ttl` in
  the `EKGF/ekg-ontologies` repository. That repository owns the raw ontology.
- Conventional commits with one scope from: `cli`, `reader`, `emit`,
  `writer`, `walk`, `source`, `git`, `iri`, `ontology`, `docs`, `ci`, `deps`,
  `release`.
- Fail fast, with no silent fallbacks. Write a regression test first for bugs.
- Never `set -e` in shell scripts.

## Verification

```sh
cargo fmt --all --check
cargo clippy --lib -- -D warnings
cargo clippy --bins -- -D warnings
cargo clippy --tests -- -D warnings
cargo test --lib
cargo test --tests
```

Performance: run `scripts/bench.sh 0.1` before and after changes to the
emit, encode or writer path. Output must stay byte-identical: compare the
sorted N-Quads of the old and new builds, excluding the run graph.

SHACL conformance against the upstream ontology:

```sh
cargo run -q -- <dir> -o /tmp/raw.nt
pyshacl -s ../ekg-ontologies/ekgf-raw.ttl -e ../ekg-ontologies/ekgf-raw.ttl -i rdfs /tmp/raw.nt
```
