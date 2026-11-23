//! S3 sources and outputs, against a real S3 API.
//!
//! Skipped unless `TRIPLIFY_TEST_S3=1`. Uses the standard AWS environment,
//! e.g. for LocalStack:
//!
//! ```sh
//! docker run -d -p 4566:4566 -e SERVICES=s3 localstack/localstack:4.4
//! TRIPLIFY_TEST_S3=1 AWS_ENDPOINT_URL=http://127.0.0.1:4566 \
//!   AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test AWS_REGION=us-east-1 \
//!   cargo test --test s3
//! ```

use {
    arrow_array::{Int64Array, RecordBatch, StringArray},
    aws_sdk_s3::{
        Client,
        primitives::ByteStream,
        types::{
            BucketVersioningStatus,
            ChecksumAlgorithm,
            VersioningConfiguration,
        },
    },
    oxrdf::{Dataset, NamedNodeRef, Quad, TermRef},
    oxrdfio::{RdfFormat, RdfParser},
    std::{io::Read, sync::Arc},
    triplify::{
        Triplify,
        location::Location,
        s3::S3Url,
        vocab::{raw, rdf},
        writer::Output,
    },
};

fn enabled() -> bool {
    std::env::var("TRIPLIFY_TEST_S3").is_ok_and(|v| v == "1")
}

async fn client() -> Client {
    let config = aws_config::load_from_env().await;
    let s3 = aws_sdk_s3::config::Builder::from(&config)
        .force_path_style(config.endpoint_url().is_some())
        .build();
    Client::from_conf(s3)
}

const PEOPLE: &str = "id,name,code\n1,Alice,00123\n2,Bob,00456\n";
const PLAIN: &str = "a,b\nx,1\ny,2\n";

fn parquet_bytes() -> Vec<u8> {
    let batch = RecordBatch::try_from_iter([
        (
            "id",
            Arc::new(Int64Array::from(vec![1, 2, 3])) as _,
        ),
        (
            "name",
            Arc::new(StringArray::from(vec!["p", "q", "r"])) as _,
        ),
    ])
    .unwrap();
    let mut out = Vec::new();
    let mut w =
        parquet::arrow::ArrowWriter::try_new(&mut out, batch.schema(), None)
            .unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    out
}

fn xlsx_bytes() -> Vec<u8> {
    let mut wb = rust_xlsxwriter::Workbook::new();
    let ws = wb.add_worksheet();
    ws.write_string(0, 0, "qty").unwrap();
    ws.write_number(1, 0, 3).unwrap();
    wb.save_to_buffer().unwrap()
}

/// A fresh, versioned bucket with four sources under `in/`.
async fn fixture(s3: &Client) -> String {
    let bucket = format!("triplify-test-{}", uuid::Uuid::new_v4());
    s3.create_bucket().bucket(&bucket).send().await.unwrap();
    s3.put_bucket_versioning()
        .bucket(&bucket)
        .versioning_configuration(
            VersioningConfiguration::builder()
                .status(BucketVersioningStatus::Enabled)
                .build(),
        )
        .send()
        .await
        .unwrap();
    // A full-object SHA-256: triplify reads this object only once.
    s3.put_object()
        .bucket(&bucket)
        .key("in/people.csv")
        .body(ByteStream::from_static(PEOPLE.as_bytes()))
        .checksum_algorithm(ChecksumAlgorithm::Sha256)
        .content_type("text/csv")
        .metadata("source-system", "crm")
        .tagging("classification=internal&owner=data%20team")
        .send()
        .await
        .unwrap();
    for (key, body) in [
        ("in/plain.csv", PLAIN.as_bytes().to_vec()),
        ("in/orders.parquet", parquet_bytes()),
        ("in/book.xlsx", xlsx_bytes()),
        ("in/notes.txt", b"ignored".to_vec()),
    ] {
        s3.put_object()
            .bucket(&bucket)
            .key(key)
            .body(ByteStream::from(body))
            .send()
            .await
            .unwrap();
    }
    bucket
}

async fn download(s3: &Client, bucket: &str, key: &str) -> Vec<u8> {
    let body = s3
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .unwrap()
        .body;
    let gz = body.collect().await.unwrap().into_bytes();
    let mut nq = Vec::new();
    // Single-member decoder: the output must be one standard gzip member.
    flate2::read::GzDecoder::new(&gz[..])
        .read_to_end(&mut nq)
        .unwrap();
    nq
}

fn load(nq: &[u8]) -> Dataset {
    RdfParser::from_format(RdfFormat::NQuads)
        .for_slice(nq)
        .collect::<Result<_, _>>()
        .unwrap()
}

fn strings(ds: &Dataset, p: NamedNodeRef<'_>) -> Vec<String> {
    let mut v: Vec<String> = ds
        .iter()
        .filter(|q| q.predicate == p)
        .filter_map(|q| {
            match q.object {
                TermRef::Literal(l) => Some(l.value().to_owned()),
                _ => None,
            }
        })
        .collect();
    v.sort();
    v
}

