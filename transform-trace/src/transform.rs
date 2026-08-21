use std::{
    collections::HashMap, fs, num::NonZero, os::raw::c_void, path::Path, sync::OnceLock, thread,
};

use pt_fuser::trace::{
    SymbolInfo, Trace,
    builder::{BuilderResult, FrameCompletionOptions, PausedTraceBuilder, TraceBuilder},
    metrics::Metrics,
    trace_error,
};
use regex::Regex;
use threadpool::ThreadPool;
use tracing::{error, info, warn};

use crate::perf;

static THREADPOOL: OnceLock<ThreadPool> = OnceLock::new();

/// Creates a symbol for a frame whose symbol information isn't known.
/// Unknown frames can be recognized by checking .offset == 0
fn fallback_symbol() -> SymbolInfo {
    SymbolInfo {
        name: "[unknown]".to_string(),
        offset: 0,
        size: 0,
    }
}

/// Returns how many frames up the callstack the address is contained in, or None if it's not contained in any frame.
fn contained_in_callstack(builder: &TraceBuilder, addr: u64) -> Option<usize> {
    for i in 1..builder.callstack_depth() {
        let symbol = builder.get_frame_symbol(i);
        if symbol.contains(addr) {
            return Some(i);
        }
    }
    None
}

enum BuilderState {
    InProgress(TraceBuilder),
    Paused(PausedTraceBuilder),
}

pub(crate) struct State {
    sym_regex: Regex,
    output_dir: String,
    traces_limit: Option<u32>,
    trace_nums: HashMap<i32, u32>,
    builders: HashMap<i32, BuilderState>,
    cur_metrics: Metrics,
}

impl State {
    pub(crate) fn new(sym_regex: Regex, output_dir: String, traces_limit: Option<u32>) -> Self {
        Self {
            sym_regex,
            output_dir,
            traces_limit,
            trace_nums: HashMap::new(),
            builders: HashMap::new(),
            cur_metrics: Metrics::constant(0),
        }
    }
}

fn export_trace(output_dir: &str, trace_num: u32, tid: i32, trace: Trace) {
    let filename = format!("trace-{}-{}.bin", tid, trace_num);
    let path = Path::new(output_dir).join(filename);
    let path2 = path.clone();

    THREADPOOL
        .get_or_init(|| {
            ThreadPool::new(
                <NonZero<usize> as Into<usize>>::into(thread::available_parallelism().unwrap())
                    .saturating_sub(1)
                    .max(1),
            )
        })
        .execute(move || {
            let binary_encoded = trace
                .bin_serialize(true)
                .expect("Failed to binary encode trace");
            if let Some(parent) = path.parent()
                && parent.components().next().is_some()
            {
                fs::create_dir_all(parent).expect("Failed to create output directory");
            }
            fs::write(path, binary_encoded).expect("Failed to write trace file");

            info!("Finished exporting: {}", path2.display());
        });
}

/// Each branch instruction is processed first as 'i' then as 'b',
/// so the instruction is added to the parent's insn_cnt and cyc_cnt will be up to date.
/// Note: updating cycle count every 'i' event is most reliable since 'i' events that don't represent
/// taken branches may still produce a CYC packet. Conversely, cycle count for 'b' events is only
/// updated when a CYC packet happens to be produced for that 'b' event, which is a matter of luck.
pub(crate) fn process_insn_event(
    state: &mut State,
    sample: &perf::perf_dlfilter_sample,
    _ctx: *mut c_void,
) {
    state.cur_metrics.insn_count += 1;
    state.cur_metrics.cycles += sample.cyc_cnt;
    state.cur_metrics.ts = sample.time;
    if let Some(BuilderState::InProgress(builder)) = state.builders.get(&sample.tid) {
        let current_symbol = builder.get_frame_symbol(0);
        if current_symbol.offset != 0 && !current_symbol.contains(sample.ip) {
            warn!(
                "Instruction event at time (ns={}) has IP (0x{:x}) that isn't contained in the current frame's symbol ({}). \
                 This indicates a bug in the transformer logic",
                sample.time, sample.ip, current_symbol
            );
        }
    }
}

