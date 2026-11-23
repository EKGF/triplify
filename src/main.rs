use {
    clap::Parser,
    triplify::{Triplify, location::Location, writer::Output},
};

/// Turn every CSV/TSV/XLSX/XLS/ODS/Parquet file under the given paths or S3
/// prefixes into raw RDF.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Files, directories (walked, honouring .gitignore), S3 objects
    /// (s3://bucket/key) or S3 prefixes (s3://bucket/prefix/).
    #[arg(default_value = ".")]
    paths: Vec<Location>,

    /// Output file or S3 object: .nq, .nt (graphs merged) or .trig,
    /// optionally .gz. Defaults to N-Quads on stdout.
    #[arg(short, long)]
    output: Option<Location>,

    /// Emit RDFC-1.0 canonical N-Quads/N-Triples (buffers the dataset in
    /// memory).
    #[arg(long)]
    canonical: bool,

    /// Profile every column: exact value/null/distinct counts, most
    /// frequent value, an inferred datatype with its conforming count, and
    /// min/max/mean/std/percentiles. Costs memory per column and time.
    #[arg(long)]
    profile: bool,

    #[command(flatten)]
    verbose: clap_verbosity_flag::Verbosity<clap_verbosity_flag::WarnLevel>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new(
                        cli.verbose
                            .tracing_level()
                            .map_or("off", |l| l.as_str()),
                    )
                }),
        )
        .without_time()
        .with_target(false)
        .init();
    let output = match cli.output {
        Some(location) => Output::to(location, cli.canonical)?,
        None => Output::stdout(cli.canonical),
    };
    let summary = Triplify::new(cli.paths, output)
        .with_profile(cli.profile)
        .run()
        .await?;
    tracing::info!(?summary, "done");
    anyhow::ensure!(
        summary.failed == 0,
        "{} source(s) failed",
        summary.failed
    );
    Ok(())
}
