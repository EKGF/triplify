//! A triplifiable input file and its metadata (DCAT, SPDX checksum, git).

use {
    crate::{
        emit::{GraphEmitter, Node},
        git::GitOrigin,
        iri::{ContentDigest, SourceIris},
        location::Location,
        s3::{S3ObjectMeta, S3Store, S3Url},
        vocab::{dcat, dcterms, prov, raw, rdf, spdx, xsd},
    },
    anyhow::Context,
    oxrdf::{Literal, NamedNode, NamedNodeRef},
    sha1::Digest as _,
    std::{
        path::{Path, PathBuf},
        sync::Arc,
    },
    tokio::io::{AsyncRead, AsyncReadExt},
};

/// Formats triplify can read. Add a variant to support a new format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Csv,
    Tsv,
    Xlsx,
    Xls,
    Ods,
    Parquet,
}

impl SourceKind {
    pub const EXTENSIONS: &[&str] = &[
        "csv", "tsv", "tab", "xlsx", "xlsm", "xlsb", "xls", "ods", "parquet",
    ];

    pub fn from_path(path: &Path) -> Option<Self> {
        Self::from_name(path.file_name()?.to_str()?)
    }

    /// Detect the format from a file or object name's extension.
    pub fn from_name(name: &str) -> Option<Self> {
        let (_, ext) = name.rsplit_once('.')?;
        let ext = ext.to_ascii_lowercase();
        Some(match ext.as_str() {
            "csv" => Self::Csv,
            "tsv" | "tab" => Self::Tsv,
            "xlsx" | "xlsm" | "xlsb" => Self::Xlsx,
            "xls" => Self::Xls,
            "ods" => Self::Ods,
            "parquet" => Self::Parquet,
            _ => return None,
        })
    }

    pub fn media_type(self) -> &'static str {
        match self {
            Self::Csv => "text/csv",
            Self::Tsv => "text/tab-separated-values",
            Self::Xlsx => {
                "application/vnd.openxmlformats-officedocument.spreadsheetml.\
                 sheet"
            },
            Self::Xls => "application/vnd.ms-excel",
            Self::Ods => "application/vnd.oasis.opendocument.spreadsheet",
            Self::Parquet => "application/vnd.apache.parquet",
        }
    }

    fn class(self) -> oxrdf::NamedNodeRef<'static> {
        match self {
            Self::Csv => raw::CSV_FILE,
            Self::Tsv => raw::TSV_FILE,
            Self::Xlsx => raw::XLSX_FILE,
            Self::Parquet => raw::PARQUET_FILE,
            Self::Xls | Self::Ods => raw::FILE,
        }
    }
}

/// Where a source's bytes come from.
#[derive(Debug, Clone)]
pub enum Origin {
    Local {
        path:     PathBuf,
        modified: Option<chrono::DateTime<chrono::Utc>>,
        git:      Option<GitOrigin>,
    },
    S3 {
        url:  S3Url,
        meta: Box<S3ObjectMeta>,
    },
}

/// A source, fingerprinted and ready to be read.
#[derive(Debug, Clone)]
pub struct Source {
    origin:   Origin,
    kind:     SourceKind,
    size:     u64,
    digest:   ContentDigest,
    /// Git blob id of the content; absent when the digest came from S3's
    /// stored checksum and the content was not read for fingerprinting.
    git_blob: Option<String>,
}

/// SHA-256 and git blob SHA-1 over one pass of the content.
struct Fingerprint {
    sha256: sha2::Sha256,
    blob:   sha1::Sha1,
}

impl Fingerprint {
    fn new(size: u64) -> Self {
        let mut blob = sha1::Sha1::new();
        blob.update(format!("blob {size}\0").as_bytes());
        Self { sha256: sha2::Sha256::new(), blob }
    }

    async fn read(
        mut self,
        mut input: impl AsyncRead + Unpin,
    ) -> anyhow::Result<(ContentDigest, String)> {
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = input.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            self.sha256.update(&buf[..n]);
            self.blob.update(&buf[..n]);
        }
        Ok((
            ContentDigest::new(self.sha256.finalize().into()),
            hex::encode(self.blob.finalize()),
        ))
    }
}

impl Source {
    /// Fingerprint a source. Local files are read once; S3 objects use the
    /// full-object SHA-256 S3 stores when there is one, and are otherwise
    /// streamed once for fingerprinting (and again when read).
    pub async fn open(
        location: Location,
        kind: SourceKind,
        store: &S3Store,
    ) -> anyhow::Result<Self> {
        match location {
            Location::Local(path) => Self::open_local(path, kind).await,
            Location::S3(url) => Self::open_s3(url, kind, store).await,
        }
    }