/// Value, row and column quads: the captured data itself.
fn data(ds: &Dataset) -> Vec<Quad> {
    let structural = [
        raw::VALUE_OF,
        raw::FOR_COLUMN,
        raw::LITERAL_VALUE,
        raw::IS_ROW_IN_VIEW,
        raw::VIEW_ROW_NUMBER,
        raw::COLUMN_NUMBER,
        raw::IS_COLUMN_IN_VIEW,
    ];
    let mut v: Vec<Quad> = ds
        .iter()
        .filter(|q| structural.contains(&q.predicate))
        .map(|q| q.into_owned())
        .collect();
    v.sort_by_key(ToString::to_string);
    v
}

#[tokio::test]
async fn s3_prefix_to_s3_gzip_with_object_metadata() {
    if !enabled() {
        eprintln!("skipped: set TRIPLIFY_TEST_S3=1 and AWS_* to run");
        return;
    }
    let s3 = client().await;
    let bucket = fixture(&s3).await;
    let input: S3Url = format!("s3://{bucket}/in/").parse().unwrap();
    let output: S3Url = format!("s3://{bucket}/out/raw.nq.gz").parse().unwrap();

    let summary = Triplify::new(
        [Location::S3(input)],
        Output::to(Location::S3(output), false).unwrap(),
    )
    .with_profile(true)
    .run()
    .await
    .unwrap();
    assert_eq!(
        (summary.sources, summary.failed),
        (4, 0),
        "notes.txt is not a source"
    );

    let ds = load(&download(&s3, &bucket, "out/raw.nq.gz").await);
    let count = |p: NamedNodeRef<'_>, o: NamedNodeRef<'_>| {
        ds.iter()
            .filter(|q| q.predicate == p && q.object == o.into())
            .count()
    };
    assert_eq!(count(rdf::TYPE, raw::S3_OBJECT), 4);
    assert_eq!(strings(&ds, raw::S3_BUCKET), vec![
        bucket.clone();
        4
    ]);
    assert_eq!(strings(&ds, raw::S3_KEY), [
        "in/book.xlsx",
        "in/orders.parquet",
        "in/people.csv",
        "in/plain.csv"
    ]);
    assert_eq!(
        strings(&ds, raw::S3_VERSION_ID).len(),
        4,
        "versioned bucket"
    );
    assert!(
        strings(&ds, raw::S3_ETAG).iter().all(|e| !e.contains('"')),
        "ETags without quotes"
    );
    assert_eq!(
        strings(&ds, raw::S3_CHECKSUM_SHA256).len(),
        1,
        "only people.csv has one"
    );
    // Verbatim: the three objects uploaded without one default to binary.
    let types = strings(&ds, raw::S3_CONTENT_TYPE);
    assert_eq!(
        types.iter().filter(|t| *t == "text/csv").count(),
        1,
        "{types:?}"
    );

    // Tags and user metadata as key-value entries.
    assert_eq!(count(rdf::TYPE, raw::S3_TAG_CLASS), 2);
    assert_eq!(count(rdf::TYPE, raw::S3_USER_METADATA_ENTRY), 1);
    assert_eq!(strings(&ds, raw::S3_ENTRY_KEY), [
        "classification",
        "owner",
        "source-system"
    ]);
    assert_eq!(strings(&ds, raw::S3_ENTRY_VALUE), [
        "crm",
        "data team",
        "internal"
    ]);

    // people.csv was identified by S3's SHA-256 and read once: no git blob.
    // The other three were fingerprinted by streaming them.
    assert_eq!(strings(&ds, raw::GIT_BLOB).len(), 3);
    assert!(strings(&ds, raw::LITERAL_VALUE).contains(&"00123".to_owned()));

    // Content-addressed: the same bytes read locally give the same data.
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("people.csv"), PEOPLE).unwrap();
    std::fs::write(local.path().join("plain.csv"), PLAIN).unwrap();
    std::fs::write(
        local.path().join("orders.parquet"),
        parquet_bytes(),
    )
    .unwrap();
    std::fs::write(local.path().join("book.xlsx"), xlsx_bytes()).unwrap();
    let out = local.path().join("local.nq");
    Triplify::new(
        [local.path().to_owned()],
        Output::file(out.clone(), false).unwrap(),
    )
    .with_profile(true)
    .run()
    .await
    .unwrap();
    assert_eq!(
        data(&load(&std::fs::read(&out).unwrap())),
        data(&ds)
    );
}

#[tokio::test]
async fn failed_run_publishes_nothing() {
    if !enabled() {
        return;
    }
    let s3 = client().await;
    let bucket = fixture(&s3).await;
    let output: S3Url =
        format!("s3://{bucket}/out/partial.nq.gz").parse().unwrap();
    let roots = [
        Location::S3(format!("s3://{bucket}/in/").parse().unwrap()),
        Location::S3("s3://triplify-no-such-bucket-0/x/".parse().unwrap()),
    ];
    let summary = Triplify::new(
        roots,
        Output::to(Location::S3(output), false).unwrap(),
    )
    .run()
    .await
    .unwrap();
    assert_eq!((summary.sources, summary.failed), (4, 1));
    let head = s3
        .head_object()
        .bucket(&bucket)
        .key("out/partial.nq.gz")
        .send()
        .await;
    assert!(
        head.is_err(),
        "a failed run must not create the output object"
    );
    let uploads = s3
        .list_multipart_uploads()
        .bucket(&bucket)
        .send()
        .await
        .unwrap();
    assert!(
        uploads.uploads().is_empty(),
        "the aborted upload left no parts behind"
    );
}
