//! Format readers. Each reader walks its input once, in physical order, and
//! feeds cells into a [`TableBuilder`]; nothing is mapped or interpreted.

mod csv;
mod parquet;
mod spreadsheet;

use {
    crate::{
        emit::{CellValue, Header, SourceEmitter, ViewEmitter},
        formula::{Formula, R1C1},
        iri::{ColumnNumber, ContentDigest, RowNumber},
        s3::{S3Store, S3Url},
        source::{Origin, Source, SourceKind},
    },
    anyhow::Context as _,
    std::{
        collections::{BTreeMap, BTreeSet},
        sync::Arc,
    },
    tokio::io::AsyncRead,
};

/// Read `source` completely, emitting into `emitter`.
pub async fn read(
    source: &Source,
    emitter: SourceEmitter,
    store: &Arc<S3Store>,
) -> anyhow::Result<SourceEmitter> {
    let delimiter = match source.kind() {
        SourceKind::Tsv => b'\t',
        _ => b',',
    };
    match (source.kind(), source.origin()) {
        (SourceKind::Csv | SourceKind::Tsv, Origin::Local { path, .. }) => {
            let file = tokio::fs::File::open(path).await?;
            csv::read(file, delimiter, emitter).await
        },
        (SourceKind::Csv | SourceKind::Tsv, Origin::S3 { url, meta }) => {
            let body = store.get(url, meta, None).await?.into_async_read();
            let body = Verified::new(body, source.digest());
            csv::read(body, delimiter, emitter).await
        },
        (SourceKind::Parquet, Origin::Local { path, .. }) => {
            let file = std::fs::File::open(path)
                .with_context(|| format!("cannot open {}", path.display()))?;
            parquet::read(file, emitter).await
        },
        (SourceKind::Parquet, Origin::S3 { url, meta }) => {
            let chunks = parquet::S3Chunks::new(
                Arc::clone(store),
                url.clone(),
                (**meta).clone(),
            );
            parquet::read(chunks, emitter).await
        },
        (
            SourceKind::Xlsx | SourceKind::Xls | SourceKind::Ods,
            Origin::Local { path, .. },
        ) => {
            spreadsheet::read(spreadsheet::Input::Path(path.clone()), emitter)
                .await
        },
        (
            SourceKind::Xlsx | SourceKind::Xls | SourceKind::Ods,
            Origin::S3 { url, meta },
        ) => {
            // A workbook is a zip whose index is at its end: buffer it.
            let bytes = store
                .get(url, meta, None)
                .await?
                .collect()
                .await?
                .into_bytes();
            verify(&bytes, source.digest(), url)?;
            spreadsheet::read(spreadsheet::Input::Bytes(bytes), emitter).await
        },
    }
}

/// Re-hashes bytes as they are consumed and fails at the end if they are
/// not the bytes the source's IRIs were minted from.
struct Verified<R> {
    inner:    R,
    hasher:   Option<sha2::Sha256>,
    expected: ContentDigest,
}

impl<R> Verified<R> {
    fn new(inner: R, expected: ContentDigest) -> Self {
        use sha2::Digest as _;
        Self { inner, hasher: Some(sha2::Sha256::new()), expected }
    }
}

/// Fail unless `bytes` are the bytes the source's IRIs were minted from.
fn verify(
    bytes: &[u8],
    expected: ContentDigest,
    what: &S3Url,
) -> anyhow::Result<()> {
    use sha2::Digest as _;
    let got = ContentDigest::new(sha2::Sha256::digest(bytes).into());
    anyhow::ensure!(
        got == expected,
        "{what}: content does not match its SHA-256 ({expected})"
    );
    Ok(())
}

impl<R: AsyncRead + Unpin> AsyncRead for Verified<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use {sha2::Digest as _, std::task::Poll};
        let before = buf.filled().len();
        let this = &mut *self;
        match std::pin::Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let new = &buf.filled()[before..];
                if !new.is_empty() {
                    if let Some(h) = this.hasher.as_mut() {
                        h.update(new);
                    }
                } else if let Some(h) = this.hasher.take() {
                    let got = ContentDigest::new(h.finalize().into());
                    if got != this.expected {
                        return Poll::Ready(Err(std::io::Error::other(
                            format!(
                                "content does not match its SHA-256 ({})",
                                this.expected
                            ),
                        )));
                    }
                }
                Poll::Ready(Ok(()))
            },
            other => other,
        }
    }
}

