//! Orchestration: walk → fingerprint → read (concurrently) → one writer.

use {
    crate::{
        emit::{GraphEmitter, Header, QuadSender, SourceEmitter},
        iri::{ContentDigest, RunIri, SourceIris},
        location::Location,
        s3::S3Store,
        source::Source,
        vocab::{dcat, prov, rdf, xsd},
        walk::{Candidate, Walker},
        writer::Output,
    },
    futures::StreamExt,
    oxrdf::{Literal, NamedNode},
    std::{
        collections::HashSet,
        sync::{
            Arc,
            Mutex,
            atomic::{AtomicU64, Ordering},
        },
    },
    tokio_stream::wrappers::ReceiverStream,
    tracing::{info, warn},
};

/// Outcome of a run.
#[derive(Debug, Default, Clone, Copy)]
pub struct Summary {
    pub sources: u64,
    pub failed:  u64,
    pub skipped: u64,
    pub quads:   u64,
}

#[derive(Debug, Clone)]
pub struct Triplify {
    roots:       Vec<Location>,
    output:      Output,
    store:       Arc<S3Store>,
    concurrency: usize,
    profile:     bool,
    header:      Header,
}

enum Outcome {
    Done(NamedNode, NamedNode),
    Duplicate,
    Failed,
}

impl Triplify {
    /// Triplify every source under `roots` (local paths or `s3://` URLs).
    pub fn new(
        roots: impl IntoIterator<Item = impl Into<Location>>,
        output: Output,
    ) -> Self {
        let concurrency =
            std::thread::available_parallelism().map_or(4, |n| n.get());
        Self {
            roots: roots.into_iter().map(Into::into).collect(),
            output,
            store: Arc::new(S3Store::default()),
            concurrency,
            profile: false,
            header: Header::FirstRow,
        }
    }

    /// Whether CSV/TSV files and worksheets start with a header row.
    /// Parquet column names always come from its schema.
    pub fn with_header(mut self, header: Header) -> Self {
        self.header = header;
        self
    }

    /// Compute column profiles (counts, inferred datatype, statistics).
    pub fn with_profile(mut self, profile: bool) -> Self {
        self.profile = profile;
        self
    }

    pub async fn run(self) -> anyhow::Result<Summary> {
        let started = chrono::Utc::now();
        let run = RunIri::new();
        // producers -> [encoder pool] -> writer; bounded for backpressure.
        let (to_writer, rx) = flume::bounded(self.concurrency * 4);
        let (verdict, verdict_rx) = tokio::sync::oneshot::channel();
        let writer = tokio::spawn(self.output.clone().run(
            rx,
            Arc::clone(&self.store),
            verdict_rx,
        ));
        let (tx, encoders) = match self.output.encoding() {
            Some(encoding) => {
                let (tx, input) = flume::bounded(self.concurrency * 4);
                let pool =
                    encoding.spawn_pool(self.concurrency, input, to_writer);
                (tx, pool)
            },
            None => (to_writer, Vec::new()),
        };

        let seen = Arc::new(Mutex::new(HashSet::<ContentDigest>::new()));
        let unlisted = Arc::new(AtomicU64::new(0));
        let candidates =
            ReceiverStream::new(Walker::new(self.roots.clone()).spawn(
                1024,
                Arc::clone(&self.store),
                Arc::clone(&unlisted),
            ));
        let outcomes: Vec<Outcome> = candidates
            .map(|c| self.triplify(c, tx.clone(), Arc::clone(&seen)))
            .buffer_unordered(self.concurrency)
            .collect()
            .await;

        // A root that could not be listed is a failure, like a failed source.
        let mut summary = Summary {
            failed: unlisted.load(Ordering::Relaxed),
            ..Summary::default()
        };
        let mut produced = Vec::new();
        for o in outcomes {
            match o {
                Outcome::Done(source, graph) => {
                    summary.sources += 1;
                    produced.push((source, graph));
                },
                Outcome::Duplicate => summary.skipped += 1,
                Outcome::Failed => summary.failed += 1,
            }
        }
        self.describe_run(&run, started, &produced, tx).await?;
        // All senders are gone now; the encoders drain and stop.
        let panicked = tokio::task::spawn_blocking(move || {
            encoders
                .into_iter()
                .map(std::thread::JoinHandle::join)
                .filter(Result::is_err)
                .count()
        })
        .await?;
        // A lost batch must fail the run, not shorten the output silently.
        anyhow::ensure!(
            panicked == 0,
            "{panicked} encoder thread(s) panicked"
        );
        // Publish (an S3 output) only if every source succeeded.
        let _ = verdict.send(summary.failed == 0);
        summary.quads = writer.await??;
        Ok(summary)
    }

