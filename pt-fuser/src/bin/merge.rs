use std::{
    fs::File,
    io::{BufReader, BufWriter},
    process::ExitCode,
};

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
        help = "Whether the input trace files are compressed (zstd)"
    )]
    compressed: bool,
    #[clap(
        long,
        default_value_t = false,
        help = "Record raw data of the merging algorithm into the trace as an annotation"
    )]
    record_raw: bool,
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
            let trace_data = File::open(input).expect("Failed to read pt-fuser trace file");
            let mut trace_data = BufReader::with_capacity(64 * 1024, trace_data);
            Trace::bin_deserialize(&mut trace_data, cli.compressed)
                .expect("pt-fuser trace file is malformed")
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
        merge::merge_traces(&traces_ref, Some(&input_files))
    } else {
        merge::merge_traces(&traces_ref, None)
    };
    let output_file = File::create(cli.output).expect("Failed to create output file");
    let mut output_file = BufWriter::with_capacity(64 * 1024, output_file);
    merged_trace
        .bin_serialize(&mut output_file, true)
        .expect("Failed to serialize merge trace");

    ExitCode::SUCCESS
}
