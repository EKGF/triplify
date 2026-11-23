//! Apache Parquet: one view per file; the schema's top-level field names are
//! the header; values keep the datatype Parquet's types define. A Parquet
//! null is an explicit source null and becomes a `raw:NullValue`.
//!
//! Nested types (lists, structs, maps), intervals and durations are captured
//! as their Arrow display text until the raw model supports nesting.

use {
    super::TableBuilder,
    crate::{
        emit::{CellValue, SourceEmitter},
        iri::ViewNumber,
        s3::{S3ObjectMeta, S3Store, S3Url},
    },
    anyhow::Context,
    arrow_array::{
        Array,
        ArrayRef,
        RecordBatchReader,
        cast::AsArray,
        types::{
            Date32Type,
            Date64Type,
            Decimal32Type,
            Decimal64Type,
            Decimal128Type,
            Decimal256Type,
            Float16Type,
            Float32Type,
            Float64Type,
            Int8Type,
            Int16Type,
            Int32Type,
            Int64Type,
            Time32MillisecondType,
            Time32SecondType,
            Time64MicrosecondType,
            Time64NanosecondType,
            TimestampMicrosecondType,
            TimestampMillisecondType,
            TimestampNanosecondType,
            TimestampSecondType,
            UInt8Type,
            UInt16Type,
            UInt32Type,
            UInt64Type,
        },
    },
    arrow_cast::display::{ArrayFormatter, FormatOptions},
    arrow_schema::{DataType, TimeUnit},
    bytes::{Buf, Bytes},
    chrono::{NaiveDateTime, TimeZone, Utc},
    parquet::{
        arrow::arrow_reader::ParquetRecordBatchReaderBuilder,
        errors::ParquetError,
        file::reader::{ChunkReader, Length},
    },
    std::sync::Arc,
};

const BATCH_ROWS: usize = 8 * 1024;

pub async fn read<T: ChunkReader + 'static>(
    input: T,
    emitter: SourceEmitter,
) -> anyhow::Result<SourceEmitter> {
    tokio::task::spawn_blocking(move || read_blocking(input, emitter)).await?
}

fn read_blocking<T: ChunkReader + 'static>(
    input: T,
    mut emitter: SourceEmitter,
) -> anyhow::Result<SourceEmitter> {
    let reader = ParquetRecordBatchReaderBuilder::try_new(input)?
        .with_batch_size(BATCH_ROWS)
        .build()?;
    let schema = reader.schema();
    {
        let mut view = emitter.view(ViewNumber(1), None);
        let mut table = TableBuilder::new(&mut view);
        for (c, field) in schema.fields().iter().enumerate() {
            table.cell(
                0,
                u32::try_from(c)?,
                CellValue::Text(field.name().clone()),
            );
        }
        let mut row: u64 = 1;
        for batch in reader {
            let batch = batch?;
            let columns = batch
                .columns()
                .iter()
                .map(Column::new)
                .collect::<anyhow::Result<Vec<_>>>()?;
            for i in 0..batch.num_rows() {
                for (c, column) in columns.iter().enumerate() {
                    table.cell(row, u32::try_from(c)?, column.cell(i)?);
                }
                row += 1;
                table.view().out().flush_if_full_blocking()?;
            }
        }
        table.finish();
    }
    emitter.out().flush_blocking()?;
    Ok(emitter)
}

/// One Arrow column, normalised (dictionaries decoded) for cell access.
struct Column {
    array: ArrayRef,
}

impl Column {
    fn new(array: &ArrayRef) -> anyhow::Result<Self> {
        let array = match array.data_type() {
            DataType::Dictionary(_, value) => arrow_cast::cast(array, value)?,
            _ => array.clone(),
        };
        Ok(Self { array })
    }

