//! Deterministic `urn:uuid:` identifiers.
//!
//! Every resource has a canonical [`ResourceName`]. Its IRI is the
//! name-based **UUIDv8 with SHA-256** of RFC 9562 (§6.5, Appendix B.2):
//!
//! ```text
//! digest = SHA-256( NAMESPACE.bytes ‖ UTF-8(name) )
//! uuid   = digest[0..16], version nibble := 8, variant bits := 0b10
//! iri    = "urn:uuid:" ‖ lowercase hyphenated uuid
//! ```
//!
//! `NAMESPACE` is itself derived with the same function:
//! `uuidv8_sha256(NameSpace_URL, "https://ekgf.org/ontology/raw#")`.
//!
//! Name grammar (ASCII, decimal integers without leading zeros, `<sha256>` =
//! 64 lowercase hex digits of the SHA-256 of the source bytes):
//!
//! ```text
//! source/<sha256>
//! source/<sha256>/graph
//! source/<sha256>/checksum
//! source/<sha256>/view/<v>                      v ≥ 1
//! source/<sha256>/view/<v>/column/<c>           c ≥ 1
//! source/<sha256>/view/<v>/row/<r>              r ≥ 0
//! source/<sha256>/view/<v>/row/<r>/column/<c>
//! agent/triplify/<semver>
//! ```
//!
//! Anyone can recompute any IRI from the source bytes and a position. Runs
//! are events, not content, so they get a time-ordered UUIDv7 instead.

use {
    oxrdf::NamedNode,
    sha2::{Digest, Sha256},
    std::{fmt, sync::LazyLock},
    uuid::Uuid,
};

/// Name-based UUIDv8 using SHA-256, per RFC 9562 Appendix B.2.
pub fn uuid_v8_sha256(namespace: Uuid, name: &str) -> Uuid {
    uuid_from(
        Sha256::new()
            .chain_update(namespace.as_bytes())
            .chain_update(name.as_bytes()),
    )
}

/// Finish a SHA-256 over `namespace ‖ name` into a UUIDv8.
fn uuid_from(hasher: Sha256) -> Uuid {
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::new_v8(bytes)
}

/// A SHA-256 state that has already absorbed `NAMESPACE ‖ prefix`, so names
/// sharing the prefix only hash their suffix.
#[derive(Debug, Clone)]
struct NamePrefix(Sha256);

impl NamePrefix {
    fn new(prefix: &str) -> Self {
        Self(
            Sha256::new()
                .chain_update(NAMESPACE.as_bytes())
                .chain_update(prefix.as_bytes()),
        )
    }

    /// IRI of `prefix ‖ n` (decimal).
    fn iri_with(&self, n: u64) -> NamedNode {
        let mut digits = itoa::Buffer::new();
        urn(uuid_from(
            self.0.clone().chain_update(digits.format(n)),
        ))
    }

    /// IRI of `prefix ‖ n ‖ middle ‖ m` (decimals).
    fn iri_with2(&self, n: u64, middle: &str, m: u64) -> NamedNode {
        let mut a = itoa::Buffer::new();
        let mut b = itoa::Buffer::new();
        urn(uuid_from(
            self.0
                .clone()
                .chain_update(a.format(n))
                .chain_update(middle)
                .chain_update(b.format(m)),
        ))
    }
}

/// Triplify's UUID namespace.
pub static NAMESPACE: LazyLock<Uuid> = LazyLock::new(|| {
    uuid_v8_sha256(Uuid::NAMESPACE_URL, crate::vocab::raw::NS)
});

/// Canonical name of a resource; see the module docs for the grammar.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceName(String);

impl ResourceName {
    pub fn as_str(&self) -> &str { &self.0 }

    fn child(&self, segment: impl fmt::Display) -> Self {
        Self(format!("{}/{segment}", self.0))
    }

    pub fn uuid(&self) -> Uuid { uuid_v8_sha256(*NAMESPACE, &self.0) }

    pub fn iri(&self) -> NamedNode { urn(self.uuid()) }
}

fn urn(uuid: Uuid) -> NamedNode {
    let mut buf = Uuid::encode_buffer();
    let mut iri = String::with_capacity(45);
    iri.push_str("urn:uuid:");
    iri.push_str(uuid.hyphenated().encode_lower(&mut buf));
    NamedNode::new_unchecked(iri)
}

/// SHA-256 of a source's bytes; the identity of a [`crate::source::Source`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentDigest([u8; 32]);

impl ContentDigest {
    pub fn new(bytes: [u8; 32]) -> Self { Self(bytes) }

    pub fn hex(&self) -> String { hex::encode(self.0) }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex())
    }
}

