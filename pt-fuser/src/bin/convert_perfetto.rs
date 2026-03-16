use std::fs;

use clap::Parser;
use pt_fuser::{perfetto, trace::Trace};

#[derive(Parser)]
#[command(about="Converts a trace from pt-fuser representation to a Perfetto trace")]
struct Cli {
    input: String,
    #[clap(long, default_value_t = false, help = "Whether the input trace file is gzipped")]
    gzip: bool,
    output: String,
}

fn main() {
    let cli = Cli::parse();

    let trace_data = fs::read(cli.input).expect("Failed to read pt-fuser trace file.");
    let trace = Trace::bin_deserialize(&trace_data, cli.gzip).expect("pt-fuser trace file is malformed.");
    let perfetto_data = perfetto::convert_to_perfetto(&trace);

    fs::write(cli.output, perfetto_data).expect("Failed to write perfetto trace to file.");
}
