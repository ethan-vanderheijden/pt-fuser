use std::{
    collections::HashMap, fs, num::NonZero, os::raw::c_void, path::Path, sync::OnceLock, thread,
};

use pt_fuser::trace::{
    SymbolInfo, Trace, TraceError,
    builder::{BuilderResult, TraceBuilder},
    metrics::Metrics,
};
use regex::Regex;
use threadpool::ThreadPool;
use tracing::{info, warn};

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

pub(crate) struct State {
    sym_regex: Regex,
    output_dir: String,
    traces_limit: Option<u32>,
    trace_nums: HashMap<i32, u32>,
    builders: HashMap<i32, TraceBuilder>,
    insn_cnt: u64,
    cyc_cnt: u64,
}

impl State {
    pub(crate) fn new(sym_regex: Regex, output_dir: String, traces_limit: Option<u32>) -> Self {
        Self {
            sym_regex,
            output_dir,
            traces_limit,
            trace_nums: HashMap::new(),
            builders: HashMap::new(),
            insn_cnt: 0,
            cyc_cnt: 0,
        }
    }
}

fn export_trace(state: &mut State, tid: i32, trace: Trace) {
    let trace_num = state.trace_nums.entry(tid).or_insert(0);
    let filename = format!("trace-{}-{}.bin", tid, trace_num);
    *trace_num += 1;

    let path = Path::new(&state.output_dir).join(filename);
    let path2 = path.clone();

    THREADPOOL
        .get_or_init(|| {
            ThreadPool::new(
                <NonZero<usize> as Into<usize>>::into(thread::available_parallelism().unwrap()) - 1,
            )
        })
        .execute(move || {
            info!("Exporting {}...", path.display());

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
    state.insn_cnt += 1;
    state.cyc_cnt += sample.cyc_cnt;
    if let Some(builder) = state.builders.get(&sample.tid) {
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

fn process_return_event(state: &mut State, sample: &perf::perf_dlfilter_sample, levels: usize) {
    let cur_metrics = Metrics {
        ts: sample.time,
        cycles: state.cyc_cnt,
        insn_count: state.insn_cnt,
    };

    let mut builder = Some(state.builders.remove(&sample.tid).unwrap());
    for _ in 1..=levels {
        match builder
            .take()
            .expect("Builder should exist since we should have incomplete frames left")
            .complete_frame(cur_metrics)
            .expect("Failed to complete stack frame")
        {
            BuilderResult::Completed(trace) => {
                info!(
                    "Completed trace for tid={}. Trace ran from {} to {} and had {} errors.",
                    sample.tid,
                    trace.root_frame().metrics.start.ts,
                    trace.root_frame().metrics.end.ts,
                    trace
                        .get_event(TraceError::DataCollectionError as u32)
                        .unwrap()
                        .occurences()
                        .len()
                );
                export_trace(state, sample.tid, trace);
            }
            BuilderResult::Builder(builder_result) => {
                builder = Some(builder_result);
            }
        }
    }
    if let Some(builder) = builder {
        state.builders.insert(sample.tid, builder);
    }
}

pub(crate) fn process_branch_event(
    state: &mut State,
    sample: &perf::perf_dlfilter_sample,
    ctx: *mut c_void,
) {
    let target_symbol = unsafe { crate::resolve_addr(sample, ctx) };
    let target_symbol = if let Some(sym) = target_symbol {
        unsafe { crate::normalize_symbol_addr(sample, sym, ctx) }
    } else {
        fallback_symbol()
    };

    let cur_metrics = Metrics {
        ts: sample.time,
        cycles: state.cyc_cnt,
        insn_count: state.insn_cnt,
    };

    let builder = state.builders.get_mut(&sample.tid);

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

    if let Some(builder) = builder {
        if sample.ip == 0 {
            builder.event_occured(TraceError::DataCollectionError as u32, cur_metrics);
        }

        let current_symbol = builder.get_frame_symbol(0);

        if current_symbol.offset == 0 {
            if target_symbol.offset != 0 {
                if let Some(levels) = contained_in_callstack(builder, sample.addr) {
                    process_return_event(state, sample, levels);
                } else {
                    process_return_event(state, sample, 2);
                    let builder = state
                        .builders
                        .get_mut(&sample.tid)
                        .expect("Builder should exist since [unknown] can't be top-level frame.");
                    builder.push_frame(cur_metrics, target_symbol);
                }
            }
        } else if !current_symbol.contains(sample.addr) {
            let returning_levels = contained_in_callstack(builder, sample.addr).or(
                if (sample.flags & perf::PERF_DLFILTER_FLAG_RETURN) != 0
                    && builder.callstack_depth() == 1
                {
                    Some(1)
                } else {
                    None
                },
            );

            if (sample.flags & perf::PERF_DLFILTER_FLAG_CALL) != 0 || returning_levels.is_none() {
                builder.push_frame(cur_metrics, target_symbol);
            } else if let Some(returning_levels) = returning_levels {
                process_return_event(state, sample, returning_levels);
            }
        }
    } else if target_symbol.offset != 0
        && state.sym_regex.is_match(&target_symbol.name)
        && (state.traces_limit.is_none() || state.traces_limit.unwrap() > 0)
    {
        info!(
            "Starting trace: tid={}, symbol={}",
            sample.tid, target_symbol.name
        );
        state.traces_limit = state.traces_limit.map(|limit| limit - 1);
        let mut new_builder = TraceBuilder::new(cur_metrics, target_symbol);
        new_builder.new_event(
            TraceError::DataCollectionError as u32,
            "Errors".to_string(),
            "Trace decoder hit an error. Callstacks may be corrupted.".to_string(),
        );
        state.builders.insert(sample.tid, new_builder);
    }
}

pub(crate) fn finish_exporting() {
    if let Some(threadpool) = THREADPOOL.get() {
        threadpool.join();
    }
}
