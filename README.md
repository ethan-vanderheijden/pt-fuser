# PT-Fuser

This is a tool for processing, merging, and comparing Intel-PT traces.

Typical workflow is as follows:

1. *Collect* an Intel-PT trace of your application using `perf`
2. *Convert* the trace into `pt-fuser`'s internal format
3. *Filter* out outliers in latency/error count through high-level analysis
4. *Merge* multiple traces into one averaged trace
5. *Compare* traces against each other and find candidate regressions
6. *Export* the `pt-fuser` trace to a [Perfetto](https://ui.perfetto.dev/) trace

## Installation

After cloning this repo, you can build the project with Rust:

```bash
cargo build --release
```

This project consists of three crates:

- `transform-trace`: produces a shared object file that transforms the trace from `perf`'s format to `pt-fuser`'s internal format via `perf script`'s dlfilter feature.
- `pt-fuser`: the main crate that provides the core functionality of merging, comparing, and exporting traces.
- `perfetto-rust`: contains protobuf code for creating Perfetto trace files. It was generated from Perfetto's protobuf definitions.

## Usage

### 1. Collecting an Intel-PT trace

Use `perf` to collect the Intel-PT trace. For example:

```bash
perf record -e intel_pt/cyc=1,mtc_period=0,noretcomp=1/u -t <TID> -o <PERF_FILE>
```

The default configuration for Intel-PT is `intel_pt/cyc=0,mtc_period=3,noretcomp=0/u`, which has coarse grained timing information. MTC packets provide timing info every `2^mtc_period` times the Always Running Timer triggers, so smaller is finer grained. Enabling CYC accurate mode (`cyc=1`), updates the cycle count on every packet, which is very fine grained. Disabling return compression (`noretcomp=1`) produces a packet on every return instruction, which also increases granularity. However, increasing granularity comes at the cost of increasing the amount of data produced and may lead to trace decoding errors.

For more information, consult the man page (`man perf-intel-pt`), and [this blog post](https://halobates.de/blog/p/432) is a good primer.

### 2. Converting the trace to `pt-fuser`'s internal format

Convert the trace into `pt-fuser`'s internal format:

```bash
perf script -i <PERF_FILE> --itrace=bei0ns --dlfilter=./target/release/libtransform_trace.so --dlarg=<SYMBOL_REGEX> --dlarg=<OUTPUT_DIR> [--dlarg=<TRACE_LIMIT>]
```

- `SYMBOL_REGEX`: a regular expression matching the function in your application that you are interested in. It will be the top-level frame in the resulting trace.

- `OUTPUT_DIR`: the directory where processed traces are written to. By default, it will process every occurrence of SYMBOL_REGEX as a new trace, so if X matching functions were executed, X traces are produced.

- `TRACE_LIMIT`: an optional, numeric argument that limits the number of traces produced.

**Note**: All produced trace files are compressed with gzip.

### 3. Filtering out outliers in latency/error count

First, create a histogram of latencies or error counts across all your traces:

```bash
cargo run --bin histogram -- --gzip <lantency|error> </path/to/traces/*>
```

- `latency` will create a histogram of latencies (in nanoseconds) while `error` will create a histogram of Intel PT decoding errors (typically indicating lost data packets)
- `</path/to/traces/*>` is a list of trace files to be analyzed for the histogram
- `--gzip` will uncompress the input file before trying to read it

Then, you can develop a set of filters that excludes any perceived outliers. Each filter has the form:

```bash
[target=regex,][errors_min=num,][errors_max=num,][duration_min=num,][duration_max=num]
```

- `target`: a regular expression that determines which function to filter based on. If not provided, the filter is based on the top-level function in the trace.
- `errors_min` and `errors_max`: exclude all traces with a function whose error count is outside of the provided range.
- `duration_min` and `duration_max`: exclude all traces with a function whose latency is outside of the provided range.

You can preview the effect of a filter by re-running the histogram command with the `--filter` option:

```bash
cargo run --bin histogram -- --gzip <lantency|error> </path/to/traces/*> --filter <FILTER1> --filter <FILTER2> ...
```

### 4. Merge multiple traces into one averaged trace

Merge multiple `pt-fuser` traces into a single trace, which does its best to extract a common sequence of function calls and averages the latencies of them:

```bash
cargo run --bin merge -- --gzip --filter <FILTER1> --filter <FILTER2> <OUTPUT_FILE> </path/to/traces/*>
```

- `OUTPUT_FILE` is the file where the merged trace will be written to. (will be gzip compressed)
- `</path/to/traces/*>` is a list of trace files to merge
- `--filter` is optional and can be repeated multiple times to exclude some of the input traces
- `--gzip` will uncompress the input file before trying to read it

### 5. Compare traces against each other and find candidate regressions

***TODO***: this functionality has yet to be implemented

### 6. Exporting the trace to Perfetto

Convert a particular `pt-fuser` trace to a trace that can be viewed in https://ui.perfetto.dev/:

```bash
cargo run --bin convert_perfetto -- --gzip <INPUT_FILE> <OUTPUT_FILE>
```

- `--gzip` will uncompress the input file before trying to read it
