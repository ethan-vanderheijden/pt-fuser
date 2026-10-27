mod elf_image;
mod intel_pt;
mod perf_data;

use std::{
    fs::{self, File},
    io::BufWriter,
    path::Path,
    thread,
};

use clap::Parser;
use fallible_iterator::FallibleIterator;
use libipt::image::Image;
use pt_fuser::trace::builder::FrameCompletionOptions;
use regex::Regex;
use threadpool::ThreadPool;
use tracing::{Level, info};

#[derive(Parser)]
#[command(about = "Converts a trace from pt-fuser representation to a Perfetto trace")]
#[command(
    display_name = concat!("pt-fuser ", env!("CARGO_BIN_NAME")),
    version = pt_fuser::VERSION,
)]
pub struct Args {
    perf_file: String,
    symbol_regex: String,
    output_dir: String,
    #[clap(
        long,
        default_value_t = 0,
        help = "Maximum number of traces to process (0 = no limit)"
    )]
    max_traces: usize,
    #[clap(
        long,
        default_value_t = true,
        help = "Remove @plt stub functions from cluttering up the trace"
    )]
    remove_plt_stubs: bool,
    #[clap(
        long,
        short = 'o',
        default_value_t = true,
        help = "Whether to compress the output trace files (zstd)"
    )]
    compress_output: bool,
}

fn main() {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let args = Args::parse();

    let trace_options = FrameCompletionOptions {
        remove_plt_stubs: args.remove_plt_stubs,
    };

    let output_dir = Path::new(&args.output_dir);
    if output_dir.components().next().is_some() {
        fs::create_dir_all(output_dir).expect("Failed to create output directory");
    }

    let sym_regex = Regex::new(&args.symbol_regex).expect("Provided regex is invalid");

    let mut threads = thread::available_parallelism().unwrap().get();
    if threads > 1 {
        threads -= 1;
    }
    let threadpool = ThreadPool::new(threads);

    let trace_limit = if args.max_traces == 0 {
        None
    } else {
        Some(args.max_traces)
    };

    let (metadata, raw_data_vec) =
        perf_data::parse_perf_data(&args.perf_file).expect("Failed to parse perf.data");
    for mut raw_data in raw_data_vec {
        let mut decoder_image =
            Image::new(None).expect("Failed to allocate an image for the decoder");
        let mut trace_iter = intel_pt::convert(
            &mut raw_data.data,
            &mut decoder_image,
            raw_data.image,
            &metadata,
            &sym_regex,
            trace_options,
            trace_limit,
        )
        .expect("Failed to create Intel PT trace decoder")
        .enumerate();
        while let Some((i, trace)) = trace_iter
            .next()
            .expect("Unrecoverable error while decoding trace")
        {
            let output_dir2 = output_dir.to_path_buf();
            threadpool.execute(move || {
                let filename = format!("trace-{}-{}.bin", raw_data.tid, i);
                let path = output_dir2.join(filename);

                let output_file = File::create(&path).expect("Failed to create output file");
                let mut output_file = BufWriter::with_capacity(64 * 1024, output_file);
                trace
                    .bin_serialize(&mut output_file, args.compress_output)
                    .expect("Failed to binary encode trace");
                info!("Finished exporting: {}", path.display());
            });
        }
    }

    threadpool.join();
}
