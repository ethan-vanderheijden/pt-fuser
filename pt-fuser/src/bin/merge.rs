use clap::Parser;
use pt_fuser::{merge, trace::Trace};

#[derive(Parser)]
#[command(about = "Combines multiple pt-fuser traces into a single \"averaged\" trace")]
struct Cli {
    input: Vec<String>,
    #[clap(
        long,
        default_value_t = false,
        help = "Whether the input trace files are gzipped"
    )]
    gzip: bool,
    output: String,
}

fn main() {
    let cli = Cli::parse();

    let mut traces = Vec::new();
    for input in cli.input {
        let trace_data = std::fs::read(input).expect("Failed to read pt-fuser trace file");
        let trace = Trace::bin_deserialize(&trace_data, cli.gzip)
            .expect("pt-fuser trace file is malformed");
        traces.push(trace);
    }

    let traces_ref = traces.iter().collect::<Vec<&Trace>>();
    let result = merge::merge_traces(&traces_ref);
    let result_data = result
        .bin_serialize(true)
        .expect("Failed to serialize merge trace");
    std::fs::write(cli.output, result_data).expect("Failed to write merged trace to file");
}
