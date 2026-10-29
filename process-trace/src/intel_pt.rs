use std::{cell::RefCell, rc::Rc};

use fallible_iterator::FallibleIterator;
use indicatif::{ProgressBar, ProgressStyle};
pub use libipt::enc_dec_builder::{Cpu, CpuVendor, Frequency};
use libipt::{
    block::{Block, BlockDecoder},
    enc_dec_builder::EncoderDecoderBuilder,
    error::{PtError, PtErrorCode},
    event::{Event as libiptEvent, EventType as libiptEventType},
    image::Image,
    insn::Class,
    status::Status,
};

use pt_fuser::trace::{
    SymbolInfo, Trace,
    builder::{BuilderResult, FrameCompletionOptions, PausedTraceBuilder, TraceBuilder},
    metrics::Metrics,
    trace_error::{DataCollectionError, TraceInterrupted},
};
use regex::Regex;
use tracing::{error, info, warn};

pub const PROGRESS_STYLE_TEMPLATE: &str =
    "[{wide_bar}] {percent}%\n   ↳ Estimated time remaining: {eta}\n     {msg}";

pub trait ImageProvider {
    type ReadError: std::error::Error;
    type SymbolError: std::error::Error;

    /// Reads a chunk of data from the process's memory. Returns the number of bytes read.
    fn read_chunk(&mut self, buffer: &mut [u8], vaddr: u64) -> Result<usize, Self::ReadError>;
    /// Resolves the symbol name based on the virtual address. The returned symbol's offset must be in the virtual address space.
    fn resolve_symbol(&mut self, vaddr: u64) -> Result<SymbolInfo, Self::SymbolError>;
}

