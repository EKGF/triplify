//! The single writer task: serializes quad batches as they arrive.
//!
//! Format follows the output file name: `.nq`, `.nt`, `.trig`, each
//! optionally followed by `.gz`. `.nt` writes the union of all graphs.
//! With `canonical`, output is RDFC-1.0 canonical N-Quads/N-Triples; that
//! needs the whole dataset in memory, everything else streams.

use {
    crate::{emit::QuadBatch, encode::Encoding},
    anyhow::Context,
    async_compression::tokio::write::GzipEncoder,
    oxrdf::{
        Dataset,
        graph::{CanonicalizationAlgorithm, CanonicalizationHashAlgorithm},
    },
    oxrdfio::{RdfFormat, RdfSerializer},
    std::{path::PathBuf, pin::Pin},
    tokio::io::{AsyncWrite, AsyncWriteExt, BufWriter},
};

const PREFIXES: &[(&str, &str)] = &[
    ("raw", crate::vocab::raw::NS),
    ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
    ("prov", "http://www.w3.org/ns/prov#"),
    ("dcat", "http://www.w3.org/ns/dcat#"),
    ("dcterms", "http://purl.org/dc/terms/"),
    ("spdx", "http://spdx.org/rdf/terms#"),
];

type Sink = Pin<Box<dyn AsyncWrite + Send>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    NTriples,
    NQuads,
    TriG,
}

impl Syntax {
    fn format(self) -> RdfFormat {
        match self {
            Self::NTriples => RdfFormat::NTriples,
            Self::NQuads => RdfFormat::NQuads,
            Self::TriG => RdfFormat::TriG,
        }
    }

    fn keeps_graphs(self) -> bool { self != Self::NTriples }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Stdout,
    File(PathBuf),
}

/// Fully resolved output: where, which syntax, compressed, canonical.
#[derive(Debug, Clone)]
pub struct Output {
    target:    Target,
    syntax:    Syntax,
    gzip:      bool,
    canonical: bool,
}

impl Output {
    pub fn stdout(canonical: bool) -> Self {
        Self {
            target: Target::Stdout,
            syntax: Syntax::NQuads,
            gzip: false,
            canonical,
        }
    }

