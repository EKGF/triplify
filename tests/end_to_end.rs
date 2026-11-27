//! Triplify a small directory and check the raw shape of the output.

use {
    oxrdf::{Dataset, NamedNodeRef, QuadRef},
    oxrdfio::{RdfFormat, RdfParser},
    std::{io::Read, path::Path},
    triplify::{
        Triplify,
        vocab::{dcat, raw, rdf, rdfs, xsd},
        writer::Output,
    },
};

fn fixtures(dir: &Path) {
    std::fs::write(
        dir.join("people.csv"),
        "\u{feff}id,name,code\n1,Alice,00123\n2,,7,extra\n",
    )
    .unwrap();
    let mut wb = rust_xlsxwriter::Workbook::new();
    let ws = wb.add_worksheet().set_name("Orders").unwrap();
    ws.write_string(0, 0, "order").unwrap();
    ws.write_string(0, 1, "amount").unwrap();
    ws.write_string(0, 2, "paid").unwrap();
    ws.write_number(1, 0, 42).unwrap();
    ws.write_number(1, 1, 9.5).unwrap();
    ws.write_boolean(1, 2, true).unwrap();
    wb.add_worksheet().set_name("Empty").unwrap();
    let ws = wb.add_worksheet().set_name("Calc").unwrap();
    for (c, h) in ["qty", "price", "total"].iter().enumerate() {
        ws.write_string(0, c as u16, *h).unwrap();
    }
    for r in 1..=3u32 {
        ws.write_number(r, 0, f64::from(r)).unwrap();
        ws.write_number(r, 1, 10.0).unwrap();
        ws.write_formula(r, 2, format!("=A{0}*B{0}", r + 1).as_str())
            .unwrap();
    }
    wb.save(dir.join("orders.xlsx")).unwrap();
    // Excel lock file and an unsupported file are ignored.
    std::fs::write(dir.join("~$orders.xlsx"), b"junk").unwrap();
    std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
}

async fn run(dir: &Path, out: &Path, canonical: bool) -> triplify::Summary {
    Triplify::new(
        vec![dir.to_owned()],
        Output::file(out.to_owned(), canonical).unwrap(),
    )
    .run()
    .await
    .unwrap()
}

fn load(bytes: &[u8], format: RdfFormat) -> Dataset {
    RdfParser::from_format(format)
        .for_slice(bytes)
        .collect::<Result<Dataset, _>>()
        .unwrap()
}

