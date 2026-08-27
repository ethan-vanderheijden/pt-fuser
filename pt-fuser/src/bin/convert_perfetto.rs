use std::fs;

use clap::Parser;
use pt_fuser::{
    perfetto::{self, PauseRenderOption},
    trace::Trace,
};

#[derive(Parser)]
#[command(about = "Converts a trace from pt-fuser representation to a Perfetto trace")]
struct Cli {
    input: String,
    #[clap(
        long,
        default_value_t = false,
        help = "Whether the input trace file is gzipped"
    )]
    gzip: bool,
    #[clap(
        long,
        default_value_t = PauseRenderOption::Gap,
        help = "Whether to render pauses in the trace as gaps or block named '--pause--'"
    )]
    render_pauses: PauseRenderOption,
    output: String,
}

fn main() {
    let cli = Cli::parse();

    println!("Reading trace file...");
    let trace_data = fs::read(cli.input).expect("Failed to read pt-fuser trace file");
    let trace =
        Trace::bin_deserialize(&trace_data, cli.gzip).expect("pt-fuser trace file is malformed");

    println!("Converting trace file... Ctrl-C to end conversion early.");

    perfetto::convert_to_perfetto(&trace, &cli.output, cli.render_pauses);
}
