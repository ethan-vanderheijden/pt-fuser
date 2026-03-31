use std::process::ExitCode;

use clap::Parser;
use pt_fuser::{merge, trace::Trace};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use tracing::{Level, info};

#[derive(Parser)]
#[command(about = "Combines multiple pt-fuser traces into a single \"averaged\" trace")]
struct Cli {
    #[clap(
        long,
        default_value_t = false,
        help = "Whether the input trace files are gzipped"
    )]
    gzip: bool,
    output: String,
    input: Vec<String>,
}

fn main() -> ExitCode {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let cli = Cli::parse();

    if cli.input.len() < 2 {
        eprintln!("At least two input trace files are required for merging");
        return ExitCode::FAILURE;
    }

    info!("Reading files...");

    let traces = cli
        .input
        .par_iter()
        .map(|input| {
            let trace_data = std::fs::read(input).expect("Failed to read pt-fuser trace file");
            Trace::bin_deserialize(&trace_data, cli.gzip).expect("pt-fuser trace file is malformed")
        })
        .collect::<Vec<Trace>>();

    let traces_ref = traces.iter().collect::<Vec<&Trace>>();
    let result = merge::merge_traces(&traces_ref);
    let result_data = result
        .bin_serialize(true)
        .expect("Failed to serialize merge trace");
    std::fs::write(cli.output, result_data).expect("Failed to write merged trace to file");

    ExitCode::SUCCESS
}