    pub fn file(path: PathBuf, canonical: bool) -> anyhow::Result<Self> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .context("output path has no file name")?
            .to_ascii_lowercase();
        let (stem, gzip) = match name.strip_suffix(".gz") {
            Some(stem) => (stem.to_owned(), true),
            None => (name, false),
        };
        let syntax = match stem.rsplit_once('.').map(|(_, e)| e) {
            Some("nq") => Syntax::NQuads,
            Some("nt") => Syntax::NTriples,
            Some("trig") => Syntax::TriG,
            other => {
                anyhow::bail!(
                    "unsupported output extension {other:?}; use .nq, .nt or \
                     .trig (optionally .gz)"
                )
            },
        };
        anyhow::ensure!(
            !canonical || syntax != Syntax::TriG,
            "RDFC-1.0 canonical output is N-Quads or N-Triples, not TriG"
        );
        Ok(Self {
            target: Target::File(path),
            syntax,
            gzip,
            canonical,
        })
    }

    async fn sink(&self) -> anyhow::Result<Sink> {
        let raw: Sink = match &self.target {
            // Unbuffered stdout costs one write system call per quad.
            Target::Stdout => {
                Box::pin(BufWriter::with_capacity(
                    1 << 20,
                    tokio::io::stdout(),
                ))
            },
            Target::File(p) => {
                Box::pin(BufWriter::with_capacity(
                    1 << 20,
                    tokio::fs::File::create(p).await.with_context(|| {
                        format!("cannot create {}", p.display())
                    })?,
                ))
            },
        };
        Ok(if self.gzip {
            Box::pin(GzipEncoder::new(raw))
        } else {
            raw
        })
    }

    /// The encoding producers apply themselves: line-based syntaxes stream
    /// pre-encoded bytes; TriG and canonical output need structured triples.
    pub fn encoding(&self) -> Option<Encoding> {
        match (self.syntax, self.canonical) {
            (_, true) | (Syntax::TriG, _) => None,
            (Syntax::NQuads, false) => Some(Encoding::NQuads),
            (Syntax::NTriples, false) => Some(Encoding::NTriples),
        }
    }

    /// Consume all batches; returns the number of quads received.
    pub async fn run(
        self,
        rx: flume::Receiver<QuadBatch>,
    ) -> anyhow::Result<u64> {
        if self.gzip && self.encoding().is_some() {
            return tokio::task::spawn_blocking(move || {
                self.write_encoded_parallel_gzip(&rx)
            })
            .await?;
        }
        let mut sink = self.sink().await?;
        let count = if self.canonical {
            self.write_canonical(&mut sink, rx).await?
        } else {
            self.write_streaming(&mut sink, rx).await?
        };
        sink.shutdown().await?;
        Ok(count)
    }

    async fn write_streaming(
        &self,
        sink: &mut Sink,
        rx: flume::Receiver<QuadBatch>,
    ) -> anyhow::Result<u64> {
        if self.encoding().is_some() {
            Self::write_encoded(sink, rx).await
        } else {
            self.write_serialized(sink, rx).await
        }
    }

    /// Line-based output: producers already encoded every batch, so the
    /// writer only moves bytes.
    async fn write_encoded(
        sink: &mut Sink,
        rx: flume::Receiver<QuadBatch>,
    ) -> anyhow::Result<u64> {
        let mut count = 0u64;
        while let Ok(batch) = rx.recv_async().await {
            let QuadBatch::Encoded { bytes, quads } = batch else {
                anyhow::bail!("structured batch sent to a line-based writer");
            };
            sink.write_all(&bytes).await?;
            count += quads;
        }
        Ok(count)
    }

    /// Gzipped line-based output, compressed on all cores into a single
    /// standard gzip member (blocks share their predecessor's dictionary,
    /// like pigz), so every gzip reader can decompress it.
    fn write_encoded_parallel_gzip(
        &self,
        rx: &flume::Receiver<QuadBatch>,
    ) -> anyhow::Result<u64> {
        use {
            gzp::{
                ZWriter,
                deflate::Gzip,
                par::compress::{ParCompress, ParCompressBuilder},
            },
            std::io::Write as _,
        };
        let out: Box<dyn std::io::Write + Send> = match &self.target {
            Target::Stdout => Box::new(std::io::stdout()),
            Target::File(p) => {
                Box::new(std::fs::File::create(p).with_context(|| {
                    format!("cannot create {}", p.display())
                })?)
            },
        };
        let threads =
            std::thread::available_parallelism().map_or(4, |n| n.get());
        // Level 6, as gzip and the single-threaded encoder default to.
        let mut gz: ParCompress<Gzip, _> = ParCompressBuilder::new()
            .compression_level(gzp::Compression::new(6))
            .num_threads(threads)
            .map_err(|e| anyhow::anyhow!("gzip threads: {e}"))?
            .from_writer(out);
        let mut count = 0u64;
        for batch in rx.iter() {
            let QuadBatch::Encoded { bytes, quads } = batch else {
                anyhow::bail!("structured batch sent to a line-based writer");
            };
            gz.write_all(&bytes)?;
            count += quads;
        }
        gz.finish().map_err(|e| anyhow::anyhow!("gzip: {e}"))?;
        Ok(count)
    }

    /// TriG: serialized here, where prefixes and graph blocks are known.
    async fn write_serialized(
        &self,
        sink: &mut Sink,
        rx: flume::Receiver<QuadBatch>,
    ) -> anyhow::Result<u64> {
        let mut ser = RdfSerializer::from_format(self.syntax.format());
        for (prefix, ns) in PREFIXES {
            ser = ser.with_prefix(*prefix, *ns)?;
        }
        let mut ser = ser.for_tokio_async_writer(sink);
        let mut count = 0u64;
        let keep = self.syntax.keeps_graphs();
        while let Ok(batch) = rx.recv_async().await {
            anyhow::ensure!(
                matches!(batch, QuadBatch::Triples { .. }),
                "encoded batch sent to a serializing writer"
            );
            count += batch.quad_count();
            for quad in batch.into_quads(keep) {
                ser.serialize_quad(&quad).await?;
            }
        }
        ser.finish().await?;
        Ok(count)
    }

    async fn write_canonical(
        &self,
        sink: &mut Sink,
        rx: flume::Receiver<QuadBatch>,
    ) -> anyhow::Result<u64> {
        let mut dataset = Dataset::new();
        let mut count = 0u64;
        let keep = self.syntax.keeps_graphs();
        while let Ok(batch) = rx.recv_async().await {
            count += batch.quad_count();
            for quad in batch.into_quads(keep) {
                dataset.insert(&quad);
            }
        }
        dataset.canonicalize(CanonicalizationAlgorithm::Rdfc10 {
            hash_algorithm: CanonicalizationHashAlgorithm::Sha256,
        });
        let mut lines: Vec<String> = dataset
            .iter()
            .map(|q| {
                let mut s = q.to_string();
                s.push_str(" .\n");
                s
            })
            .collect();
        lines.sort_unstable();
        for line in lines {
            sink.write_all(line.as_bytes()).await?;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        let o = Output::file("x/out.nt.gz".into(), false).unwrap();
        assert_eq!((o.syntax, o.gzip), (Syntax::NTriples, true));
        let o = Output::file("out.NQ".into(), true).unwrap();
        assert_eq!((o.syntax, o.gzip), (Syntax::NQuads, false));
        assert!(Output::file("out.trig".into(), true).is_err());
        assert!(Output::file("out.ttl".into(), false).is_err());
    }
}