fn process_return_event(
    mut builder: TraceBuilder,
    state: &mut State,
    sample: &perf::perf_dlfilter_sample,
    levels: usize,
) -> Option<TraceBuilder> {
    for i in 1..=levels {
        builder = match builder
            .complete_frame(
                state.cur_metrics,
                Some(FrameCompletionOptions {
                    remove_plt_stubs: true,
                }),
            )
            .expect("Failed to complete stack frame")
        {
            BuilderResult::Completed(trace) => {
                let trace_num = state.trace_nums.entry(sample.tid).or_insert(0);
                info!(
                    "Extracted trace #{} for tid={}. Trace ran from {} to {} and had {} errors.",
                    trace_num,
                    sample.tid,
                    trace.root_frame().metrics.start.ts,
                    trace.root_frame().metrics.end().ts,
                    trace
                        .get_event(trace_error::DataCollectionError::ID)
                        .unwrap()
                        .occurences()
                        .len()
                );
                export_trace(&state.output_dir, *trace_num, sample.tid, trace);
                *trace_num += 1;

                if i != levels {
                    warn!(
                        "At time {}, tried returning {} levels but trace ended after {} levels.",
                        sample.time, levels, i
                    );
                }
                return None;
            }
            BuilderResult::Builder(builder_result) => builder_result,
        }
    }
    Some(builder)
}

fn process_call_event(
    mut builder: TraceBuilder,
    state: &mut State,
    sample: &perf::perf_dlfilter_sample,
    call_target: SymbolInfo,
) -> Option<TraceBuilder> {
    let root_frame = builder.get_frame_symbol(builder.callstack_depth() - 1);
    if &call_target == root_frame {
        error!(
            "A function was called at timestamp={}, but that function is the same as the root function of the \
            current trace. Unless this is supposed to be a recursive function, it likely means the root frame \
            already returned, but we missed it due to a trace error. In this case, the trace is corrupted beyond \
            repair, so we will end it now.",
            sample.time
        );
        let depth = builder.callstack_depth();
        process_return_event(builder, state, sample, depth)
    } else {
        builder.push_frame(state.cur_metrics, call_target);
        Some(builder)
    }
}