    async fn triplify(
        &self,
        candidate: Candidate,
        tx: QuadSender,
        seen: Arc<Mutex<HashSet<ContentDigest>>>,
    ) -> Outcome {
        let path = candidate.location.clone();
        match self.triplify_inner(candidate, tx, seen).await {
            Ok(Some(done)) => {
                info!("triplified {path}");
                Outcome::Done(done.0, done.1)
            },
            Ok(None) => Outcome::Duplicate,
            Err(e) => {
                warn!("{path}: {e:#}");
                Outcome::Failed
            },
        }
    }

    async fn triplify_inner(
        &self,
        candidate: Candidate,
        tx: QuadSender,
        seen: Arc<Mutex<HashSet<ContentDigest>>>,
    ) -> anyhow::Result<Option<(NamedNode, NamedNode)>> {
        let source =
            Source::open(candidate.location, candidate.kind, &self.store)
                .await?;
        let fresh = seen
            .lock()
            .map_err(|_| anyhow::anyhow!("poisoned"))?
            .insert(source.digest());
        if !fresh {
            info!(
                "{} has identical content to a file already triplified, \
                 skipping",
                source.display()
            );
            return Ok(None);
        }
        let iris = SourceIris::new(source.digest());
        let mut out = GraphEmitter::new(iris.graph(), tx);
        source.describe(&iris, &mut out);
        let emitter = crate::reader::read(
            &source,
            SourceEmitter::new(iris.clone(), out)
                .with_profile(self.profile)
                .with_header(self.header),
            &self.store,
        )
        .await?;
        emitter.into_inner().flush().await?;
        Ok(Some((iris.source(), iris.graph())))
    }

    /// PROV/DCAT for the run itself, in its own named graph.
    async fn describe_run(
        &self,
        run: &RunIri,
        started: chrono::DateTime<chrono::Utc>,
        produced: &[(NamedNode, NamedNode)],
        tx: QuadSender,
    ) -> anyhow::Result<()> {
        let mut out = GraphEmitter::new(run.graph().clone(), tx);
        let a = run.node().clone();
        let agent = RunIri::agent();
        let time = |t: chrono::DateTime<chrono::Utc>| {
            Literal::new_typed_literal(t.to_rfc3339(), xsd::DATE_TIME)
        };
        out.quad(a.clone(), rdf::TYPE, prov::ACTIVITY.into_owned());
        out.quad(a.clone(), prov::STARTED_AT_TIME, time(started));
        out.quad(
            a.clone(),
            prov::ENDED_AT_TIME,
            time(chrono::Utc::now()),
        );
        out.quad(
            a.clone(),
            prov::WAS_ASSOCIATED_WITH,
            agent.clone(),
        );
        out.quad(
            agent,
            rdf::TYPE,
            prov::SOFTWARE_AGENT.into_owned(),
        );
        for (source, graph) in produced {
            out.quad(a.clone(), prov::USED, source.clone());
            out.quad(
                graph.clone(),
                rdf::TYPE,
                dcat::DATASET.into_owned(),
            );
            out.quad(
                graph.clone(),
                prov::WAS_DERIVED_FROM,
                source.clone(),
            );
            out.quad(graph.clone(), prov::WAS_GENERATED_BY, a.clone());
        }
        out.flush().await?;
        Ok(())
    }
}
