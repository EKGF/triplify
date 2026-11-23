//! Amazon S3 (and S3-compatible) sources and outputs.
//!
//! Credentials, region and endpoint come from the standard AWS chain:
//! environment (`AWS_ACCESS_KEY_ID`, `AWS_REGION`, `AWS_ENDPOINT_URL`, ...),
//! `~/.aws` profiles including SSO, ECS task roles, Lambda and instance
//! metadata. A custom endpoint (MinIO, LocalStack, ...) uses path-style
//! addressing.

use {
    anyhow::Context,
    aws_sdk_s3::{
        Client,
        primitives::ByteStream,
        types::{ChecksumMode, CompletedMultipartUpload, CompletedPart},
    },
    bytes::Bytes,
    std::{fmt, str::FromStr},
    tokio::sync::OnceCell,
};

/// `s3://bucket/key`. A key ending in `/` (or empty) is a prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct S3Url {
    bucket: String,
    key:    String,
}

#[derive(Debug, thiserror::Error)]
#[error("`{0}` is not an s3://bucket/key URL")]
pub struct InvalidS3Url(String);

impl S3Url {
    pub fn new(bucket: impl Into<String>, key: impl Into<String>) -> Self {
        Self { bucket: bucket.into(), key: key.into() }
    }

    pub fn bucket(&self) -> &str { &self.bucket }

    pub fn key(&self) -> &str { &self.key }

    /// Whether this names a prefix (to list) rather than one object.
    pub fn is_prefix(&self) -> bool {
        self.key.is_empty() || self.key.ends_with('/')
    }

    /// The last key segment, like a file name.
    pub fn file_name(&self) -> &str {
        self.key.rsplit('/').next().unwrap_or(&self.key)
    }

    /// The URL as an IRI, with the key percent-encoded per segment.
    pub fn iri(&self) -> String {
        let mut url = url::Url::parse(&format!("s3://{}/", self.bucket))
            .expect("validated bucket");
        if let Ok(mut segments) = url.path_segments_mut() {
            segments.clear();
            segments.extend(self.key.split('/'));
        }
        url.into()
    }

    pub fn looks_like(s: &str) -> bool { s.starts_with("s3://") }
}

impl FromStr for S3Url {
    type Err = InvalidS3Url;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let rest = s
            .strip_prefix("s3://")
            .ok_or_else(|| InvalidS3Url(s.into()))?;
        let (bucket, key) = rest.split_once('/').unwrap_or((rest, ""));
        let valid_bucket = (3..=63).contains(&bucket.len()) &&
            bucket.bytes().all(|b| {
                b.is_ascii_lowercase() ||
                    b.is_ascii_digit() ||
                    b == b'.' ||
                    b == b'-'
            });
        if valid_bucket {
            Ok(Self::new(bucket, key))
        } else {
            Err(InvalidS3Url(s.into()))
        }
    }
}

impl fmt::Display for S3Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "s3://{}/{}", self.bucket, self.key)
    }
}

/// Everything S3 reports about one object version.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct S3ObjectMeta {
    pub size:                     u64,
    pub region:                   Option<String>,
    pub endpoint:                 Option<String>,
    pub version_id:               Option<String>,
    pub e_tag:                    Option<String>,
    pub last_modified:            Option<chrono::DateTime<chrono::Utc>>,
    pub content_type:             Option<String>,
    pub content_encoding:         Option<String>,
    pub content_language:         Option<String>,
    pub content_disposition:      Option<String>,
    pub cache_control:            Option<String>,
    pub storage_class:            Option<String>,
    pub server_side_encryption:   Option<String>,
    pub kms_key_id:               Option<String>,
    pub bucket_key_enabled:       Option<bool>,
    pub checksum_type:            Option<String>,
    pub checksum_crc32:           Option<String>,
    pub checksum_crc32c:          Option<String>,
    pub checksum_crc64nvme:       Option<String>,
    pub checksum_sha1:            Option<String>,
    pub checksum_sha256:          Option<String>,
    pub parts_count:              Option<u32>,
    pub object_lock_mode:         Option<String>,
    pub object_lock_retain_until: Option<chrono::DateTime<chrono::Utc>>,
    pub object_lock_legal_hold:   Option<bool>,
    pub replication_status:       Option<String>,
    pub expiration:               Option<String>,
    pub user_metadata:            Vec<(String, String)>,
    pub tags:                     Vec<(String, String)>,
}

