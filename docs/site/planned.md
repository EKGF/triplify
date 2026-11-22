---
title: "Planned features"
description: "Features planned for triplify: more source formats, nested data, formula translation and typed value copies."
permalink: /planned/
nav_order: 7
grapharch:
  subject: urn:triplify:website:page:planned
---

# Planned features

These are plans, not current features.

## Source formats

- **PostgreSQL `pg_dump`.** Plain SQL dumps, with `COPY … FROM stdin` data and
  column types taken from `CREATE TABLE`. One file then captures a whole
  database.
- **MySQL `mysqldump`** `INSERT` statements.
- **JSON Lines**, for NoSQL and API dumps. This first needs a raw model for
  nested records.
- **Avro and ORC.**
- **Direct JDBC/ODBC table dumps.** Full table scans only, no views.
- **Partitioned datasets.** A directory of Parquet or CSV parts captured as one
  view.

## Modelling

- **Nested values** (lists, structs, maps) as raw structure instead of text.
- **Formula translation.** Compile column formulas into SHACL rules and
  constraints.
- **Typed value copies.** Optionally, a typed copy of each value that conforms
  to its column's inferred datatype, next to the source literal.