fn count(ds: &Dataset, p: NamedNodeRef<'_>, o: NamedNodeRef<'_>) -> usize {
    ds.iter()
        .filter(|q: &QuadRef<'_>| q.predicate == p && q.object == o.into())
        .count()
}

#[tokio::test]
async fn csv_and_xlsx_to_gzipped_nquads() {
    let input = tempfile::tempdir().unwrap();
    fixtures(input.path());
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("raw.nq.gz");
    let summary = run(input.path(), &out, false).await;
    assert_eq!((summary.sources, summary.failed), (2, 0));

    let mut nq = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(&out).unwrap())
        .read_to_end(&mut nq)
        .unwrap();
    let ds = load(&nq, RdfFormat::NQuads);
    assert_eq!(ds.len() as u64, summary.quads);

    assert_eq!(count(&ds, rdf::TYPE, raw::CSV_FILE), 1);
    assert_eq!(count(&ds, rdf::TYPE, raw::XLSX_FILE), 1);
    assert_eq!(count(&ds, rdf::TYPE, dcat::DATASET), 2);
    assert_eq!(count(&ds, rdf::TYPE, raw::VIEW), 4);
    // CSV: 3 header columns + 1 ragged; Orders: 3; Calc: 3.
    assert_eq!(count(&ds, rdf::TYPE, raw::COLUMN), 10);
    assert_eq!(count(&ds, rdf::TYPE, raw::ROW), 6);
    // CSV: 1,Alice,00123,2,7,extra ; XLSX: 42,9.5,true
    // CSV 6 + Orders 3 + Calc 9 (formula cells carry their cached result).
    assert_eq!(count(&ds, rdf::TYPE, raw::VALUE), 18);
    assert_eq!(count(&ds, rdf::TYPE, raw::NULL_VALUE), 0);

    let texts = |p: NamedNodeRef<'_>| -> Vec<String> {
        let mut v: Vec<String> = ds
            .iter()
            .filter(|q| q.predicate == p)
            .filter_map(|q| {
                match q.object {
                    oxrdf::TermRef::Literal(l) => Some(l.value().to_owned()),
                    _ => None,
                }
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(texts(raw::FORMULA), ["A2*B2", "A3*B3", "A4*B4"]);
    // 3 cells + 1 column-level rule, all the same relative form.
    assert_eq!(texts(raw::FORMULA_R1C1), ["RC[-2]*RC[-1]"; 4]);

    let literals: Vec<_> = ds
        .iter()
        .filter(|q| q.predicate == raw::LITERAL_VALUE)
        .filter_map(|q| {
            match q.object {
                oxrdf::TermRef::Literal(l) => {
                    Some((l.value().to_owned(), l.datatype().into_owned()))
                },
                _ => None,
            }
        })
        .collect();
    assert!(
        literals.contains(&("00123".into(), xsd::STRING.into_owned())),
        "no guessing"
    );
    assert!(
        literals.contains(&("1".into(), xsd::STRING.into_owned())),
        "BOM stripped from header only"
    );
    assert!(
        literals.contains(&("42".into(), xsd::DOUBLE.into_owned())),
        "{literals:?}"
    );
    assert!(literals.contains(&("true".into(), xsd::BOOLEAN.into_owned())));
}

#[tokio::test]
async fn canonical_ntriples_is_sorted_and_deterministic() {
    let input = tempfile::tempdir().unwrap();
    fixtures(input.path());
    let out_dir = tempfile::tempdir().unwrap();
    let a = out_dir.path().join("a.nt");
    let b = out_dir.path().join("b.nt");
    run(input.path(), &a, true).await;
    run(input.path(), &b, true).await;
    // Everything except the run (a UUIDv7 event) must be identical.
    let strip_run = |p: &Path| -> Vec<String> {
        let text = std::fs::read_to_string(p).unwrap();
        let run = text
            .lines()
            .find(|l| l.ends_with("<http://www.w3.org/ns/prov#Activity> ."))
            .and_then(|l| l.split_whitespace().next())
            .unwrap()
            .to_owned();
        text.lines()
            .filter(|l| !l.contains(&run))
            .map(str::to_owned)
            .collect()
    };
    let (la, lb) = (strip_run(&a), strip_run(&b));
    assert!(!la.is_empty());
    assert!(
        la.windows(2).all(|w| w[0] < w[1]),
        "sorted and unique"
    );
    assert_eq!(la, lb, "same input, same canonical source graphs");
}

#[tokio::test]
async fn profile_states_inferred_datatypes_on_columns() {
    let input = tempfile::tempdir().unwrap();
    let mut csv = String::from("when,amount,code\n");
    for d in 1..=20 {
        csv.push_str(&format!("2026-03-{d:02},{d}.5,00{d}\n"));
    }
    csv.push_str("unknown,,X\n");
    std::fs::write(input.path().join("ledger.csv"), csv).unwrap();
    let out_dir = tempfile::tempdir().unwrap();
    let out = out_dir.path().join("p.nq");
    Triplify::new(
        vec![input.path().to_owned()],
        Output::file(out.clone(), false).unwrap(),
    )
    .with_profile(true)
    .run()
    .await
    .unwrap();
    let ds = load(&std::fs::read(&out).unwrap(), RdfFormat::NQuads);
    // `when`: 20 of 21 are dates (95.2%); `amount`: 20 decimals; `code`:
    // leading zeros are identifiers, so no datatype.
    assert_eq!(count(&ds, raw::INFERRED_DATATYPE, xsd::DATE), 1);
    assert_eq!(
        count(&ds, raw::INFERRED_DATATYPE, xsd::DECIMAL),
        1
    );
    assert_eq!(
        ds.iter()
            .filter(|q| q.predicate == raw::INFERRED_DATATYPE)
            .count(),
        2
    );
    let values = |p: NamedNodeRef<'_>| -> Vec<String> {
        let mut v: Vec<String> = ds
            .iter()
            .filter(|q| q.predicate == p)
            .filter_map(|q| {
                match q.object {
                    oxrdf::TermRef::Literal(l) => Some(l.value().to_owned()),
                    _ => None,
                }
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(values(raw::CONFORMING_COUNT), ["20", "20"]);
    assert_eq!(values(raw::COUNT), ["20", "21", "21"]);
    // Values keep their source literals: still plain strings.
    assert!(ds.iter().all(|q| q.predicate != raw::LITERAL_VALUE
        || matches!(q.object, oxrdf::TermRef::Literal(l) if l.datatype() == xsd::STRING)));
}

#[tokio::test]
async fn no_header_makes_the_first_row_data() {
    use triplify::emit::Header;
    let input = tempfile::tempdir().unwrap();
    std::fs::write(input.path().join("plain.csv"), "1,Alice\n2,Bob\n").unwrap();
    let mut wb = rust_xlsxwriter::Workbook::new();
    let ws = wb.add_worksheet();
    ws.write_number(0, 0, 10).unwrap();
    ws.write_number(1, 0, 20).unwrap();
    wb.save(input.path().join("plain.xlsx")).unwrap();

    let run = |header: Header| {
        let dir = input.path().to_owned();
        async move {
            let out_dir = tempfile::tempdir().unwrap();
            let out = out_dir.path().join("o.nq");
            Triplify::new(
                vec![dir],
                Output::file(out.clone(), false).unwrap(),
            )
            .with_header(header)
            .run()
            .await
            .unwrap();
            load(&std::fs::read(&out).unwrap(), RdfFormat::NQuads)
        }
    };
    let absent = run(Header::Absent).await;
    // CSV: 2 rows × 2 values; XLSX: 2 rows × 1 value.
    assert_eq!(count(&absent, rdf::TYPE, raw::ROW), 4);
    assert_eq!(count(&absent, rdf::TYPE, raw::VALUE), 6);
    assert_eq!(
        absent.iter().filter(|q| q.predicate == rdfs::LABEL).count(),
        1,
        "only the worksheet's own name; columns have no labels"
    );
    let numbers: Vec<String> = {
        let mut v: Vec<String> = absent
            .iter()
            .filter(|q| q.predicate == raw::VIEW_ROW_NUMBER)
            .map(|q| q.object.to_string())
            .collect();
        v.sort();
        v
    };
    assert!(
        numbers[0].starts_with("\"0\""),
        "rows count from 0: {numbers:?}"
    );

    let first_row = run(Header::FirstRow).await;
    assert_eq!(count(&first_row, rdf::TYPE, raw::ROW), 2);
    assert_eq!(count(&first_row, rdf::TYPE, raw::VALUE), 3);
}