impl S3ObjectMeta {
    /// The SHA-256 of the content, when S3 holds a full-object SHA-256.
    pub fn content_sha256(&self) -> Option<[u8; 32]> {
        use base64::Engine as _;
        if self.checksum_type.as_deref() != Some("FULL_OBJECT") {
            return None;
        }
        let raw = base64::engine::general_purpose::STANDARD
            .decode(self.checksum_sha256.as_deref()?)
            .ok()?;
        raw.try_into().ok()
    }
}

fn utc(
    t: &aws_smithy_types::DateTime,
) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::from_timestamp(t.secs(), t.subsec_nanos())
}

/// A lazily configured S3 client, shared by every S3 source and output.
#[derive(Debug, Default)]
pub struct S3Store {
    client: OnceCell<(Client, Option<String>, Option<String>)>,
}

impl S3Store {
    async fn client(
        &self,
    ) -> anyhow::Result<&(Client, Option<String>, Option<String>)> {
        self.client
            .get_or_try_init(|| {
                async {
                    let config = aws_config::load_from_env().await;
                    let endpoint = config.endpoint_url().map(str::to_owned);
                    let region = config.region().map(|r| r.as_ref().to_owned());
                    let s3 = aws_sdk_s3::config::Builder::from(&config)
                        .force_path_style(endpoint.is_some())
                        .build();
                    Ok((Client::from_conf(s3), region, endpoint))
                }
            })
            .await
    }

    /// Every object under a prefix, or the one object a key names.
    pub async fn list(&self, url: &S3Url) -> anyhow::Result<Vec<S3Url>> {
        if !url.is_prefix() {
            return Ok(vec![url.clone()]);
        }
        let (client, ..) = self.client().await?;
        let mut pages = client
            .list_objects_v2()
            .bucket(url.bucket())
            .prefix(url.key())
            .into_paginator()
            .send();
        let mut found = Vec::new();
        while let Some(page) = pages.next().await {
            let page = page.with_context(|| format!("cannot list {url}"))?;
            for object in page.contents() {
                if let Some(key) = object.key().filter(|k| !k.ends_with('/')) {
                    found.push(S3Url::new(url.bucket(), key));
                }
            }
        }
        Ok(found)
    }

    /// Object metadata (HeadObject with checksums) and tags.
    pub async fn head(&self, url: &S3Url) -> anyhow::Result<S3ObjectMeta> {
        let (client, region, endpoint) = self.client().await?;
        let h = client
            .head_object()
            .bucket(url.bucket())
            .key(url.key())
            .checksum_mode(ChecksumMode::Enabled)
            .send()
            .await
            .with_context(|| format!("cannot read metadata of {url}"))?;
        let s = |v: Option<&str>| v.map(str::to_owned);
        let mut user_metadata: Vec<(String, String)> = h
            .metadata()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        user_metadata.sort();
        let tags = match client
            .get_object_tagging()
            .bucket(url.bucket())
            .key(url.key())
            .set_version_id(h.version_id().map(str::to_owned))
            .send()
            .await
        {
            Ok(t) => {
                let mut tags: Vec<(String, String)> = t
                    .tag_set()
                    .iter()
                    .map(|t| (t.key().to_owned(), t.value().to_owned()))
                    .collect();
                tags.sort();
                tags
            },
            Err(e) => {
                tracing::warn!(
                    "{url}: tags not captured ({}); grant s3:GetObjectTagging \
                     to capture them",
                    aws_sdk_s3::error::DisplayErrorContext(&e)
                );
                Vec::new()
            },
        };
        Ok(S3ObjectMeta {
            size: u64::try_from(h.content_length().unwrap_or(0)).unwrap_or(0),
            region: region.clone(),
            endpoint: endpoint.clone(),
            version_id: s(h.version_id()).filter(|v| v != "null"),
            e_tag: h.e_tag().map(|e| e.trim_matches('"').to_owned()),
            last_modified: h.last_modified().and_then(utc),
            content_type: s(h.content_type()),
            content_encoding: s(h.content_encoding()),
            content_language: s(h.content_language()),
            content_disposition: s(h.content_disposition()),
            cache_control: s(h.cache_control()),
            storage_class: h.storage_class().map(|c| c.as_str().to_owned()),
            server_side_encryption: h
                .server_side_encryption()
                .map(|e| e.as_str().to_owned()),
            kms_key_id: s(h.ssekms_key_id()),
            bucket_key_enabled: h.bucket_key_enabled(),
            checksum_type: h.checksum_type().map(|t| t.as_str().to_owned()),
            checksum_crc32: s(h.checksum_crc32()),
            checksum_crc32c: s(h.checksum_crc32_c()),
            checksum_crc64nvme: s(h.checksum_crc64_nvme()),
            checksum_sha1: s(h.checksum_sha1()),
            checksum_sha256: s(h.checksum_sha256()),
            parts_count: h.parts_count().and_then(|n| u32::try_from(n).ok()),
            object_lock_mode: h
                .object_lock_mode()
                .map(|m| m.as_str().to_owned()),
            object_lock_retain_until: h
                .object_lock_retain_until_date()
                .and_then(utc),
            object_lock_legal_hold: h
                .object_lock_legal_hold_status()
                .map(|s| s.as_str() == "ON"),
            replication_status: h
                .replication_status()
                .map(|r| r.as_str().to_owned()),
            expiration: s(h.expiration()),
            user_metadata,
            tags,
        })
    }