pub(crate) fn process_branch_event(
    state: &mut State,
    sample: &perf::perf_dlfilter_sample,
    ctx: *mut c_void,
) -> bool {
    let target_symbol = unsafe { crate::resolve_addr(sample, ctx) };
    let target_symbol = if let Some(sym) = target_symbol {
        unsafe { crate::normalize_symbol_addr(sample, sym, ctx) }
    } else {
        fallback_symbol()
    };

    if sample.flags & perf::PERF_DLFILTER_FLAG_TRACE_BEGIN != 0 {
        // trace begins anew on an error or when the process was unscheduled
        // in either case, an instruction branch does not precede so we must update timestamps
        state.cur_metrics.insn_count += 1;
        state.cur_metrics.cycles += sample.cyc_cnt;
        state.cur_metrics.ts = sample.time;
    }

    let builder_state = state.builders.remove(&sample.tid);

    // How we handle [unknown] frames
    // ------------------------------
    // When we hit an unknown symbol, we push it onto the callstack.
    // We wait until we see a branch target that lands in a known symbol, at which point
    // we pop off the unknown frame. If the known symbol is higher up the callstack,
    // we pop off frames until we get there. Otherwise, we treat it as a new call.
    // INVARIANT: at most, there is a single unknown frame at the top of the callstack.

    // Here's how we detect calls and returns
    // --------------------------------------
    // If current frame is unknown:
    //    if target address is X levels up the callstack -> return X levels
    //       (handles case where function calls into an unknown symbol that eventually returns)
    //    if target address is in a known symbol -> return 2 levels + call
    //       (either a function executing unknown code [probably library function] calls a known symbol;
    //        in this rare case, we only want to return one level)
    //       (or trace decoding errored inside the unknown symbol, and by now, the unknown symbol and it's
    //        parent frame are done; in this rare case, we want to return two levels)
    // Otherwise if target address isn't inside the current frame:
    //    if there is an explicit CALL instruction -> call
    //    if target address is X levels up the callstack -> return X levels
    //       (can handle exotic control flows that return without a RET instruction, e.g. thrown exceptions)
    //    if there is an explicit RET instruction AND only one incomplete frame -> return 1 level
    //       (handles the corner case where the callstack is empty and it's time to finsh the trace)
    //    else -> call
    //       (handles indirect function calls)

    if let Some(builder_state) = builder_state {
        let mut builder = match builder_state {
            BuilderState::InProgress(mut builder) => {
                if sample.flags & perf::PERF_DLFILTER_FLAG_TRACE_BEGIN != 0 {
                    // TRACE_BEGIN without a previous TRACE_END indicates an error in the trace
                    builder.event_occured(trace_error::DataCollectionError::ID, state.cur_metrics);
                }
                builder
            }
            BuilderState::Paused(paused) => {
                if sample.flags & perf::PERF_DLFILTER_FLAG_TRACE_BEGIN == 0 {
                    error!(
                        "Expected TRACE_BEGIN before receiving other events at time {}.",
                        sample.time
                    );
                }
                paused.resume(state.cur_metrics)
            }
        };

        let current_symbol = builder.get_frame_symbol(0);

        let resulting_builder = if current_symbol.offset == 0 {
            if target_symbol.offset != 0 {
                if let Some(levels) = contained_in_callstack(&mut builder, sample.addr) {
                    process_return_event(builder, state, sample, levels)
                } else {
                    let temp_builder = process_return_event(builder, state, sample, 2);
                    if let Some(builder) = temp_builder {
                        process_call_event(builder, state, sample, target_symbol)
                    } else {
                        None
                    }
                }
            } else {
                Some(builder)
            }
        } else if !current_symbol.contains(sample.addr) {
            let returning_levels = contained_in_callstack(&mut builder, sample.addr).or(
                if (sample.flags & perf::PERF_DLFILTER_FLAG_RETURN) != 0
                    && builder.callstack_depth() == 1
                {
                    Some(1)
                } else {
                    None
                },
            );

            if (sample.flags & perf::PERF_DLFILTER_FLAG_CALL) != 0 || returning_levels.is_none() {
                process_call_event(builder, state, sample, target_symbol)
            } else if let Some(returning_levels) = returning_levels {
                process_return_event(builder, state, sample, returning_levels)
            } else {
                Some(builder)
            }
        } else {
            Some(builder)
        };

        // if resulting_builder is None, then trace is over and our work is done
        if let Some(mut builder) = resulting_builder {
            if sample.flags & perf::PERF_DLFILTER_FLAG_TRACE_END != 0 {
                if sample.flags & perf::PERF_DLFILTER_FLAG_ASYNC != 0 {
                    builder.event_occured(trace_error::TraceInterrupted::ID, state.cur_metrics);
                }

                let paused = builder
                    .pause(state.cur_metrics)
                    .expect("Failed to pause TraceBuilder");
                state
                    .builders
                    .insert(sample.tid, BuilderState::Paused(paused));
            } else {
                state
                    .builders
                    .insert(sample.tid, BuilderState::InProgress(builder));
            }
        }
    } else if target_symbol.offset != 0 && state.sym_regex.is_match(&target_symbol.name) {
        if state.traces_limit.is_some() && state.traces_limit.unwrap() == 0 {
            return false;
        }

        info!(
            "Starting trace: tid={}, symbol={}",
            sample.tid, target_symbol.name
        );
        state.traces_limit = state.traces_limit.map(|limit| limit - 1);
        let mut new_builder = TraceBuilder::new(state.cur_metrics, target_symbol);
        new_builder.new_event(
            trace_error::DataCollectionError::ID,
            trace_error::DataCollectionError::NAME.to_string(),
            trace_error::DataCollectionError::DESC.to_string(),
        );
        new_builder.new_event(
            trace_error::TraceInterrupted::ID,
            trace_error::TraceInterrupted::NAME.to_string(),
            trace_error::TraceInterrupted::DESC.to_string(),
        );
        state
            .builders
            .insert(sample.tid, BuilderState::InProgress(new_builder));
    }

    true
}

pub(crate) fn finish_exporting() {
    if let Some(threadpool) = THREADPOOL.get() {
        threadpool.join();
    }
}
