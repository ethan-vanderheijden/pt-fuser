use std::process::ExitCode;

use clap::Parser;
use pt_fuser::{
    analysis::filter::{self, Filter},
    merge,
    trace::Trace,
};
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
    #[clap(
        long,
        default_value_t = false,
        help = "Record raw data of the merging algorithm into the trace as an annotation"
    )]
    record_raw: bool,
    #[clap(
        long,
        default_value_t = false,
        help = "Record noise contribution for each merged frame as an annotation"
    )]
    record_noise_contribution: bool,
    #[clap(long, help = Filter::HELP)]
    filter: Vec<Filter>,
    output: String,
    input: Vec<String>,
}

fn main() -> ExitCode {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let mut cli = Cli::parse();

    if cli.input.len() < 2 {
        eprintln!("At least two input trace files are required for merging");
        return ExitCode::FAILURE;
    }

    info!("Reading files...");

    let mut traces = cli
        .input
        .par_iter()
        .map(|input| {
            let trace_data = std::fs::read(input).expect("Failed to read pt-fuser trace file");
            Trace::bin_deserialize(&trace_data, cli.gzip).expect("pt-fuser trace file is malformed")
        })
        .collect::<Vec<Trace>>();

    if cli.filter.len() > 0 {
        info!("Filtering traces...");
    }
    for filter in &cli.filter {
        let bitmap = filter::filter_bitmap(&traces, filter);
        traces = traces
            .into_iter()
            .enumerate()
            .filter(|(i, _)| *bitmap.get(*i).unwrap_or(&false))
            .map(|(_, trace)| trace)
            .collect();
        cli.input = cli
            .input
            .into_iter()
            .enumerate()
            .filter(|(i, _)| *bitmap.get(*i).unwrap_or(&false))
            .map(|(_, input)| input)
            .collect();
    }

    let traces_ref = traces.iter().collect::<Vec<&Trace>>();

    let merged_trace = if cli.record_raw {
        let input_files = cli.input.iter().map(|s| s.as_str()).collect::<Vec<&str>>();
        merge::merge_traces(
            &traces_ref,
            Some(&input_files),
            cli.record_noise_contribution,
        )
    } else {
        merge::merge_traces(&traces_ref, None, cli.record_noise_contribution)
    };
    let result_data = merged_trace
        .bin_serialize(true)
        .expect("Failed to serialize merge trace");
    std::fs::write(cli.output, result_data).expect("Failed to write merged trace to file");

    ExitCode::SUCCESS
}
