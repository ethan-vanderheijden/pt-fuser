mod elf_image;
mod intel_pt;
mod perf_data;
mod symbol_cache;

use std::{
    fs::{self, File},
    io::BufWriter,
    path::Path,
    thread,
};

use clap::Parser;
use fallible_iterator::FallibleIterator;
use libipt::image::Image;
use pt_fuser::trace::{Trace, builder::FrameCompletionOptions};
use regex::Regex;
use threadpool::ThreadPool;
use tracing::{Level, error, info};

use crate::intel_pt::{ConversionOpts, ProgressTracker};

#[derive(Parser)]
#[command(
    about = "Converts a trace from the perf.data format to pt-fuser's internal representation"
)]
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
        short = 'v',
        default_value_t = false,
        help = "Enable verbose logging (disables progress bar)"
    )]
    verbose: bool,
}

fn main() {
    let args = Args::parse();

    let subscriber = tracing_subscriber::fmt()
        .with_max_level(if args.verbose {
            Level::DEBUG
        } else {
            Level::ERROR
        })
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();

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

    // look for vmlinux up front so any error is reported early
    match elf_image::find_vmlinux() {
        Ok(_) => {}
        Err(reason) => error!(reason),
    }

    let (metadata, raw_data_vec) =
        perf_data::parse_perf_data(&args.perf_file).expect("Failed to parse perf.data");
    for mut raw_data in raw_data_vec {
        let progress_tracker = if args.verbose {
            None
        } else {
            Some(ProgressTracker::new(|num_traces| {
                format!("Thread {} - produced {} traces", raw_data.tid, num_traces)
            }))
        };

        let mut decoder_image =
            Image::new(None).expect("Failed to allocate an image for the decoder");

        let options = ConversionOpts {
            target_symbol: sym_regex.clone(),
            frame_completion_options: trace_options,
            max_traces: trace_limit,
            progress_info: progress_tracker,
        };
        let mut trace_iter = intel_pt::convert(
            &mut raw_data.data,
            &mut decoder_image,
            raw_data.image,
            &metadata,
            options,
        )
        .expect("Failed to create Intel PT trace decoder")
        .enumerate();

        while let Some((i, trace)) = trace_iter
            .next()
            .expect("Unrecoverable error while decoding trace")
        {
            let output_dir2 = output_dir.to_path_buf();
            threadpool.execute(move || {
                let filename = format!(
                    "trace-{}-{}.{}",
                    raw_data.tid,
                    i,
                    Trace::RECOMMENDED_EXTENSION
                );
                let path = output_dir2.join(filename);

                let output_file = File::create(&path).expect("Failed to create output file");
                let mut output_file = BufWriter::with_capacity(64 * 1024, output_file);
                trace
                    .bin_serialize(&mut output_file, true)
                    .expect("Failed to binary encode trace");
                info!("Finished exporting: {}", path.display());
            });
        }
    }

    threadpool.join();
    // a panicking export job only kills its worker, so surface it in the exit code
    if threadpool.panic_count() > 0 {
        eprintln!("Failed to export {} trace(s)", threadpool.panic_count());
        std::process::exit(1);
    }
}
