//! XLSX / XLSM / XLSB / XLS / ODS via calamine. Cell types are the ones the
//! workbook stores; we do not reinterpret text.

use {
    super::TableBuilder,
    crate::{
        emit::{CellValue, SourceEmitter},
        formula::Formula,
        iri::ViewNumber,
    },
    anyhow::Context,
    calamine::{
        Data,
        DataRef,
        Reader,
        Sheets,
        open_workbook_auto,
        open_workbook_auto_from_rs,
    },
    std::{
        collections::BTreeMap,
        io::{Read, Seek},
        path::PathBuf,
    },
    tracing::warn,
};

type Formulas = BTreeMap<(u64, u32), Formula>;

/// All formulas of a sheet keyed by absolute 0-based position.
fn formulas<RS: Read + Seek>(
    workbook: &mut Sheets<RS>,
    sheet: &str,
) -> Formulas {
    let range = match workbook.worksheet_formula(sheet) {
        Ok(r) => r,
        Err(e) => {
            warn!("formulas of sheet {sheet:?} not readable: {e}");
            return Formulas::new();
        },
    };
    let (row0, col0) = range.start().unwrap_or((0, 0));
    range
        .used_cells()
        .filter_map(|(r, c, text)| {
            let pos = (u64::from(row0) + r as u64, col0 + c as u32);
            Formula::new(text.as_str()).map(|f| (pos, f))
        })
        .collect()
}

/// Emit a cell, with its formula when it has one.
fn put(
    table: &mut TableBuilder<'_, '_>,
    formulas: &mut Formulas,
    row: u64,
    col: u32,
    value: Option<CellValue>,
) {
    match (formulas.remove(&(row, col)), value) {
        (Some(f), cached) => table.formula_cell(row, col, cached, f),
        (None, Some(v)) => table.cell(row, col, v),
        (None, None) => {},
    }
}

/// A workbook on disk, or fully buffered (from S3).
pub enum Input {
    Path(PathBuf),
    Bytes(bytes::Bytes),
}

pub async fn read(
    input: Input,
    emitter: SourceEmitter,
) -> anyhow::Result<SourceEmitter> {
    tokio::task::spawn_blocking(move || {
        match input {
            Input::Path(path) => {
                let workbook =
                    open_workbook_auto(&path).with_context(|| {
                        format!("cannot open {}", path.display())
                    })?;
                read_blocking(workbook, emitter)
            },
            Input::Bytes(bytes) => {
                let workbook =
                    open_workbook_auto_from_rs(std::io::Cursor::new(bytes))
                        .context("cannot open workbook")?;
                read_blocking(workbook, emitter)
            },
        }
    })
    .await?
}

fn read_blocking<RS: Read + Seek>(
    mut workbook: Sheets<RS>,
    mut emitter: SourceEmitter,
) -> anyhow::Result<SourceEmitter> {
    let names = workbook.sheet_names();
    for (index, name) in names.iter().enumerate() {
        let number = ViewNumber(u32::try_from(index + 1)?);
        let mut view = emitter.view(number, Some(name));
        let mut formulas = formulas(&mut workbook, name);
        let header = view.header();
        let mut table = TableBuilder::new(&mut view, header);
        match &mut workbook {
            // XLSX: true streaming, one cell at a time.
            Sheets::Xlsx(xlsx) => {
                let mut cells = xlsx.worksheet_cells_reader(name)?;
                while let Some(cell) = cells.next_cell()? {
                    let (row, col) = cell.get_position();
                    let value = CellValue::from_data_ref(cell.get_value());
                    put(
                        &mut table,
                        &mut formulas,
                        u64::from(row),
                        col,
                        value,
                    );
                    table.view().out().flush_if_full_blocking()?;
                }
            },
            other => {
                let range = other.worksheet_range(name)?;
                let (row0, col0) = range.start().unwrap_or((0, 0));
                for (r, cells) in range.rows().enumerate() {
                    for (c, data) in cells.iter().enumerate() {
                        put(
                            &mut table,
                            &mut formulas,
                            u64::from(row0) + r as u64,
                            col0 + u32::try_from(c)?,
                            CellValue::from_data(data),
                        );
                    }
                    table.view().out().flush_if_full_blocking()?;
                }
            },
        }
        // Formula cells the value stream never reported (no cached value).
        for ((row, col), f) in std::mem::take(&mut formulas) {
            table.formula_cell(row, col, None, f);
        }
        table.finish();
    }
    emitter.out().flush_blocking()?;
    Ok(emitter)
}

impl CellValue {
    fn from_data(d: &Data) -> Option<Self> {
        Some(match d {
            Data::Empty => return None,
            Data::String(s) => Self::Text(s.clone()),
            Data::Int(i) => Self::Integer(*i),
            Data::Float(f) => Self::Double(*f),
            Data::Bool(b) => Self::Boolean(*b),
            Data::DateTime(dt) => Self::excel_date_time(dt),
            Data::DateTimeIso(s) => Self::iso_date_time(s),
            Data::DurationIso(s) => Self::Duration(s.clone()),
            Data::Error(e) => Self::Null { source_text: Some(e.to_string()) },
        })
    }

    fn from_data_ref(d: &DataRef<'_>) -> Option<Self> {
        Some(match d {
            DataRef::Empty => return None,
            DataRef::String(s) => Self::Text(s.clone()),
            DataRef::SharedString(s) => Self::Text((*s).to_owned()),
            DataRef::Int(i) => Self::Integer(*i),
            DataRef::Float(f) => Self::Double(*f),
            DataRef::Bool(b) => Self::Boolean(*b),
            DataRef::DateTime(dt) => Self::excel_date_time(dt),
            DataRef::DateTimeIso(s) => Self::iso_date_time(s),
            DataRef::DurationIso(s) => Self::Duration(s.clone()),
            DataRef::Error(e) => {
                Self::Null { source_text: Some(e.to_string()) }
            },
        })
    }

    fn excel_date_time(dt: &calamine::ExcelDateTime) -> Self {
        if dt.is_duration() {
            // Excel durations are fractional days; keep the serial number.
            return Self::Double(dt.as_f64());
        }
        match dt.as_datetime() {
            Some(ndt) => Self::DateTime(ndt),
            None => Self::Double(dt.as_f64()),
        }
    }

    fn iso_date_time(s: &str) -> Self {
        s.parse::<chrono::NaiveDateTime>()
            .map(Self::DateTime)
            .unwrap_or_else(|_| Self::Text(s.to_owned()))
    }
}
