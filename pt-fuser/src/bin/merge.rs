use std::{
    fs::File,
    io::{BufReader, BufWriter},
    process::ExitCode,
};

use clap::Parser;
use pt_fuser::{
    analysis::filter::{self, Filter},
    merge::{
        self,
        stats::{BasicStats, NoiseContribution, RawLatencies, StatsGenerator, StatsProvider},
    },
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
        default_value_t = true,
        help = "Record basic statistics (quartiles, mean, stddev) for each merged frame as an annotation"
    )]
    record_basic_stats: bool,
    #[clap(
        long,
        default_value_t = false,
        help = "Record raw latencies of the merging algorithm into the trace as an annotation"
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

    let cli = Cli::parse();

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
    let mut trace_names = cli.input.clone();
    for filter in &cli.filter {
        let bitmap = filter::filter_bitmap(&traces, filter);

        let mut bitmap_iter = bitmap.iter();
        traces.retain(|_| *bitmap_iter.next().unwrap());

        let mut bitmap_iter = bitmap.iter();
        trace_names.retain(|_| *bitmap_iter.next().unwrap());
    }

    let traces_ref = traces.iter().collect::<Vec<&Trace>>();
    let mut stats_gens: Vec<StatsGenerator> = Vec::new();

    let trace_names = trace_names.iter().map(|s| s.as_str());
    let traces_with_names = trace_names.zip(traces.iter()).collect::<Vec<_>>();
    if cli.record_basic_stats {
        if let Some(stat_gen) = BasicStats::prepare(&traces_with_names) {
            stats_gens.push(Box::new(stat_gen));
        }
    }
    if cli.record_raw {
        if let Some(stat_gen) = RawLatencies::prepare(&traces_with_names) {
            stats_gens.push(Box::new(stat_gen));
        }
    }
    if cli.record_noise_contribution {
        if let Some(stat_gen) = NoiseContribution::prepare(&traces_with_names) {
            stats_gens.push(Box::new(stat_gen));
        }
    }

    let merged_trace = merge::merge_traces(&traces_ref, stats_gens);
    let output_file = File::create(cli.output).expect("Failed to create output file");
    let mut output_file = BufWriter::with_capacity(64 * 1024, output_file);
    merged_trace
        .bin_serialize(&mut output_file, true)
        .expect("Failed to serialize merge trace");

    ExitCode::SUCCESS
}
