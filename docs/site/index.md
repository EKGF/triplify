---
title: "Triplify"
description: "Turn CSV files and spreadsheets into RDF, preserving their original structure and values."
permalink: /
nav_order: 1
grapharch:
  subject: urn:triplify:website:page:home
  hero: landing
  panel_sections: [what-it-does]
  feature_sections: [principles]
  card_icons:
    capture-never-map: {name: layers, tone: primary}
    content-addressed-identity: {name: file-box, tone: accent}
    provenance-included: {name: git-branch, tone: primary}
    built-for-big-dumps: {name: database, tone: accent}
---

EKGF tool

# Triplify

Turn CSV files and spreadsheets into RDF, preserving their original structure and values.

Capture your source data without defining a mapping first. Use SPARQL and SHACL
afterward to transform, connect and validate it.

[Install](install.md) [Usage](usage.md) [Source on GitHub](https://github.com/EKGF/triplify)

## What it does

```sh
triplify ./dumps -o raw.nq.gz
```

Triplify walks the files and directories you give it, honouring
`.gitignore`, and captures every table it can read as RDF. It reads local files and Amazon S3 objects or prefixes: CSV, TSV,
XLSX, XLSM, XLSB, XLS, ODS and Parquet files. The output follows the
[EKGF Raw Data Ontology](https://ekgf.github.io/ekg-ontologies/spec/main/ekg-raw/index.html):
sources, views, columns, rows and values, exactly as they physically are.

Each source gets its own named graph, along with its metadata: a DCAT
distribution, an SPDX SHA-256 checksum, its Git origin, and a PROV activity for
the run. See [Output model](output-model.md) for the details.

## Principles

### Capture, never map

Readers report physical structure and the source's own value types. Triplify
never infers datatypes from text, never picks key columns and never renames
anything. Mapping happens later, in the graph, with SPARQL and SHACL.

### Content-addressed identity

Every IRI is a `urn:uuid:` that anyone can recompute from the source bytes and
a position. Identical files found twice are triplified once.

### Provenance included

Every source records its media type, byte size, location, checksum and Git
blob. Inside a Git work tree it also records the repository, commit and path,
and whether the file differs from `HEAD`.

### Built for big dumps

Files are processed concurrently, one per core, into a single writer. Output
streams, optionally gzip-compressed, unless you ask for canonical RDF.

## Learn more

- [Why capture first](why.md) explains the approach and how it differs from
  mapping languages.
- [Usage](usage.md) is the command-line reference.
- [Contributing](contributing.md) covers the architecture, conventions and
  releases.