    /// Stream the object version `meta` describes (or a byte range of it).
    /// `If-Match` on the ETag makes the read fail, rather than mix
    /// versions, if the object changed since its metadata was read.
    pub async fn get(
        &self,
        url: &S3Url,
        meta: &S3ObjectMeta,
        range: Option<(u64, u64)>,
    ) -> anyhow::Result<ByteStream> {
        let (client, ..) = self.client().await?;
        let out = client
            .get_object()
            .bucket(url.bucket())
            .key(url.key())
            .set_version_id(meta.version_id.clone())
            .set_if_match(meta.e_tag.as_ref().map(|e| format!("\"{e}\"")))
            .set_range(
                range.map(|(start, end)| format!("bytes={start}-{}", end - 1)),
            )
            .send()
            .await
            .with_context(|| format!("cannot read {url}"))?;
        Ok(out.body)
    }

    /// Start a multipart upload; see [`Upload`].
    pub async fn upload(
        &self,
        url: &S3Url,
        content_type: &str,
    ) -> anyhow::Result<Upload> {
        let (client, ..) = self.client().await?;
        let created = client
            .create_multipart_upload()
            .bucket(url.bucket())
            .key(url.key())
            .content_type(content_type)
            .send()
            .await
            .with_context(|| format!("cannot start upload to {url}"))?;
        Ok(Upload {
            client: client.clone(),
            url:    url.clone(),
            id:     created.upload_id().context("no upload id")?.to_owned(),
        })
    }
}

/// An in-progress multipart upload. Parts may be uploaded concurrently.
#[derive(Clone)]
pub struct Upload {
    client: Client,
    url:    S3Url,
    id:     String,
}

impl Upload {
    /// Upload part `number` (1-based; every part but the last >= 5 MiB).
    pub async fn part(
        &self,
        number: i32,
        bytes: Bytes,
    ) -> anyhow::Result<CompletedPart> {
        anyhow::ensure!(
            (1..=10_000).contains(&number),
            "{}: part number {number} out of range",
            self.url
        );
        let done = self
            .client
            .upload_part()
            .bucket(self.url.bucket())
            .key(self.url.key())
            .upload_id(&self.id)
            .part_number(number)
            .body(ByteStream::from(bytes))
            .send()
            .await
            .with_context(|| {
                format!("cannot upload part {number} of {}", self.url)
            })?;
        Ok(CompletedPart::builder()
            .part_number(number)
            .set_e_tag(done.e_tag().map(str::to_owned))
            .build())
    }

    pub async fn complete(
        self,
        mut parts: Vec<CompletedPart>,
    ) -> anyhow::Result<()> {
        parts.sort_by_key(|p| p.part_number());
        self.client
            .complete_multipart_upload()
            .bucket(self.url.bucket())
            .key(self.url.key())
            .upload_id(&self.id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(parts))
                    .build(),
            )
            .send()
            .await
            .with_context(|| {
                format!("cannot complete upload to {}", self.url)
            })?;
        Ok(())
    }

    /// Discard the parts so no partial object is left behind.
    pub async fn abort(self) {
        if let Err(e) = self
            .client
            .abort_multipart_upload()
            .bucket(self.url.bucket())
            .key(self.url.key())
            .upload_id(&self.id)
            .send()
            .await
        {
            tracing::error!(
                "{}: could not abort upload {}: {}",
                self.url,
                self.id,
                aws_sdk_s3::error::DisplayErrorContext(&e)
            );
        }
    }
}

/// Bytes written to [`S3Upload::writer`] become a multipart upload, which is
/// completed only by [`S3Upload::commit`]. Anything else (an error, a
/// dropped writer, [`S3Upload::abort`]) aborts it, so no truncated object is
/// ever published.
pub struct S3Upload {
    writer: Option<tokio::io::DuplexStream>,
    commit: tokio::sync::oneshot::Sender<()>,
    task:   tokio::task::JoinHandle<anyhow::Result<()>>,
}