/// Creates a symbol for a frame whose symbol information isn't known.
/// Unknown frames can be recognized by checking .offset == 0
fn fallback_symbol() -> SymbolInfo {
    SymbolInfo {
        name: "[unknown]".to_string(),
        vaddr_start: 0,
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

enum DecoderState {
    Finished,
    ReadEvent { continue_resync: bool },
    ReadBlock,
    Resync,
    SyncForward,
}

struct Transformer<'a, 'b: 'a, 'c, I: ImageProvider, F: Fn(usize) -> String> {
    decoder: BlockDecoder<'a, 'c>,
    // decoder holds a raw pointer to the buffer, which must outlive it
    _buffer: &'b [u8],
    image: Rc<RefCell<I>>,
    tsc_conversion: TscConversion,
    target_symbol: Regex,
    frame_completion_options: FrameCompletionOptions,
    traces_left: Option<usize>,
    cur_metrics: Metrics,
    builder: Option<BuilderState>,
    // tracks the reason why control flows from one block to the next
    branch_reason: Class,
    decoder_state: DecoderState,
    progress_tracker: Option<ProgressTracker<F>>,
}

impl<'a, 'b: 'a, 'c, I: ImageProvider, F: Fn(usize) -> String> Transformer<'a, 'b, 'c, I, F> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        decoder: BlockDecoder<'a, 'c>,
        buffer: &'b [u8],
        image: Rc<RefCell<I>>,
        tsc_conversion: TscConversion,
        target_symbol: Regex,
        frame_completion_options: FrameCompletionOptions,
        traces_left: Option<usize>,
        progress_tracker: Option<ProgressTracker<F>>,
    ) -> Result<Self, PtError> {
        Ok(Self {
            decoder,
            _buffer: buffer,
            image,
            tsc_conversion,
            target_symbol,
            frame_completion_options,
            traces_left,
            cur_metrics: Metrics::constant(0),
            builder: None,
            branch_reason: Class::Unknown,
            decoder_state: DecoderState::SyncForward,
            progress_tracker,
        })
    }

    fn update_cur_metrics(&mut self, tsc: u64, insn_delta: u64) {
        self.cur_metrics.insn_count += insn_delta;
        if tsc >= self.cur_metrics.cycles {
            self.cur_metrics.cycles = tsc;
            self.cur_metrics.ts = self.tsc_conversion.convert(tsc);
        } else {
            warn!(
                "TSC went backwards! From {} to {}",
                self.cur_metrics.cycles, tsc
            );
        }
    }

    fn process_call_event(
        &self,
        mut builder: TraceBuilder,
        call_target: SymbolInfo,
    ) -> BuilderResult {
        let root_frame = builder.get_frame_symbol(builder.callstack_depth() - 1);
        if &call_target == root_frame {
            error!(
                "A function was called at timestamp={}, but that function is the same as the root function of the \
                current trace. Unless this is supposed to be a recursive function, it likely means the root frame \
                already returned, but we missed it due to a trace error. In this case, the trace is corrupted beyond \
                repair, so we will end it now.",
                self.cur_metrics.ts
            );
            let depth = builder.callstack_depth();
            self.process_return_event(builder, depth)
        } else {
            builder.push_frame(self.cur_metrics, call_target);
            BuilderResult::Builder(builder)
        }
    }

    fn process_return_event(&self, mut builder: TraceBuilder, levels: usize) -> BuilderResult {
        for i in 1..=levels {
            builder = match builder
                .complete_frame(self.cur_metrics, Some(self.frame_completion_options))
                .expect("Failed to complete stack frame")
            {
                BuilderResult::Completed(trace) => {
                    if i != levels {
                        warn!(
                            "At time {}, tried returning {} levels but trace ended after {} levels.",
                            self.cur_metrics.ts, levels, i
                        );
                    }
                    return BuilderResult::Completed(trace);
                }
                BuilderResult::Builder(builder_result) => builder_result,
            }
        }
        BuilderResult::Builder(builder)
    }

    fn disable_tracing(&mut self, cur_metrics: Metrics) {
        info!("Disabling tracing at time {}", cur_metrics);
        if let Some(builder_state) = self.builder.take() {
            match builder_state {
                BuilderState::InProgress(builder) => {
                    self.builder = Some(BuilderState::Paused(builder.pause(cur_metrics)));
                }
                BuilderState::Paused(builder) => {
                    error!(
                        "Received Disabled event while trace is already paused (as time {})!",
                        cur_metrics
                    );
                    self.builder = Some(BuilderState::Paused(builder));
                }
            }
        }
    }

    fn enable_tracing(&mut self, cur_metrics: Metrics) {
        info!("Enabling tracing at time {}", cur_metrics);
        if let Some(builder_state) = self.builder.take() {
            match builder_state {
                BuilderState::InProgress(builder) => {
                    error!(
                        "Received Enabled event while trace is already in progress (as time {})!",
                        cur_metrics
                    );
                    self.builder = Some(BuilderState::InProgress(builder));
                }
                BuilderState::Paused(builder) => {
                    self.builder = Some(BuilderState::InProgress(builder.resume(cur_metrics)));
                }
            }
        }
    }

    fn process_event(&mut self, event: libiptEvent) {
        let cur_metrics = if let Some(tsc) = event.tsc() {
            Metrics {
                cycles: tsc,
                ts: self.tsc_conversion.convert(tsc),
                insn_count: self.cur_metrics.insn_count,
            }
        } else {
            self.cur_metrics
        };

        match event.event_type() {
            libiptEventType::AsnycDisabled(_)
            | libiptEventType::AsyncBranch(_)
            | libiptEventType::AsyncPaging(_) => {
                if let Some(BuilderState::InProgress(builder)) = &mut self.builder {
                    builder.event_occured(TraceInterrupted::ID, cur_metrics);
                }
            }
            libiptEventType::Overflow(_) => {
                if let Some(BuilderState::InProgress(builder)) = &mut self.builder {
                    builder.event_occured(DataCollectionError::ID, cur_metrics);
                }
            }
            _ => {}
        }

        match event.event_type() {
            libiptEventType::AsnycDisabled(_) | libiptEventType::Disabled(_) => {
                self.disable_tracing(cur_metrics)
            }
            libiptEventType::Enabled(_) => self.enable_tracing(cur_metrics),
            _ => {}
        }

        match event.event_type() {
            libiptEventType::AsyncBranch(_)
            | libiptEventType::AsyncPaging(_)
            | libiptEventType::Paging(_) => {
                // these events indicate out-of-the-ordinary control flow (e.g., trap to kernel)
                self.branch_reason = Class::Unknown;
            }
            libiptEventType::Enabled(_) => {
                // after tracing is re-enabled, we could be anywhere in the program and
                // don't know how we got there
                self.branch_reason = Class::Unknown;
            }
            libiptEventType::Overflow(_) => {
                // after the overflow is cleared, we could be arbitrarily further into the program
                self.branch_reason = Class::Unknown;
            }
            _ => {}
        }
    }

    fn process_block(&mut self, block: Block) -> Option<Trace> {
        let block_sym = match self.image.borrow_mut().resolve_symbol(block.ip()) {
            Ok(s) => s,
            Err(_) => fallback_symbol(),
        };

        let branch_reason = self.branch_reason;
        self.branch_reason = block.class();

        info!("{:?} into {}", branch_reason, block_sym);
        info!("at time {}", self.cur_metrics);

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

        if let Some(builder) = self.builder.take() {
            let builder = match builder {
                BuilderState::InProgress(trace_builder) => trace_builder,
                BuilderState::Paused(_) => {
                    error!(
                        "Decoding a block but the trace is paused (as time {})!",
                        self.cur_metrics
                    );
                    self.builder = Some(builder);
                    return None;
                }
            };

            let current_sym = builder.get_frame_symbol(0);

            let builder_result = if current_sym.vaddr_start == 0 {
                if block_sym.vaddr_start != 0 {
                    if let Some(levels) = contained_in_callstack(&builder, block.ip()) {
                        self.process_return_event(builder, levels)
                    } else {
                        match self.process_return_event(builder, 2) {
                            BuilderResult::Builder(new_builder) => {
                                self.process_call_event(new_builder, block_sym)
                            }
                            r => r,
                        }
                    }
                } else {
                    BuilderResult::Builder(builder)
                }
            } else if !current_sym.contains(block.ip()) {
                let returning_levels = contained_in_callstack(&builder, block.ip()).or(
                    if (branch_reason == Class::Return) && builder.callstack_depth() == 1 {
                        Some(1)
                    } else {
                        None
                    },
                );

                if branch_reason == Class::Call || returning_levels.is_none() {
                    self.process_call_event(builder, block_sym)
                } else if let Some(levels) = returning_levels {
                    self.process_return_event(builder, levels)
                } else {
                    BuilderResult::Builder(builder)
                }
            } else if branch_reason == Class::Call {
                // function was probably called recursively
                self.process_call_event(builder, block_sym)
            } else {
                BuilderResult::Builder(builder)
            };

            match builder_result {
                BuilderResult::Builder(new_builder) => {
                    self.builder = Some(BuilderState::InProgress(new_builder));
                }
                BuilderResult::Completed(trace) => {
                    info!("Trace completed at time {}!", self.cur_metrics);
                    return Some(trace);
                }
            }
        } else if block_sym.vaddr_start != 0 && self.target_symbol.is_match(&block_sym.name) {
            info!("Starting trace: symbol={}", block_sym.name);
            let mut new_builder = TraceBuilder::new(self.cur_metrics, block_sym);
            new_builder.new_event(
                DataCollectionError::ID,
                DataCollectionError::NAME.to_string(),
                DataCollectionError::DESC.to_string(),
            );
            new_builder.new_event(
                TraceInterrupted::ID,
                TraceInterrupted::NAME.to_string(),
                TraceInterrupted::DESC.to_string(),
            );
            self.builder = Some(BuilderState::InProgress(new_builder));
        }
        None
    }

    fn update_state(&mut self, status: Status) {
        if status.eos() {
            self.decoder_state = DecoderState::Finished;
        } else if status.event_pending() {
            self.decoder_state = DecoderState::ReadEvent {
                continue_resync: false,
            };
        } else {
            self.decoder_state = DecoderState::ReadBlock;
        }
    }

    fn produce_trace(&mut self) -> Result<Option<Trace>, PtError> {
        loop {
            if let Some(progress_tracker) = &mut self.progress_tracker
                && let Ok(cur_offset) = self.decoder.offset()
            {
                progress_tracker.bar.set_position(cur_offset);
            }

            match self.decoder_state {
                DecoderState::Finished => {
                    return Ok(None);
                }
                DecoderState::Resync => {
                    self.branch_reason = Class::Unknown;
                    match self.decoder.resync() {
                        Ok(new_status) => {
                            if let Ok((tsc, _, _)) = self.decoder.time() {
                                self.update_cur_metrics(tsc, 0);
                            }
                            // resync() returns ip_suppressed to indicate it resync-ed in an
                            // area where tracing was disabled
                            if new_status.ip_supressed() {
                                self.disable_tracing(self.cur_metrics);
                            }
                            self.update_state(new_status);
                        }
                        Err(err) => match err.code() {
                            PtErrorCode::EventIgnored => {
                                self.decoder_state = DecoderState::ReadEvent {
                                    continue_resync: true,
                                };
                            }
                            PtErrorCode::Eos => {
                                self.decoder_state = DecoderState::Finished;
                            }
                            _ => {
                                error!("Unexpected error when resyncing: {:?}", err);
                                self.decoder_state = DecoderState::Finished;
                                return Err(err);
                            }
                        },
                    }
                }
                DecoderState::SyncForward => {
                    self.branch_reason = Class::Unknown;
                    if self.builder.is_some() {
                        error!("Discarding trace as we sync to the next PSB!");
                        self.builder = None;
                    }
                    // only fails if a bad packet is encountered, which is unrecoverable
                    let status = self.decoder.sync_forward()?;
                    self.update_state(status);
                }
                DecoderState::ReadEvent { continue_resync } => {
                    match self.decoder.event() {
                        Ok((event, status)) => {
                            self.update_state(status);
                            if !matches!(self.decoder_state, DecoderState::Finished) {
                                if !event.status_update() {
                                    self.process_event(event);
                                }
                                if continue_resync {
                                    self.decoder_state = DecoderState::Resync;
                                }
                            }
                        }
                        Err(err) => {
                            if matches!(err.code(), PtErrorCode::BadQuery) {
                                // shouldn't happen since we know an event is pending
                            } else {
                                // other errors shouldn't be possible unless something
                                // very wrong happened
                                error!("Unexpected error when decoding event: {:?}", err);
                                self.decoder_state = DecoderState::Finished;
                                return Err(err);
                            }
                        }
                    }
                }
                DecoderState::ReadBlock => {
                    match self.decoder.decode_next() {
                        Ok((block, status)) => {
                            self.update_state(status);
                            let (tsc, _, _) = self.decoder.time()?;
                            self.update_cur_metrics(tsc, block.ninsn() as u64);
                            if let Some(finished_trace) = self.process_block(block) {
                                return Ok(Some(finished_trace));
                            }
                        }
                        Err(err) => {
                            match err.code() {
                                PtErrorCode::BadContext
                                | PtErrorCode::BadOpc
                                | PtErrorCode::BadPacket => {
                                    error!(
                                        "Unrecoverable decoder error, trace data is likely corrupted! {:?}",
                                        err
                                    );
                                    // these indicate the data stream is corrupted
                                    self.decoder_state = DecoderState::Finished;
                                    return Err(err);
                                }
                                PtErrorCode::Nosync | PtErrorCode::RetstackEmpty => {
                                    error!(
                                        "Recoverable decoder error, but must sync to PSB! {:?}",
                                        err
                                    );
                                    self.decoder_state = DecoderState::SyncForward;
                                }
                                PtErrorCode::Eos => {
                                    self.decoder_state = DecoderState::Finished;
                                }
                                PtErrorCode::Nomap
                                | PtErrorCode::BadInsn
                                | PtErrorCode::BadImage
                                | PtErrorCode::BadRetcomp
                                | PtErrorCode::BadQuery => {
                                    warn!("Recoverable decoder error! {:?}", err);
                                    // these errors likely indicate that the memory image is outdated or incomplete
                                    // trace data should be recoverable, so we can just resync at the next known IP
                                    self.decoder_state = DecoderState::Resync;
                                }
                                _ => {
                                    // these other errors shouldn't be possible?
                                    error!("Unexpected error when decoding block: {:?}", err);
                                    self.decoder_state = DecoderState::Finished;
                                    return Err(err);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

impl<I: ImageProvider, F: Fn(usize) -> String> FallibleIterator for Transformer<'_, '_, '_, I, F> {
    type Item = Trace;
    type Error = PtError;

    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        if let Some(traces_left) = &mut self.traces_left {
            if *traces_left == 0 {
                self.decoder_state = DecoderState::Finished;
            } else {
                *traces_left -= 1;
            }
        }
        let result = self.produce_trace();
        if let Some(mut progress_tracker) = self.progress_tracker.take() {
            match &result {
                Ok(Some(_)) => {
                    progress_tracker.incr_produced();
                    self.progress_tracker = Some(progress_tracker);
                }
                _ => {
                    progress_tracker.bar.finish();
                    self.progress_tracker = None;
                    eprintln!(
                        "\n\nProduced {} traces in {:2?}",
                        progress_tracker.num_produced,
                        progress_tracker.bar.elapsed()
                    );
                }
            };
        }
        result
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TscConversion {
    pub time_shift: u16,
    pub time_mult: u32,
}

impl TscConversion {
    pub fn convert(&self, tsc: u64) -> u64 {
        let product = (tsc as u128) * (self.time_mult as u128);
        (product >> self.time_shift) as u64
    }
}

pub struct ProgressTracker<F: Fn(usize) -> String> {
    bar: ProgressBar,
    produced_fn: F,
    num_produced: usize,
}

impl<F: Fn(usize) -> String> ProgressTracker<F> {
    pub fn new(produce_msg: F) -> Self {
        let bar = ProgressBar::new(0);
        bar.set_style(
            ProgressStyle::with_template(PROGRESS_STYLE_TEMPLATE)
                .expect("Failed to produce ProgressStyle"),
        );
        bar.set_message(produce_msg(0));
        Self {
            bar,
            produced_fn: produce_msg,
            num_produced: 0,
        }
    }

    fn incr_produced(&mut self) {
        self.num_produced += 1;
        self.bar.set_message((self.produced_fn)(self.num_produced));
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TraceMetadata {
    pub cpu_info: Cpu,
    pub frequency_info: Frequency,
    pub tsc_info: TscConversion,
}

#[allow(clippy::too_many_arguments)]
pub fn convert<'a, 'b: 'a, 'c, I: ImageProvider + 'a + 'c, F: 'a + Fn(usize) -> String>(
    data: &'a mut [u8],
    decoder_image: &'b mut Image<'c>,
    image_provider: I,
    metadata: &TraceMetadata,
    target_symbol: &Regex,
    frame_completion_options: FrameCompletionOptions,
    max_traces: Option<usize>,
    mut progress_tracker: Option<ProgressTracker<F>>,
) -> Result<impl FallibleIterator<Item = Trace, Error = PtError> + 'a, PtError> {
    let builder = EncoderDecoderBuilder::<BlockDecoder>::new()
        .cpu(metadata.cpu_info)
        .freq(metadata.frequency_info);
    let builder = unsafe { builder.buffer_from_raw(data.as_mut_ptr(), data.len()) };

    let builder = builder
        .set_end_on_call(true)
        .set_enable_tick_events(true)
        .set_end_on_jump(true)
        .set_keep_tcal_on_ovf(true)
        .set_enable_iflags_events(false);

    let mut decoder = builder.build()?;

    let image_provider_cell = Rc::new(RefCell::new(image_provider));

    let image_provider_clone = image_provider_cell.clone();
    decoder_image.set_callback(Some(
        move |buf: &mut [u8], vaddr, _| match image_provider_clone
            .borrow_mut()
            .read_chunk(buf, vaddr)
        {
            Ok(bytes) => bytes as i32,
            Err(err) => {
                warn!("Error reading memory chunk: {}", err);
                -(PtErrorCode::Nomap as i32)
            }
        },
    ));
    decoder.set_image(Some(decoder_image))?;

    if let Some(progress_tracker) = &mut progress_tracker {
        progress_tracker.bar.set_length(data.len() as u64);
    }

    Transformer::new(
        decoder,
        data,
        image_provider_cell,
        metadata.tsc_info,
        target_symbol.to_owned(),
        frame_completion_options,
        max_traces,
        progress_tracker,
    )
}
