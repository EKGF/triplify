//! CSV / TSV: every field is an `xsd:string`, empty fields are absent.

use {
    super::TableBuilder,
    crate::{
        emit::{CellValue, SourceEmitter},
        iri::ViewNumber,
    },
    futures::StreamExt,
    tokio::io::AsyncRead,
};

pub async fn read(
    input: impl AsyncRead + Unpin + Send,
    delimiter: u8,
    mut emitter: SourceEmitter,
) -> anyhow::Result<SourceEmitter> {
    let mut reader = csv_async::AsyncReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .create_reader(input);
    let mut records = reader.byte_records();
    {
        let mut view = emitter.view(ViewNumber(1), None);
        let mut table = TableBuilder::new(&mut view);
        let mut row: u64 = 0;
        while let Some(record) = records.next().await {
            let record = record?;
            for (col, field) in record.iter().enumerate() {
                let field = if row == 0 && col == 0 {
                    field.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(field)
                } else {
                    field
                };
                if field.is_empty() {
                    continue;
                }
                let text = String::from_utf8_lossy(field).into_owned();
                table.cell(row, u32::try_from(col)?, CellValue::Text(text));
            }
            row += 1;
            table.view().out().flush_if_full().await?;
        }
        table.finish();
    }
    emitter.out().flush().await?;
    Ok(emitter)
}