const PART_SIZE: usize = 16 << 20;
const PARTS_IN_FLIGHT: usize = 4;

impl S3Upload {
    pub async fn start(
        store: &S3Store,
        url: &S3Url,
        content_type: &str,
    ) -> anyhow::Result<Self> {
        let upload = store.upload(url, content_type).await?;
        let (writer, reader) = tokio::io::duplex(1 << 20);
        let (commit, committed) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(Self::pump(reader, upload, committed));
        Ok(Self { writer: Some(writer), commit, task })
    }

    /// The write side; take it once.
    pub fn writer(&mut self) -> tokio::io::DuplexStream {
        self.writer.take().expect("S3Upload::writer taken twice")
    }

    /// Publish the object: everything was written successfully.
    pub async fn commit(self) -> anyhow::Result<()> {
        drop(self.writer);
        // The pump aborts by itself if it never gets this signal.
        let _ = self.commit.send(());
        self.task.await?
    }

    /// Discard the upload and wait until S3 has dropped its parts.
    pub async fn abort(self) {
        drop(self.writer);
        drop(self.commit);
        if let Ok(Err(e)) = self.task.await {
            tracing::debug!("upload aborted: {e:#}");
        }
    }

    async fn pump(
        mut reader: tokio::io::DuplexStream,
        upload: Upload,
        committed: tokio::sync::oneshot::Receiver<()>,
    ) -> anyhow::Result<()> {
        let result = Self::parts(&mut reader, &upload).await;
        match (result, committed.await) {
            (Ok(parts), Ok(())) => upload.complete(parts).await,
            (Ok(_), Err(_)) => {
                upload.abort().await;
                anyhow::bail!("output not committed; upload aborted")
            },
            (Err(e), _) => {
                upload.abort().await;
                Err(e)
            },
        }
    }

    /// Read the pipe into parts and upload them, a few at a time.
    async fn parts(
        reader: &mut tokio::io::DuplexStream,
        upload: &Upload,
    ) -> anyhow::Result<Vec<CompletedPart>> {
        use tokio::io::AsyncReadExt as _;
        let mut in_flight = tokio::task::JoinSet::new();
        let mut done = Vec::new();
        let mut number = 0i32;
        loop {
            // Double the part size every 1,000 parts: 10,000 parts then hold
            // far more than any output.
            let size = PART_SIZE << (number / 1000).min(8);
            let mut part = Vec::with_capacity(size);
            while part.len() < size {
                if (&mut *reader)
                    .take((size - part.len()) as u64)
                    .read_to_end(&mut part)
                    .await? ==
                    0
                {
                    break;
                }
            }
            let last = part.len() < size;
            if part.is_empty() && number > 0 {
                break;
            }
            number += 1;
            while in_flight.len() >= PARTS_IN_FLIGHT {
                done.push(in_flight.join_next().await.expect("non-empty")??);
            }
            let upload = upload.clone();
            in_flight.spawn(async move {
                upload.part(number, Bytes::from(part)).await
            });
            if last {
                break;
            }
        }
        while let Some(part) = in_flight.join_next().await {
            done.push(part??);
        }
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_objects_and_prefixes() {
        let u: S3Url = "s3://my-bucket/a/b c.csv".parse().unwrap();
        assert_eq!((u.bucket(), u.key()), ("my-bucket", "a/b c.csv"));
        assert!(!u.is_prefix());
        assert_eq!(u.file_name(), "b c.csv");
        assert_eq!(u.iri(), "s3://my-bucket/a/b%20c.csv");
        assert!(
            "s3://my-bucket/dumps/"
                .parse::<S3Url>()
                .unwrap()
                .is_prefix()
        );
        assert!("s3://my-bucket".parse::<S3Url>().unwrap().is_prefix());
        assert!("s3://Bad_Bucket/x".parse::<S3Url>().is_err());
        assert!("https://x/y".parse::<S3Url>().is_err());
    }

    #[test]
    fn full_object_sha256_is_the_content_digest() {
        let meta = S3ObjectMeta {
            checksum_type: Some("FULL_OBJECT".into()),
            // SHA-256 of the empty string.
            checksum_sha256: Some(
                "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=".into(),
            ),
            ..Default::default()
        };
        assert_eq!(
            hex::encode(meta.content_sha256().unwrap()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let composite =
            S3ObjectMeta { checksum_type: Some("COMPOSITE".into()), ..meta };
        assert_eq!(composite.content_sha256(), None);
    }
}
