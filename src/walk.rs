//! Fast, `.gitignore`-aware discovery of everything we can triplify.

use {
    crate::{
        location::Location,
        s3::{S3Store, S3Url},
        source::SourceKind,
    },
    std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
    },
    tokio::sync::mpsc,
    tracing::{error, warn},
};

/// A source worth triplifying.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub location: Location,
    pub kind:     SourceKind,
}

/// Walks local roots on a blocking thread and lists S3 prefixes, streaming
/// candidates as they are found.
#[derive(Debug, Clone)]
pub struct Walker {
    roots: Vec<PathBuf>,
    s3:    Vec<S3Url>,
}

impl Walker {
    pub fn new(roots: Vec<Location>) -> Self {
        let mut walker = Self { roots: Vec::new(), s3: Vec::new() };
        for root in roots {
            match root {
                Location::Local(p) => walker.roots.push(p),
                Location::S3(u) => walker.s3.push(u),
            }
        }
        walker
    }

    /// Start walking. `failed` counts roots that could not be listed.
    pub fn spawn(
        self,
        capacity: usize,
        store: Arc<S3Store>,
        failed: Arc<AtomicU64>,
    ) -> mpsc::Receiver<Candidate> {
        let (tx, rx) = mpsc::channel(capacity);
        for url in self.s3.clone() {
            let (tx, store, failed) = (
                tx.clone(),
                Arc::clone(&store),
                Arc::clone(&failed),
            );
            tokio::spawn(async move {
                match store.list(&url).await {
                    Ok(objects) => {
                        for object in objects {
                            let Some(kind) =
                                SourceKind::from_name(object.file_name())
                            else {
                                continue;
                            };
                            let location = Location::S3(object);
                            if tx
                                .send(Candidate { location, kind })
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    },
                    Err(e) => {
                        error!("{url}: {e:#}");
                        failed.fetch_add(1, Ordering::Relaxed);
                    },
                }
            });
        }
        if !self.roots.is_empty() {
            tokio::task::spawn_blocking(move || self.walk(&tx));
        }
        rx
    }

    fn walk(self, tx: &mpsc::Sender<Candidate>) {
        let Some((first, rest)) = self.roots.split_first() else {
            return
        };
        let mut builder = ignore::WalkBuilder::new(first);
        for root in rest {
            builder.add(root);
        }
        builder
            .hidden(true)
            .git_ignore(true)
            .git_exclude(true)
            .git_global(true)
            .parents(true);
        builder.build_parallel().run(|| {
            let tx = tx.clone();
            Box::new(move |entry| {
                use ignore::WalkState::{Continue, Quit};
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        warn!("{e}");
                        return Continue;
                    },
                };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return Continue;
                }
                let path = entry.into_path();
                // Skip Excel lock files like `~$Book.xlsx`.
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("~$"))
                {
                    return Continue;
                }
                let Some(kind) = SourceKind::from_path(&path) else {
                    return Continue
                };
                if tx
                    .blocking_send(Candidate {
                        location: Location::Local(path),
                        kind,
                    })
                    .is_err()
                {
                    Quit
                } else {
                    Continue
                }
            })
        });
    }
}
