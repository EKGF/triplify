//! Parquet sources keep their schema types; nulls are explicit NullValues.

use {
    arrow_array::{
        ArrayRef,
        BooleanArray,
        Date32Array,
        Decimal128Array,
        Float64Array,
        Int64Array,
        RecordBatch,
        StringArray,
        TimestampMicrosecondArray,
    },
    oxrdf::{Dataset, NamedNodeRef, TermRef},
    oxrdfio::{RdfFormat, RdfParser},
    parquet::arrow::ArrowWriter,
    std::sync::Arc,
    triplify::{
        Triplify,
        vocab::{raw, rdf, rdfs, xsd},
        writer::Output,
    },
};

fn write_fixture(path: &std::path::Path) {
    let columns: Vec<(&str, ArrayRef)> = vec![
        ("id", Arc::new(Int64Array::from(vec![1, 2, 3]))),
        (
            "name",
            Arc::new(StringArray::from(vec![
                Some("00123"),
                None,
                Some("Bob"),
            ])),
        ),
        (
            "price",
            Arc::new(
                Decimal128Array::from(vec![1250, -5, 100_000])
                    .with_precision_and_scale(10, 2)
                    .unwrap(),
            ),
        ),
        (
            "ratio",
            Arc::new(Float64Array::from(vec![0.5, 1.0, 2.25])),
        ),
        (
            "active",
            Arc::new(BooleanArray::from(vec![true, false, true])),
        ),
        // 2026-11-18 is day 20775 since the Unix epoch.
        (
            "day",
            Arc::new(Date32Array::from(vec![20775, 20776, 20777])),
        ),
        (
            "at",
            Arc::new(
                TimestampMicrosecondArray::from(vec![1_763_424_000_000_000; 3])
                    .with_timezone("+01:00"),
            ),
        ),
    ];
    let batch = RecordBatch::try_from_iter(columns).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut writer = ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
}

fn literals(ds: &Dataset, p: NamedNodeRef<'_>) -> Vec<(String, String)> {
    let mut v: Vec<_> = ds
        .iter()
        .filter(|q| q.predicate == p)
        .filter_map(|q| {
            match q.object {
                TermRef::Literal(l) => {
                    Some((
                        l.value().to_owned(),
                        l.datatype().as_str().to_owned(),
                    ))
                },
                _ => None,
            }
        })
        .collect();
    v.sort();
    v
}

#[tokio::test]
async fn parquet_values_keep_schema_types() {
    let input = tempfile::tempdir().unwrap();
    write_fixture(&input.path().join("orders.parquet"));
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("raw.nq");
    let summary = Triplify::new(
        vec![input.path().to_owned()],
        Output::file(out.clone(), false).unwrap(),
    )
    .with_profile(true)
    .run()
    .await
    .unwrap();
    assert_eq!((summary.sources, summary.failed), (1, 0));
    let ds: Dataset = RdfParser::from_format(RdfFormat::NQuads)
        .for_slice(&std::fs::read(&out).unwrap())
        .collect::<Result<_, _>>()
        .unwrap();

    let typed = |p, o: NamedNodeRef<'_>| {
        ds.iter()
            .filter(|q| q.predicate == p && q.object == o.into())
            .count()
    };
    assert_eq!(typed(rdf::TYPE, raw::PARQUET_FILE), 1);
    assert_eq!(typed(rdf::TYPE, raw::VIEW), 1);
    assert_eq!(typed(rdf::TYPE, raw::COLUMN), 7);
    assert_eq!(typed(rdf::TYPE, raw::ROW), 3);
    // 7 × 3 cells, one of them an explicit Parquet null.
    assert_eq!(typed(rdf::TYPE, raw::VALUE), 20);
    assert_eq!(typed(rdf::TYPE, raw::NULL_VALUE), 1);

    let headers: Vec<String> = literals(&ds, rdfs::LABEL)
        .into_iter()
        .map(|(v, _)| v)
        .collect();
    assert_eq!(headers, [
        "active", "at", "day", "id", "name", "price", "ratio"
    ]);

    let values = literals(&ds, raw::LITERAL_VALUE);
    let has = |v: &str, t: NamedNodeRef<'_>| {
        values.contains(&(v.into(), t.as_str().into()))
    };
    assert!(
        has("00123", xsd::STRING),
        "text is never reinterpreted"
    );
    assert!(has("12.50", xsd::DECIMAL), "{values:?}");
    assert!(has("-0.05", xsd::DECIMAL));
    assert!(has("1", xsd::INTEGER));
    assert!(has("2.25", xsd::DOUBLE));
    assert!(has("true", xsd::BOOLEAN));
    assert!(has("2026-11-18", xsd::DATE));
    assert!(
        has("2025-11-18T00:00:00Z", xsd::DATE_TIME),
        "zoned timestamps are UTC instants"
    );

    // Profiles see the native types.
    let inferred: Vec<String> = ds
        .iter()
        .filter(|q| q.predicate == raw::INFERRED_DATATYPE)
        .map(|q| q.object.to_string())
        .collect();
    for t in [
        xsd::INTEGER,
        xsd::DECIMAL,
        xsd::DATE,
        xsd::BOOLEAN,
        xsd::DATE_TIME,
    ] {
        assert!(
            inferred.contains(&t.to_string()),
            "{t} in {inferred:?}"
        );
    }
}

#[tokio::test]
async fn parquet_ignores_no_header() {
    let input = tempfile::tempdir().unwrap();
    write_fixture(&input.path().join("orders.parquet"));
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("raw.nq");
    Triplify::new(
        vec![input.path().to_owned()],
        Output::file(out.clone(), false).unwrap(),
    )
    .with_header(triplify::emit::Header::Absent)
    .run()
    .await
    .unwrap();
    let ds: Dataset = RdfParser::from_format(RdfFormat::NQuads)
        .for_slice(&std::fs::read(&out).unwrap())
        .collect::<Result<_, _>>()
        .unwrap();
    let rows = ds
        .iter()
        .filter(|q| q.predicate == rdf::TYPE && q.object == raw::ROW.into())
        .count();
    let labels = literals(&ds, rdfs::LABEL).len();
    assert_eq!(
        (rows, labels),
        (3, 7),
        "schema names stay labels, not a data row"
    );
}