    async fn open_local(
        path: PathBuf,
        kind: SourceKind,
    ) -> anyhow::Result<Self> {
        let path = tokio::fs::canonicalize(&path)
            .await
            .with_context(|| format!("cannot resolve {}", path.display()))?;
        let meta = tokio::fs::metadata(&path).await?;
        let size = meta.len();
        let modified = meta.modified().ok().map(chrono::DateTime::from);
        let file = tokio::fs::File::open(&path).await?;
        let (digest, git_blob) = Fingerprint::new(size).read(file).await?;
        let git = GitOrigin::discover(path.clone()).await;
        Ok(Self {
            origin: Origin::Local { path, modified, git },
            kind,
            size,
            digest,
            git_blob: Some(git_blob),
        })
    }

    async fn open_s3(
        url: S3Url,
        kind: SourceKind,
        store: &S3Store,
    ) -> anyhow::Result<Self> {
        let meta = store.head(&url).await?;
        let size = meta.size;
        let (digest, git_blob) = match meta.content_sha256() {
            Some(sha) => (ContentDigest::new(sha), None),
            None => {
                let body =
                    store.get(&url, &meta, None).await?.into_async_read();
                let (digest, blob) = Fingerprint::new(size).read(body).await?;
                (digest, Some(blob))
            },
        };
        Ok(Self {
            origin: Origin::S3 { url, meta: Box::new(meta) },
            kind,
            size,
            digest,
            git_blob,
        })
    }

    pub fn origin(&self) -> &Origin { &self.origin }

    pub fn kind(&self) -> SourceKind { self.kind }

    pub fn digest(&self) -> ContentDigest { self.digest }

    pub fn size(&self) -> u64 { self.size }

    /// For logs.
    pub fn display(&self) -> String {
        match &self.origin {
            Origin::Local { path, .. } => path.display().to_string(),
            Origin::S3 { url, .. } => url.to_string(),
        }
    }

    /// Emit the `raw:File` / `dcat:Distribution` description of this source.
    pub fn describe(&self, iris: &SourceIris, out: &mut GraphEmitter) {
        let s: Node = Arc::new(iris.source());
        out.quad(s.clone(), rdf::TYPE, self.kind.class());
        for class in [raw::FILE, raw::SOURCE, prov::ENTITY, dcat::DISTRIBUTION]
        {
            out.quad(s.clone(), rdf::TYPE, class);
        }
        let (name, location, modified) = match &self.origin {
            Origin::Local { path, modified, .. } => {
                (
                    path.file_name().map(|n| n.to_string_lossy().into_owned()),
                    url::Url::from_file_path(path).ok().map(String::from),
                    *modified,
                )
            },
            Origin::S3 { url, meta } => {
                (
                    Some(url.file_name().to_owned()),
                    Some(url.iri()),
                    meta.last_modified,
                )
            },
        };
        if let Some(name) = name {
            out.quad(
                s.clone(),
                raw::FILE_NAME,
                Literal::new_simple_literal(&name),
            );
            out.quad(
                s.clone(),
                dcterms::TITLE,
                Literal::new_simple_literal(&name),
            );
        }
        if let Some(iri) = location.and_then(|l| NamedNode::new(l).ok()) {
            let iri = Arc::new(iri);
            out.quad(s.clone(), dcat::DOWNLOAD_URL, &iri);
            out.quad(s.clone(), prov::AT_LOCATION, iri);
        }
        out.quad(
            s.clone(),
            dcat::MEDIA_TYPE,
            NamedNode::new_unchecked(format!(
                "https://www.iana.org/assignments/media-types/{}",
                self.kind.media_type()
            )),
        );
        out.quad(
            s.clone(),
            dcat::BYTE_SIZE,
            Literal::new_typed_literal(
                self.size.to_string(),
                xsd::NON_NEGATIVE_INTEGER,
            ),
        );
        if let Some(m) = modified {
            out.quad(
                s.clone(),
                dcterms::MODIFIED,
                Literal::new_typed_literal(m.to_rfc3339(), xsd::DATE_TIME),
            );
        }
        let ck: Node = Arc::new(iris.checksum());
        out.quad(s.clone(), spdx::CHECKSUM, &ck);
        out.quad(ck.clone(), rdf::TYPE, spdx::CHECKSUM_CLASS);
        out.quad(ck.clone(), spdx::ALGORITHM, spdx::SHA256);
        out.quad(
            ck,
            spdx::CHECKSUM_VALUE,
            Literal::new_typed_literal(self.digest.hex(), xsd::HEX_BINARY),
        );
        if let Some(blob) = &self.git_blob {
            out.quad(
                s.clone(),
                raw::GIT_BLOB,
                Literal::new_simple_literal(blob),
            );
        }
        match &self.origin {
            Origin::Local { git: Some(git), .. } => {
                if let Some(blob) = &self.git_blob {
                    git.describe(&s, blob, out);
                }
            },
            Origin::Local { git: None, .. } => {},
            Origin::S3 { url, meta } => describe_s3(&s, url, meta, iris, out),
        }
    }
}

