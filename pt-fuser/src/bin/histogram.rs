use clap::{Parser, ValueEnum};
use pt_fuser::{
    analysis::{
        FrameFinder,
        filter::{self, Filter},
        histogram::{Histogram, HistogramASCII},
    },
    trace::{Frame, Trace, trace_error},
};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use regex::Regex;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
enum Action {
    Errors,
    Latency,
    Interrupts,
}

#[derive(Parser)]
struct Cli {
    #[clap(value_enum, help = "The data to visualize")]
    action: Action,
    #[clap(
        long,
        default_value_t = false,
        help = "Whether the input trace files are gzipped"
    )]
    gzip: bool,
    #[cfg(feature = "gui")]
    #[clap(
        long,
        default_value_t = false,
        help = "Show the histogram in a GUI instead of ASCII"
    )]
    gui: bool,
    #[clap(
        long,
        help = "A regular expression for filter frames symbols. If not provided, will analyze root frames."
    )]
    name_regex: Option<String>,
    #[clap(long, help = Filter::HELP)]
    filter: Vec<Filter>,
    #[clap(help = "The pt-fuser trace files to analyze")]
    input_files: Vec<String>,
}

fn add_histogram_datapoint<'a>(
    frames: impl IntoIterator<Item = &'a Frame>,
    trace: &Trace,
    data: &mut Vec<f64>,
    action: &Action,
) {
    let error_event = match action {
        Action::Errors => trace.get_event(trace_error::DataCollectionError::ID),
        Action::Interrupts => trace.get_event(trace_error::TraceInterrupted::ID),
        Action::Latency => None,
    };

    match action {
        Action::Errors | Action::Interrupts => {
            let Some(errors) = error_event else {
                data.push(0f64);
                return;
            };
            let errors = errors.occurences();
            let mut error_index = 0;
            let mut num_errors = 0;
            for frame in frames {
                while error_index < errors.len() && errors[error_index] < frame.metrics.end() {
                    if errors[error_index] >= frame.metrics.start {
                        num_errors += 1;
                    }
                    error_index += 1;
                }
            }
            data.push(num_errors as f64);
        }
        Action::Latency => {
            for frame in frames {
                let datapoint = frame.metrics.total_time() as f64;
                let datapoint = datapoint / 1000f64; // convert nanoseconds to microseconds
                data.push(datapoint);
            }
        }
    }
}

fn main() {
    let cli = Cli::parse();

    let mut traces = cli
        .input_files
        .par_iter()
        .map(|input| {
            let trace_data = std::fs::read(input).expect("Failed to read pt-fuser trace file");
            Trace::bin_deserialize(&trace_data, cli.gzip).expect("pt-fuser trace file is malformed")
        })
        .collect::<Vec<Trace>>();

    for filter in &cli.filter {
        traces = filter::filter_traces(traces, filter);
    }

    let regex = if let Some(name) = cli.name_regex {
        Some(Regex::new(&name).expect("Invalid regular expression"))
    } else {
        None
    };

    let mut data = Vec::new();
    for trace in &traces {
        if let Some(regex) = &regex {
            let pred = |f: &Frame| regex.is_match(&f.symbol.name);
            add_histogram_datapoint(
                FrameFinder::new(trace.root_frame(), &pred),
                trace,
                &mut data,
                &cli.action,
            );
        } else {
            add_histogram_datapoint(
                std::iter::once(trace.root_frame()),
                trace,
                &mut data,
                &cli.action,
            );
        }
    }

    let (title, x_axis) = match cli.action {
        Action::Errors => (
            format!("Error Count Distribution of {} traces", traces.len()),
            "Error Count".to_string(),
        ),
        Action::Latency => (
            format!("Latency (µs) Distribution of {} traces", traces.len()),
            "Latency (µs)".to_string(),
        ),
        Action::Interrupts => (
            format!("Interrupt Count Distribution of {} traces", traces.len()),
            "Interrupt Count".to_string(),
        ),
    };

    #[cfg(feature = "gui")]
    if cli.gui {
        use pt_fuser::analysis::histogram::HistogramGUI;

        HistogramGUI::new(title, &data, x_axis)
            .show()
            .expect("Failed to show histogram GUI");
        return;
    }

    HistogramASCII::new(title, &data, x_axis)
        .show()
        .expect("Failed to show histogram ASCII");
}