    fn cell(&self, i: usize) -> anyhow::Result<CellValue> {
        let a = &self.array;
        if a.is_null(i) {
            return Ok(CellValue::Null { source_text: None });
        }
        Ok(match a.data_type() {
            DataType::Null => CellValue::Null { source_text: None },
            DataType::Boolean => CellValue::Boolean(a.as_boolean().value(i)),
            DataType::Int8 => int(a.as_primitive::<Int8Type>().value(i)),
            DataType::Int16 => int(a.as_primitive::<Int16Type>().value(i)),
            DataType::Int32 => int(a.as_primitive::<Int32Type>().value(i)),
            DataType::Int64 => {
                CellValue::Integer(a.as_primitive::<Int64Type>().value(i))
            },
            DataType::UInt8 => int(a.as_primitive::<UInt8Type>().value(i)),
            DataType::UInt16 => int(a.as_primitive::<UInt16Type>().value(i)),
            DataType::UInt32 => int(a.as_primitive::<UInt32Type>().value(i)),
            DataType::UInt64 => {
                let v = a.as_primitive::<UInt64Type>().value(i);
                i64::try_from(v).map_or_else(
                    |_| CellValue::Decimal(v.to_string()),
                    CellValue::Integer,
                )
            },
            DataType::Float16 => {
                CellValue::Double(f64::from(
                    a.as_primitive::<Float16Type>().value(i),
                ))
            },
            DataType::Float32 => {
                CellValue::Double(f64::from(
                    a.as_primitive::<Float32Type>().value(i),
                ))
            },
            DataType::Float64 => {
                CellValue::Double(a.as_primitive::<Float64Type>().value(i))
            },
            DataType::Utf8 => {
                CellValue::Text(a.as_string::<i32>().value(i).to_owned())
            },
            DataType::LargeUtf8 => {
                CellValue::Text(a.as_string::<i64>().value(i).to_owned())
            },
            DataType::Utf8View => {
                CellValue::Text(a.as_string_view().value(i).to_owned())
            },
            DataType::Binary => {
                CellValue::Binary(a.as_binary::<i32>().value(i).to_vec())
            },
            DataType::LargeBinary => {
                CellValue::Binary(a.as_binary::<i64>().value(i).to_vec())
            },
            DataType::BinaryView => {
                CellValue::Binary(a.as_binary_view().value(i).to_vec())
            },
            DataType::FixedSizeBinary(_) => {
                CellValue::Binary(a.as_fixed_size_binary().value(i).to_vec())
            },
            DataType::Date32 => {
                CellValue::Date(
                    a.as_primitive::<Date32Type>()
                        .value_as_date(i)
                        .context("Date32 out of range")?,
                )
            },
            DataType::Date64 => {
                CellValue::Date(
                    a.as_primitive::<Date64Type>()
                        .value_as_date(i)
                        .context("Date64 out of range")?,
                )
            },
            DataType::Timestamp(unit, tz) => {
                let naive = match unit {
                    TimeUnit::Second => {
                        a.as_primitive::<TimestampSecondType>()
                            .value_as_datetime(i)
                    },
                    TimeUnit::Millisecond => {
                        a.as_primitive::<TimestampMillisecondType>()
                            .value_as_datetime(i)
                    },
                    TimeUnit::Microsecond => {
                        a.as_primitive::<TimestampMicrosecondType>()
                            .value_as_datetime(i)
                    },
                    TimeUnit::Nanosecond => {
                        a.as_primitive::<TimestampNanosecondType>()
                            .value_as_datetime(i)
                    },
                }
                .context("timestamp out of range")?;
                timestamp(naive, tz.is_some())
            },
            DataType::Time32(TimeUnit::Second) => {
                time(a.as_primitive::<Time32SecondType>().value_as_time(i))?
            },
            DataType::Time32(_) => {
                time(
                    a.as_primitive::<Time32MillisecondType>().value_as_time(i),
                )?
            },
            DataType::Time64(TimeUnit::Nanosecond) => {
                time(a.as_primitive::<Time64NanosecondType>().value_as_time(i))?
            },
            DataType::Time64(_) => {
                time(
                    a.as_primitive::<Time64MicrosecondType>().value_as_time(i),
                )?
            },
            DataType::Decimal32(..) => {
                CellValue::Decimal(
                    a.as_primitive::<Decimal32Type>().value_as_string(i),
                )
            },
            DataType::Decimal64(..) => {
                CellValue::Decimal(
                    a.as_primitive::<Decimal64Type>().value_as_string(i),
                )
            },
            DataType::Decimal128(..) => {
                CellValue::Decimal(
                    a.as_primitive::<Decimal128Type>().value_as_string(i),
                )
            },
            DataType::Decimal256(..) => {
                CellValue::Decimal(
                    a.as_primitive::<Decimal256Type>().value_as_string(i),
                )
            },
            _ => CellValue::Text(self.display(i)?),
        })
    }

    fn display(&self, i: usize) -> anyhow::Result<String> {
        let options = FormatOptions::default();
        let formatter = ArrayFormatter::try_new(self.array.as_ref(), &options)?;
        Ok(formatter.value(i).try_to_string()?)
    }
}

fn int(v: impl Into<i64>) -> CellValue { CellValue::Integer(v.into()) }

/// Arrow stores timestamps as UTC instants when a timezone is declared, and
/// as zone-less wall-clock values otherwise.
fn timestamp(naive: NaiveDateTime, zoned: bool) -> CellValue {
    if zoned {
        CellValue::DateTimeUtc(Utc.from_utc_datetime(&naive))
    } else {
        CellValue::DateTime(naive)
    }
}

fn time(t: Option<chrono::NaiveTime>) -> anyhow::Result<CellValue> {
    Ok(CellValue::Time(t.context("time out of range")?))
}

/// A Parquet file in S3, read with ranged GETs: the footer first, then only
/// the column chunks of each row group. Runs on a blocking thread.
pub struct S3Chunks {
    store:  Arc<S3Store>,
    url:    S3Url,
    meta:   S3ObjectMeta,
    handle: tokio::runtime::Handle,
}

impl S3Chunks {
    pub fn new(store: Arc<S3Store>, url: S3Url, meta: S3ObjectMeta) -> Self {
        Self {
            store,
            url,
            meta,
            handle: tokio::runtime::Handle::current(),
        }
    }
}

impl Length for S3Chunks {
    fn len(&self) -> u64 { self.meta.size }
}

impl ChunkReader for S3Chunks {
    type T = bytes::buf::Reader<Bytes>;

    fn get_read(&self, start: u64) -> parquet::errors::Result<Self::T> {
        let length = usize::try_from(self.meta.size.saturating_sub(start))
            .map_err(|e| ParquetError::External(Box::new(e)))?;
        Ok(self.get_bytes(start, length)?.reader())
    }

    fn get_bytes(
        &self,
        start: u64,
        length: usize,
    ) -> parquet::errors::Result<Bytes> {
        if length == 0 {
            return Ok(Bytes::new());
        }
        let end = start + length as u64;
        self.handle
            .block_on(async {
                let body = self
                    .store
                    .get(&self.url, &self.meta, Some((start, end)))
                    .await?;
                anyhow::Ok(body.collect().await?.into_bytes())
            })
            .map_err(|e| ParquetError::External(e.into()))
    }
}
