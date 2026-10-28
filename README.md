# PT-Fuser

This is a tool for processing, merging, and comparing Intel-PT traces.

Typical workflow is as follows:

1. *Collect* an Intel-PT trace of your application using `perf`
2. *Convert* the trace into `pt-fuser`'s internal format
3. *Filter* out outliers in latency/error count through high-level analysis
4. *Merge* multiple traces into one averaged trace
5. *Compare* traces against each other and find candidate regressions
6. *Export* the `pt-fuser` trace to a [Perfetto](https://ui.perfetto.dev/) trace

> [!Note]
> The best way to understand why this tool exists and how to use it is to follow our interactive tutorial on investigating a real performance regression in MongoDB.
> 
> https://github.com/ethan-vanderheijden/perf-creep-tutorial

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
cargo run --bin process_trace -- <PERF_FILE> <SYMBOL_REGEX> <OUTPUT_DIR>
```

The required arguments are:

- `PERF_FILE`: the path to the `perf.data` file produced by `perf record`
- `SYMBOL_REGEX`: a regular expression matching the function in your application that you are interested in. It will be the top-level frame in the resulting trace.
- `OUTPUT_DIR`: the directory where processed traces are written to. Every occurrence of SYMBOL_REGEX becomes a new trace, so if X matching functions were executed, X traces are produced. Traces are named `OUTPUT_DIR/trace-<TID>-<INDEX>.bin.zst`.

The options are:

- `--max-traces=N`: stops producing traces after N traces have been produced.
- `--remove-plt-stubs` (default true): binaries that call into shared objects first call into a PLT stub, which then jumps to the actual function. This option removes the PLT stub from the trace, decluttering it.

### 3. Filtering out outliers in latency/error count

First, create a histogram of latencies or error counts across all your traces:

```bash
cargo run --bin histogram -- <lantency|errors|interrupts> </path/to/traces/*>
```

- `latency` will create a histogram of latencies (in nanoseconds). `errors` will create a histogram of Intel PT decoding errors, typically lost data due to buffer overflow. `interrupts` will create a histogram of the number of times the trace was paused while the CPU serviced an interrupt.
- `</path/to/traces/*>` is a list of trace files to be analyzed for the histogram

Then, you can develop a set of filters that excludes any perceived outliers. Each filter has the form:

```bash
[target=regex,][min_errors=num,][max_errors=num,][min_latency=num,][max_latency=num]
```

- `target`: a regular expression that determines which function to filter based on. If not provided, the filter is based on the top-level function in the trace.
- `min_errors` and `max_errors`: exclude all traces with a function whose error count is outside of the provided range.
- `min_latency` and `max_latency`: exclude all traces with a function whose latency is outside of the provided range.

You can preview the effect of a filter by re-running the histogram command with the `--filter` option:

```bash
cargo run --bin histogram -- <lantency|errors> </path/to/traces/*> --filter <FILTER1> --filter <FILTER2> ...
```

### 4. Merge multiple traces into one averaged trace

Merge multiple `pt-fuser` traces into a single trace, which does its best to extract a common sequence of function calls and averages the latencies of them:

```bash
cargo run --bin merge -- --filter <FILTER1> --filter <FILTER2> <OUTPUT_FILE> </path/to/traces/*>
```

- `OUTPUT_FILE` is the file where the merged trace will be written to.
- `</path/to/traces/*>` is a list of trace files to merge
- `--filter` is optional and can be repeated multiple times to exclude some of the input traces

Each frame in the merged trace is produced from a set frames in the original traces. We can compute statistics over the set of original frames. The statistics can be seen in Perfetto under "debug" in the Arguments pane.
- `--record-basic-stats` (default true): records quartiles, mean, and standard deviation
- `--record-raw` (default false): records the latencies of every original frame, in case you want to do your own analysis later.
- `--record-noise-contribution=<inclusive|exclusive>` (disabled by default): TODO - document Noise Contribution

### 5. Compare traces against each other and find candidate regressions

***TODO***: this functionality has yet to be implemented

### 6. Exporting the trace to Perfetto

Convert a particular `pt-fuser` trace to a trace that can be viewed in https://ui.perfetto.dev/:

```bash
cargo run --bin convert_perfetto -- <INPUT_FILE> <OUTPUT_FILE>
```

The options are:
- `--render-pauses=<gap|block>` (default gap): places where the tracing was paused are usually rendered as gaps in Perfetto. If there's a lot of gaps, this can become annoying, so you can reder pauses as a special frame called '--pause--'.

## Versioning

All three binaries (`merge`, `histogram`, and `convert_perfetto`) support `-V`
and `--version`. They print the project name, the tool name, the Cargo package
version, and the short Git commit of the build, for example:

```text
pt-fuser merge 0.3.0 (03cb739)
```

The version comes from `Cargo.toml` and the commit is embedded at build time.
The commit is optional: a build from a source tree without Git metadata just
prints `pt-fuser merge 0.3.0`. Setting `PT_FUSER_COMMIT` overrides the lookup,
which is how the release workflow gives every target the same commit.

In our versioning scheme "vX.Y.Z", "X.Y" refers to the version of the trace format. Every time the trace format changes in a backwards-incompatible way, we change "X.Y". If you try to use our tools with a trace file of an incompatible version, the file will be rejected with a version mismatch error.