/// 1-based position of a view (worksheet) within a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewNumber(pub u32);

/// 1-based physical column index, as in `raw:columnNumber`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnNumber(pub u32);

/// 0-based data-row sequence number, as in `raw:viewRowNumber`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowNumber(pub u64);

/// Mints the IRIs of one source and everything inside it.
#[derive(Debug, Clone)]
pub struct SourceIris {
    name: ResourceName,
}

impl SourceIris {
    pub fn new(digest: ContentDigest) -> Self {
        Self { name: ResourceName(format!("source/{digest}")) }
    }

    pub fn source(&self) -> NamedNode { self.name.iri() }

    /// Named graph holding everything triplified from this source.
    pub fn graph(&self) -> NamedNode { self.name.child("graph").iri() }

    pub fn checksum(&self) -> NamedNode { self.name.child("checksum").iri() }

    /// An S3 tag (`kind` = `tag`) or user metadata entry (`metadata`),
    /// named by its key: `source/<sha256>/s3/<kind>/<percent-encoded key>`.
    pub fn s3_entry(&self, kind: &str, key: &str) -> NamedNode {
        let key: String =
            url::form_urlencoded::byte_serialize(key.as_bytes()).collect();
        self.name.child("s3").child(kind).child(key).iri()
    }

    pub fn view(&self, v: ViewNumber) -> ViewIris {
        ViewIris::new(self.name.child("view").child(v.0))
    }
}

/// Mints the IRIs inside one view. Row, column and value names share the
/// view's prefix, which is hashed once.
#[derive(Debug, Clone)]
pub struct ViewIris {
    name:   ResourceName,
    rows:   NamePrefix,
    column: NamePrefix,
}

impl ViewIris {
    fn new(name: ResourceName) -> Self {
        let rows = NamePrefix::new(&format!("{}/row/", name.as_str()));
        let column = NamePrefix::new(&format!("{}/column/", name.as_str()));
        Self { name, rows, column }
    }

    pub fn view(&self) -> NamedNode { self.name.iri() }

    #[cfg(test)]
    fn row_name(&self, r: RowNumber) -> ResourceName {
        self.name.child("row").child(r.0)
    }

    pub fn column(&self, c: ColumnNumber) -> NamedNode {
        self.column.iri_with(u64::from(c.0))
    }

    pub fn row(&self, r: RowNumber) -> NamedNode { self.rows.iri_with(r.0) }

    pub fn value(&self, r: RowNumber, c: ColumnNumber) -> NamedNode {
        self.rows.iri_with2(r.0, "/column/", u64::from(c.0))
    }
}

/// One run of triplify (a `prov:Activity`), identified by a UUIDv7.
#[derive(Debug, Clone)]
pub struct RunIri {
    node:  NamedNode,
    graph: NamedNode,
}

impl RunIri {
    pub fn new() -> Self {
        let id = Uuid::now_v7();
        Self {
            node:  urn(id),
            graph: urn(uuid_v8_sha256(id, "graph")),
        }
    }

    pub fn node(&self) -> &NamedNode { &self.node }

    pub fn graph(&self) -> &NamedNode { &self.graph }

    pub fn agent() -> NamedNode {
        ResourceName(format!(
            "agent/triplify/{}",
            env!("CARGO_PKG_VERSION")
        ))
        .iri()
    }
}

impl Default for RunIri {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc9562_appendix_b2_test_vector() {
        assert_eq!(
            uuid_v8_sha256(Uuid::NAMESPACE_DNS, "www.example.com").to_string(),
            "5c146b14-3c52-8afd-938a-375d0df1fbf6"
        );
    }

    #[test]
    fn names_follow_the_grammar() {
        let s = SourceIris::new(ContentDigest::new([0xab; 32]));
        let v = s.view(ViewNumber(2));
        let name = v.row_name(RowNumber(7)).child("column").child(3);
        assert_eq!(
            name.as_str(),
            format!("source/{}/view/2/row/7/column/3", "ab".repeat(32))
        );
        assert_eq!(v.value(RowNumber(7), ColumnNumber(3)), name.iri());
        // The prefix-hashed fast paths mint exactly the name-based IRIs.
        assert_eq!(
            v.row(RowNumber(7)),
            v.row_name(RowNumber(7)).iri()
        );
        assert_eq!(
            v.column(ColumnNumber(12)),
            v.name.child("column").child(12).iri()
        );
        assert_eq!(
            v.value(RowNumber(0), ColumnNumber(1)),
            v.row_name(RowNumber(0)).child("column").child(1).iri()
        );
        assert!(name.iri().as_str().starts_with("urn:uuid:"));
        assert_eq!(name.uuid().get_version_num(), 8);
    }
}
