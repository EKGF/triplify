//! Generate deterministic benchmark inputs.
//!
//! ```sh
//! cargo run --release --example bench_data -- <out-dir> [scale]
//! ```
//!
//! `scale` 1.0 (default) writes:
//! - `wide.csv`: 5M rows × 10 mixed columns (~400 MB);
//! - `book.xlsx`: 1M rows × 8 columns, one filled-down formula column;
//! - `table.parquet`: 10M rows × 8 typed columns, Zstd;
//! - `many/`: 2,000 small CSV files in a git repository.

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
    parquet::{
        arrow::ArrowWriter,
        basic::{Compression, ZstdLevel},
        file::properties::WriterProperties,
    },
    std::{
        fs,
        io::{BufWriter, Write},
        path::Path,
        sync::Arc,
    },
};

/// Small, fast, deterministic PRNG (SplitMix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 { self.next() % n }
}

const WORDS: &[&str] = &[
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
    "india", "juliet", "kilo", "lima", "mike", "november", "oscar", "papa",
];

fn scaled(n: u64, scale: f64) -> u64 { ((n as f64) * scale).max(1.0) as u64 }

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let out = args.next().ok_or("usage: bench_data <out-dir> [scale]")?;
    let scale: f64 = args.next().map_or(Ok(1.0), |s| s.parse())?;
    let out = Path::new(&out);
    fs::create_dir_all(out)?;
    csv(&out.join("wide.csv"), scaled(5_000_000, scale))?;
    xlsx(&out.join("book.xlsx"), scaled(1_000_000, scale))?;
    parquet(
        &out.join("table.parquet"),
        scaled(10_000_000, scale),
    )?;
    many(&out.join("many"), scaled(2_000, scale))?;
    Ok(())
}

fn csv(path: &Path, rows: u64) -> std::io::Result<()> {
    let mut rng = Rng(1);
    let mut w = BufWriter::with_capacity(1 << 20, fs::File::create(path)?);
    writeln!(
        w,
        "id,code,name,city,amount,qty,ratio,day,at,active"
    )?;
    for i in 0..rows {
        let day = 1 + rng.below(28);
        let null = rng.below(50) == 0;
        writeln!(
            w,
            "{i},{:06},{} \
             {},{},{}.{:02},{},{:.4},2026-{:02}-{day:02},2026-01-{day:02} \
             {:02}:{:02}:00,{}",
            rng.below(1_000_000),
            WORDS[rng.below(16) as usize],
            WORDS[rng.below(16) as usize],
            if null {
                ""
            } else {
                WORDS[rng.below(16) as usize]
            },
            rng.below(10_000),
            rng.below(100),
            rng.below(1000),
            rng.next() as f64 / u64::MAX as f64,
            1 + rng.below(12),
            rng.below(24),
            rng.below(60),
            rng.below(2) == 0,
        )?;
    }
    w.flush()
}

fn xlsx(path: &Path, rows: u64) -> Result<(), Box<dyn std::error::Error>> {
    let mut rng = Rng(2);
    let mut wb = rust_xlsxwriter::Workbook::new();
    let ws = wb.add_worksheet_with_constant_memory()?;
    ws.set_name("Data")?;
    for (c, h) in [
        "id", "name", "qty", "price", "total", "day", "active", "note",
    ]
    .iter()
    .enumerate()
    {
        ws.write_string(0, c as u16, *h)?;
    }
    let date = rust_xlsxwriter::Format::new().set_num_format("yyyy-mm-dd");
    for r in 1..=rows as u32 {
        let qty = rng.below(100) as f64;
        let price = rng.below(10_000) as f64 / 100.0;
        ws.write_number(r, 0, f64::from(r))?;
        ws.write_string(r, 1, WORDS[rng.below(16) as usize])?;
        ws.write_number(r, 2, qty)?;
        ws.write_number(r, 3, price)?;
        ws.write_formula(r, 4, format!("=C{0}*D{0}", r + 1).as_str())?;
        let d = rust_xlsxwriter::ExcelDateTime::from_ymd(
            2026,
            1 + rng.below(12) as u8,
            1 + rng.below(28) as u8,
        )?;
        ws.write_datetime_with_format(r, 5, &d, &date)?;
        ws.write_boolean(r, 6, rng.below(2) == 0)?;
        if rng.below(10) == 0 {
            ws.write_string(r, 7, "check")?;
        }
    }
    wb.save(path)?;
    Ok(())
}