/// Turns a row-major stream of `(physical row, physical column, cell)` into
/// raw columns, rows and values. The first physical row seen is the header.
pub(crate) struct TableBuilder<'v, 'e> {
    header_mode:     Header,
    view:            &'v mut ViewEmitter<'e>,
    header_row:      Option<u64>,
    header:          BTreeMap<u32, String>,
    header_done:     bool,
    current_row:     Option<u64>,
    column_formulas: BTreeMap<u32, BTreeSet<R1C1>>,
}

impl<'v, 'e> TableBuilder<'v, 'e> {
    pub fn new(view: &'v mut ViewEmitter<'e>, header: Header) -> Self {
        Self {
            view,
            header_mode: header,
            header_row: None,
            header: BTreeMap::new(),
            // Without a header row there is nothing to wait for.
            header_done: header == Header::Absent,
            current_row: None,
            column_formulas: BTreeMap::new(),
        }
    }

    pub fn view(&mut self) -> &mut ViewEmitter<'e> { self.view }

    /// `row` and `column` are 0-based physical positions.
    pub fn cell(&mut self, row: u64, column: u32, cell: CellValue) {
        self.put(row, column, cell, None);
    }

    /// A cell computed by `formula`; `cached` is the value the workbook
    /// stored for it, if any.
    pub fn formula_cell(
        &mut self,
        row: u64,
        column: u32,
        cached: Option<CellValue>,
        formula: Formula,
    ) {
        let cell = cached.unwrap_or(CellValue::Null { source_text: None });
        self.put(row, column, cell, Some(formula));
    }

    fn put(
        &mut self,
        row: u64,
        column: u32,
        cell: CellValue,
        formula: Option<Formula>,
    ) {
        // The first physical row seen: the header row, or data row 0.
        let first = *self.header_row.get_or_insert(row);
        let physical = (row, column);
        let column = ColumnNumber(column + 1);
        let row = match self.header_mode {
            Header::FirstRow => {
                if row <= first {
                    if row == first {
                        self.header.insert(column.0, cell.header_text());
                    }
                    return;
                }
                self.finish_header();
                RowNumber(row - first - 1)
            },
            Header::Absent => {
                if row < first {
                    return;
                }
                RowNumber(row - first)
            },
        };
        if self.current_row != Some(row.0) {
            self.current_row = Some(row.0);
            self.view.row(row);
        }
        self.view.value(row, column, &cell);
        if let Some(formula) = formula {
            let r1c1 = formula.to_r1c1(physical.0, physical.1);
            self.view.formula(row, column, &formula, &r1c1);
            let forms = self.column_formulas.entry(column.0).or_default();
            if forms.len() < 2 {
                forms.insert(r1c1);
            }
        }
    }

    fn finish_header(&mut self) {
        if self.header_done {
            return;
        }
        self.header_done = true;
        let max = self.header.keys().next_back().copied().unwrap_or(0);
        for c in 1..=max {
            self.view.column(
                ColumnNumber(c),
                self.header.get(&c).map(String::as_str),
            );
        }
    }

    pub fn finish(mut self) {
        self.finish_header();
        self.view.finish_profiles();
        for (column, forms) in std::mem::take(&mut self.column_formulas) {
            if let [single] = forms.into_iter().collect::<Vec<_>>().as_slice() {
                self.view.column_formula(ColumnNumber(column), single);
            }
        }
    }
}

impl CellValue {
    fn header_text(&self) -> String {
        match self {
            Self::Text(s) | Self::Duration(s) => s.trim().to_owned(),
            Self::Integer(i) => i.to_string(),
            Self::Double(d) => d.to_string(),
            Self::Boolean(b) => b.to_string(),
            Self::DateTime(dt) => dt.to_string(),
            Self::Date(d) => d.to_string(),
            Self::DateTimeUtc(dt) => dt.to_rfc3339(),
            Self::Decimal(s) => s.clone(),
            Self::Time(t) => t.to_string(),
            Self::Binary(b) => hex::encode_upper(b),
            Self::Null { source_text } => {
                source_text.clone().unwrap_or_default()
            },
        }
    }
}
