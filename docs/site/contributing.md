---
title: "Contributing"
description: "How triplify is built: its architecture, coding conventions, verification commands, commit conventions and release process."
permalink: /contributing/
nav_order: 8
grapharch:
  subject: urn:triplify:website:page:contributing
---

# Contributing

Triplify is open source under the MIT license. Issues and pull requests are
welcome on [GitHub](https://github.com/EKGF/triplify).

## Build from source

The repository pins a nightly Rust toolchain in `rust-toolchain.toml`, which
`rustup` selects automatically. Install the latest `main` with:

```sh
cargo install --locked --git https://github.com/EKGF/triplify
```

Or build a checkout:

```sh
git clone https://github.com/EKGF/triplify
cd triplify
cargo build --release --locked
./target/release/triplify --version
```

### Build a Python wheel

Triplify can also be packaged as a [maturin](https://www.maturin.rs/) `bin`
wheel. The wheel contains the native executable, so it needs no Python
dependencies and downloads nothing at runtime. Build one from a checkout and
install it into an isolated tool environment:

```sh
uvx maturin build --release --locked --out dist
uv tool install dist/triplify-*.whl
```

## Principle

Capture, never map. Readers report physical structure (views, columns, rows,
cells) and the source's own value types. Never infer datatypes from text, never
pick key columns, and never rename anything.

## Architecture

```text
walk → source → reader::{csv, spreadsheet} → reader::TableBuilder → emit → writer
```

- `walk`: a parallel, `.gitignore`-aware walker that selects files by
  extension.
- `source`: one streaming pass for SHA-256 and the Git blob id, plus the Git
  origin.
- `reader`: format readers that feed cells, in physical order, into a
  `TableBuilder`.
- `emit`: the only place that knows the raw ontology. It sends quad batches
  through a bounded channel.
- `writer`: a single task that writes N-Quads, N-Triples or TriG, optionally
  gzip-compressed, optionally RDFC-1.0 canonical.

A new input format is a new `SourceKind` variant plus a reader that feeds a
`TableBuilder`. Every emitted vocabulary term is a constant in `src/vocab.rs`.

## Conventions

- Rust 2024 on the nightly toolchain pinned in `rust-toolchain.toml`.
- Strict newtypes (`BaseIri`, `ContentDigest`, `ViewNumber`, `ColumnNumber`,
  `RowNumber`) rather than bare strings or integers.
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

Check SHACL conformance against the upstream ontology, from a checkout of
[ekg-ontologies](https://github.com/EKGF/ekg-ontologies) next to this one:

```sh
cargo run -q -- <dir> -o /tmp/raw.nt
pyshacl -s ../ekg-ontologies/ekgf-raw.ttl -e ../ekg-ontologies/ekgf-raw.ttl -i rdfs /tmp/raw.nt
```

## Commits

Commits follow [Conventional Commits](https://www.conventionalcommits.org/)
with exactly one scope: `cli`, `reader`, `emit`, `writer`, `walk`, `source`,
`git`, `iri`, `ontology`, `docs`, `ci`, `deps` or `release`.
[Cocogitto](https://docs.cocogitto.io/) checks them in CI.

## Releases

Releases are cut with `cog bump` on `main`, which sets the crate version and
tags `v<version>`. A `v*` tag builds the Python wheels for every supported
platform and the source distribution, and publishes them to PyPI as the
project `triplify` through a PyPI trusted publisher (the `ci.yml` workflow,
environment `pypi`). The crate is published to crates.io under the same name
with `cargo publish`.

## Documentation website

This website is built by [GraphArch](https://github.com/EKGF/grapharch) from
the Markdown pages in `docs/site/` and the RDF definitions in
`docs/grapharch.ttl`, and is published to GitHub Pages on every push to `main`.