/// The S3 object's own metadata, as S3 reported it.
fn describe_s3(
    s: &Node,
    url: &S3Url,
    m: &S3ObjectMeta,
    iris: &SourceIris,
    out: &mut GraphEmitter,
) {
    out.quad(s.clone(), rdf::TYPE, raw::S3_OBJECT);
    let mut text = |p: NamedNodeRef<'static>, v: Option<&str>| {
        if let Some(v) = v {
            out.quad(s.clone(), p, Literal::new_simple_literal(v));
        }
    };
    text(raw::S3_BUCKET, Some(url.bucket()));
    text(raw::S3_KEY, Some(url.key()));
    text(raw::S3_REGION, m.region.as_deref());
    text(raw::S3_ENDPOINT, m.endpoint.as_deref());
    text(raw::S3_VERSION_ID, m.version_id.as_deref());
    text(raw::S3_ETAG, m.e_tag.as_deref());
    text(raw::S3_CONTENT_TYPE, m.content_type.as_deref());
    text(
        raw::S3_CONTENT_ENCODING,
        m.content_encoding.as_deref(),
    );
    text(
        raw::S3_CONTENT_LANGUAGE,
        m.content_language.as_deref(),
    );
    text(
        raw::S3_CONTENT_DISPOSITION,
        m.content_disposition.as_deref(),
    );
    text(raw::S3_CACHE_CONTROL, m.cache_control.as_deref());
    text(raw::S3_STORAGE_CLASS, m.storage_class.as_deref());
    text(
        raw::S3_SERVER_SIDE_ENCRYPTION,
        m.server_side_encryption.as_deref(),
    );
    text(raw::S3_KMS_KEY_ID, m.kms_key_id.as_deref());
    text(raw::S3_CHECKSUM_TYPE, m.checksum_type.as_deref());
    text(
        raw::S3_CHECKSUM_CRC32,
        m.checksum_crc32.as_deref(),
    );
    text(
        raw::S3_CHECKSUM_CRC32C,
        m.checksum_crc32c.as_deref(),
    );
    text(
        raw::S3_CHECKSUM_CRC64NVME,
        m.checksum_crc64nvme.as_deref(),
    );
    text(raw::S3_CHECKSUM_SHA1, m.checksum_sha1.as_deref());
    text(
        raw::S3_CHECKSUM_SHA256,
        m.checksum_sha256.as_deref(),
    );
    text(
        raw::S3_OBJECT_LOCK_MODE,
        m.object_lock_mode.as_deref(),
    );
    text(
        raw::S3_REPLICATION_STATUS,
        m.replication_status.as_deref(),
    );
    text(raw::S3_EXPIRATION, m.expiration.as_deref());
    for (p, v) in [
        (raw::S3_BUCKET_KEY_ENABLED, m.bucket_key_enabled),
        (
            raw::S3_OBJECT_LOCK_LEGAL_HOLD,
            m.object_lock_legal_hold,
        ),
    ] {
        if let Some(v) = v {
            out.quad(s.clone(), p, Literal::from(v));
        }
    }
    if let Some(n) = m.parts_count {
        out.quad(
            s.clone(),
            raw::S3_PARTS_COUNT,
            Literal::new_typed_literal(n.to_string(), xsd::INTEGER),
        );
    }
    if let Some(t) = m.object_lock_retain_until {
        out.quad(
            s.clone(),
            raw::S3_OBJECT_LOCK_RETAIN_UNTIL,
            Literal::new_typed_literal(t.to_rfc3339(), xsd::DATE_TIME),
        );
    }
    let entries = [
        ("tag", raw::S3_TAG, raw::S3_TAG_CLASS, &m.tags),
        (
            "metadata",
            raw::S3_USER_METADATA,
            raw::S3_USER_METADATA_ENTRY,
            &m.user_metadata,
        ),
    ];
    for (kind, link, class, pairs) in entries {
        for (key, value) in pairs {
            let e: Node = Arc::new(iris.s3_entry(kind, key));
            out.quad(s.clone(), link, &e);
            out.quad(e.clone(), rdf::TYPE, class);
            out.quad(
                e.clone(),
                raw::S3_ENTRY_KEY,
                Literal::new_simple_literal(key),
            );
            out.quad(
                e,
                raw::S3_ENTRY_VALUE,
                Literal::new_simple_literal(value),
            );
        }
    }
}