fn parquet(path: &Path, rows: u64) -> Result<(), Box<dyn std::error::Error>> {
    const BATCH: u64 = 100_000;
    let mut rng = Rng(3);
    let props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(3)?))
        .build();
    let mut writer: Option<ArrowWriter<fs::File>> = None;
    let mut start = 0;
    while start < rows {
        let n = BATCH.min(rows - start) as usize;
        let ids: Vec<i64> = (start as i64..start as i64 + n as i64).collect();
        let names: Vec<Option<&str>> = (0..n)
            .map(|_| {
                (rng.below(50) != 0).then(|| WORDS[rng.below(16) as usize])
            })
            .collect();
        let columns: Vec<(&str, ArrayRef)> = vec![
            ("id", Arc::new(Int64Array::from(ids))),
            ("name", Arc::new(StringArray::from(names))),
            (
                "amount",
                Arc::new(
                    Decimal128Array::from_iter_values(
                        (0..n).map(|_| rng.below(1_000_000) as i128),
                    )
                    .with_precision_and_scale(12, 2)?,
                ),
            ),
            (
                "ratio",
                Arc::new(Float64Array::from_iter_values(
                    (0..n).map(|_| rng.next() as f64 / u64::MAX as f64),
                )),
            ),
            (
                "active",
                Arc::new(BooleanArray::from_iter(
                    (0..n).map(|_| Some(rng.below(2) == 0)),
                )),
            ),
            (
                "day",
                Arc::new(Date32Array::from_iter_values(
                    (0..n).map(|_| 20_000 + rng.below(1000) as i32),
                )),
            ),
            (
                "at",
                Arc::new(
                    TimestampMicrosecondArray::from_iter_values((0..n).map(
                        |_| 1_760_000_000_000_000 + rng.below(1 << 40) as i64,
                    ))
                    .with_timezone("UTC"),
                ),
            ),
            (
                "qty",
                Arc::new(Int64Array::from_iter_values(
                    (0..n).map(|_| rng.below(100) as i64),
                )),
            ),
        ];
        let batch = RecordBatch::try_from_iter(columns)?;
        let w = match &mut writer {
            Some(w) => w,
            None => {
                writer.insert(ArrowWriter::try_new(
                    fs::File::create(path)?,
                    batch.schema(),
                    Some(props.clone()),
                )?)
            },
        };
        w.write(&batch)?;
        start += n as u64;
    }
    if let Some(w) = writer {
        w.close()?;
    }
    Ok(())
}

fn many(dir: &Path, files: u64) -> Result<(), Box<dyn std::error::Error>> {
    let mut rng = Rng(4);
    fs::create_dir_all(dir)?;
    for f in 0..files {
        let sub = dir.join(format!("part-{:03}", f / 100));
        fs::create_dir_all(&sub)?;
        let mut w = BufWriter::new(fs::File::create(
            sub.join(format!("file-{f:05}.csv")),
        )?);
        writeln!(w, "id,name,value")?;
        for i in 0..50 {
            writeln!(
                w,
                "{i},{},{}",
                WORDS[rng.below(16) as usize],
                rng.below(1000)
            )?;
        }
        w.flush()?;
    }
    let status = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()?;
    if !status.success() {
        return Err("git init failed".into());
    }
    for args in [
        &["add", "-A"][..],
        &[
            "-c",
            "user.name=bench",
            "-c",
            "user.email=bench@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "bench data",
        ][..],
    ] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()?;
        if !status.success() {
            return Err(format!("git {args:?} failed").into());
        }
    }
    Ok(())
}
