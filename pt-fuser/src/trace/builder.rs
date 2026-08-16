use crate::trace::{self, Event, Frame, Metrics, MetricsRange, SymbolInfo, Trace};

pub struct FrameCompletionOptions {
    // sometimes, calling a function first goes to the Procedure Linkage Table (PLT) stub, which
    // immediately jumps to the target function. We can remove the PLT stub frame from the trace.
    pub remove_plt_stubs: bool,
}

impl Default for FrameCompletionOptions {
    fn default() -> Self {
        FrameCompletionOptions {
            remove_plt_stubs: false,
        }
    }
}

fn is_plt_stub(parent_symbol: &SymbolInfo, child_symbol: &SymbolInfo) -> bool {
    parent_symbol.name.ends_with("@plt")
        && child_symbol.name == parent_symbol.name.trim_end_matches("@plt")
}

#[derive(Debug, Clone, PartialEq)]
struct IncompleteFrame {
    start_metrics: Metrics,
    child_frames: Vec<Frame>,
    pauses: Vec<MetricsRange>,
    symbol: SymbolInfo,
}

impl IncompleteFrame {
    fn complete(
        mut self,
        end_metrics: Metrics,
        options: FrameCompletionOptions,
    ) -> Result<Frame, trace::Error> {
        if options.remove_plt_stubs
            && self.child_frames.len() == 1
            && self.pauses.is_empty()
            && is_plt_stub(&self.symbol, &self.child_frames[0].symbol)
        {
            let mut child = self.child_frames.remove(0);
            child.metrics = MetricsRange::new(self.start_metrics, end_metrics);
            return Ok(child);
        }

        let mut completed = Frame::new(
            MetricsRange::new(self.start_metrics, end_metrics),
            self.symbol,
        );

        while !self.child_frames.is_empty() && !self.pauses.is_empty() {
            let child = self.child_frames.first().unwrap();
            let pause = self.pauses.first().unwrap();
            if child.metrics.start < pause.start {
                completed.add_child(self.child_frames.remove(0))?;
            } else {
                completed.add_pause(self.pauses.remove(0))?;
            }
        }

        for child in self.child_frames.into_iter() {
            completed.add_child(child)?;
        }
        for pause in self.pauses.into_iter() {
            completed.add_pause(pause)?;
        }

        Ok(completed)
    }
}

#[derive(Debug)]
pub struct TraceBuilder {
    last_metrics: Metrics,
    current_frame: IncompleteFrame,
    callstack: Vec<IncompleteFrame>,
    events: Vec<Event>,
}

#[derive(Debug)]
pub struct PausedTraceBuilder {
    inner: TraceBuilder,
    pause_start: Metrics,
}

impl TraceBuilder {
    fn ensure_monotonic(&mut self, new_metrics: Metrics) {
        if new_metrics.ts < self.last_metrics.ts
            || new_metrics.cycles < self.last_metrics.cycles
            || new_metrics.insn_count < self.last_metrics.insn_count
        {
            panic!(
                "Metrics must increase monotonically. Previous: {}, New: {}",
                self.last_metrics, new_metrics
            );
        }
        self.last_metrics = new_metrics;
    }

    pub fn new(start_metrics: Metrics, symbol: SymbolInfo) -> Self {
        TraceBuilder {
            last_metrics: start_metrics,
            current_frame: IncompleteFrame {
                start_metrics,
                child_frames: Vec::new(),
                pauses: Vec::new(),
                symbol,
            },
            callstack: Vec::new(),
            events: Vec::new(),
        }
    }

    pub fn push_frame(&mut self, metrics: Metrics, symbol: SymbolInfo) {
        self.ensure_monotonic(metrics);
        let new_frame = IncompleteFrame {
            start_metrics: metrics,
            child_frames: Vec::new(),
            pauses: Vec::new(),
            symbol,
        };
        let old_frame = std::mem::replace(&mut self.current_frame, new_frame);
        self.callstack.push(old_frame);
    }

    pub fn complete_frame(
        mut self,
        end_metrics: Metrics,
        options: Option<FrameCompletionOptions>,
    ) -> Result<BuilderResult, trace::Error> {
        self.ensure_monotonic(end_metrics);
        let options = options.unwrap_or_default();
        if self.callstack.is_empty() {
            let completed_frame = self.current_frame.complete(end_metrics, options)?;
            Ok(BuilderResult::Completed(Trace::new(
                completed_frame,
                self.events,
            )))
        } else {
            let prev = self.callstack.pop().unwrap();
            let current_frame = std::mem::replace(&mut self.current_frame, prev);
            let completed_frame = current_frame.complete(end_metrics, options)?;
            self.current_frame.child_frames.push(completed_frame);
            Ok(BuilderResult::Builder(self))
        }
    }

    pub fn new_event(&mut self, id: u32, name: String, description: String) {
        self.events.push(Event::new(id, name, description));
    }

    pub fn event_occured(&mut self, event_id: u32, metrics: Metrics) {
        for event in self.events.iter_mut() {
            if event.id == event_id {
                event.add_occurence(metrics);
            }
        }
    }

    pub fn callstack_depth(&self) -> usize {
        self.callstack.len() + 1
    }

    /// index = 0 means top of the callstack. Higher indices go down the callstack.
    pub fn get_frame_symbol(&self, index: usize) -> &SymbolInfo {
        if index == 0 {
            &self.current_frame.symbol
        } else {
            &self.callstack[self.callstack.len() - index].symbol
        }
    }

    pub fn pause(self, metrics: Metrics) -> Result<PausedTraceBuilder, trace::Error> {
        Ok(PausedTraceBuilder {
            inner: self,
            pause_start: metrics,
        })
    }
}

impl PausedTraceBuilder {
    pub fn resume(mut self, metrics: Metrics) -> TraceBuilder {
        self.inner.ensure_monotonic(metrics);
        self.inner
            .current_frame
            .pauses
            .push(MetricsRange::new(self.pause_start, metrics));
        self.inner
    }
}

pub enum BuilderResult {
    Builder(TraceBuilder),
    Completed(Trace),
}

#[cfg(test)]
mod test {
    use super::*;
    use trace::{
        Chunk,
        test::{INNER_RANGE1, INNER_RANGE2, METRICS_ONE, SAMPLE_RANGE, TEST_SYMBOL},
    };

    fn extract_builder(result: BuilderResult) -> TraceBuilder {
        match result {
            BuilderResult::Builder(builder) => builder,
            BuilderResult::Completed(_) => panic!("Expected builder, got completed trace"),
        }
    }

    fn extract_pause_chunk(chunk: &Chunk) -> &MetricsRange {
        match chunk {
            Chunk::Pause(pause) => pause,
            _ => panic!("Expected pause chunk"),
        }
    }

    fn extract_frame_chunk(chunk: &Chunk) -> &Frame {
        match chunk {
            Chunk::Frame(frame) => frame,
            _ => panic!("Expected frame chunk"),
        }
    }

    #[test]
    fn complete_empty_frame() {
        let incomplete = IncompleteFrame {
            start_metrics: SAMPLE_RANGE.start,
            child_frames: Vec::new(),
            pauses: Vec::new(),
            symbol: TEST_SYMBOL.clone(),
        };
        let completed = incomplete
            .complete(SAMPLE_RANGE.end, FrameCompletionOptions::default())
            .unwrap();
        assert_eq!(completed.chunks().len(), 1);
        assert!(completed.check_invariant());
    }

    #[test]
    fn complete_frame_with_chunks() {
        let inner1 = Frame::new(INNER_RANGE1, TEST_SYMBOL.clone());
        let inner2 = Frame::new(INNER_RANGE2, TEST_SYMBOL.clone());
        let incomplete = IncompleteFrame {
            start_metrics: SAMPLE_RANGE.start,
            child_frames: vec![inner1, inner2],
            pauses: Vec::new(),
            symbol: TEST_SYMBOL.clone(),
        };
        let completed = incomplete
            .complete(SAMPLE_RANGE.end, FrameCompletionOptions::default())
            .unwrap();
        assert_eq!(completed.chunks().len(), 5);
        assert!(completed.check_invariant());
    }

    #[test]
    fn complete_frame_with_pauses() {
        let pause1 = MetricsRange::new(INNER_RANGE1.start, INNER_RANGE1.start + METRICS_ONE);
        let inner_frame = Frame::new(
            MetricsRange::new(INNER_RANGE1.end - METRICS_ONE, INNER_RANGE1.end),
            TEST_SYMBOL.clone(),
        );
        let pause2 = MetricsRange::new(INNER_RANGE2.start, INNER_RANGE2.end);
        let incomplete = IncompleteFrame {
            start_metrics: SAMPLE_RANGE.start,
            child_frames: vec![inner_frame],
            pauses: vec![pause1, pause2],
            symbol: TEST_SYMBOL.clone(),
        };
        let completed = incomplete
            .complete(SAMPLE_RANGE.end, FrameCompletionOptions::default())
            .unwrap();
        assert_eq!(completed.chunks().len(), 7);
        assert!(completed.check_invariant());
        let _ = extract_pause_chunk(&completed.chunks()[1]);
        let _ = extract_frame_chunk(&completed.chunks()[3]);
        let _ = extract_pause_chunk(&completed.chunks()[5]);
    }

    #[test]
    fn complete_without_plt_stub() {
        let inner_frame = Frame::new(
            MetricsRange::new(INNER_RANGE1.start, INNER_RANGE1.end),
            SymbolInfo {
                name: "my_func".to_string(),
                offset: 0,
                size: 0,
            },
        );
        let incomplete = IncompleteFrame {
            start_metrics: SAMPLE_RANGE.start,
            child_frames: vec![inner_frame],
            pauses: Vec::new(),
            symbol: SymbolInfo {
                name: "my_func@plt".to_string(),
                offset: 0,
                size: 0,
            },
        };
        let completed = incomplete
            .complete(
                SAMPLE_RANGE.end,
                FrameCompletionOptions {
                    remove_plt_stubs: true,
                },
            )
            .unwrap();
        assert_eq!(completed.symbol.name, "my_func");
        assert_eq!(completed.metrics, SAMPLE_RANGE);
        assert_eq!(completed.chunks().len(), 1);
    }

    #[test]
    fn build_trace_simple() {
        let builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        let result = builder.complete_frame(SAMPLE_RANGE.end, None).unwrap();
        match result {
            BuilderResult::Completed(trace) => {
                assert_eq!(trace.root.metrics, SAMPLE_RANGE);
                assert_eq!(trace.root.chunks().len(), 1);
                assert!(matches!(
                    &trace.root.chunks()[0],
                    trace::Chunk::Straightline(_)
                ));
            }
            BuilderResult::Builder(_) => panic!("Expected trace to be completed"),
        }
    }

    #[test]
    fn build_trace_nested() {
        let mut builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        builder.push_frame(INNER_RANGE1.start, TEST_SYMBOL.clone());
        let mut builder = extract_builder(builder.complete_frame(INNER_RANGE1.end, None).unwrap());
        builder.push_frame(INNER_RANGE2.start, TEST_SYMBOL.clone());
        builder.push_frame(INNER_RANGE2.start, TEST_SYMBOL.clone());
        let builder = extract_builder(builder.complete_frame(INNER_RANGE2.end, None).unwrap());
        let builder = extract_builder(builder.complete_frame(SAMPLE_RANGE.end, None).unwrap());
        match builder.complete_frame(SAMPLE_RANGE.end, None).unwrap() {
            BuilderResult::Completed(trace) => {
                assert_eq!(trace.root.chunks().len(), 4);
                assert!(matches!(
                    &trace.root.chunks()[0],
                    trace::Chunk::Straightline(_)
                ));
                assert!(matches!(
                    &trace.root.chunks()[2],
                    trace::Chunk::Straightline(_)
                ));

                match &trace.root.chunks()[1] {
                    trace::Chunk::Frame(frame) => {
                        assert_eq!(frame.metrics, INNER_RANGE1);
                        assert_eq!(frame.chunks().len(), 1);
                        assert!(matches!(&frame.chunks()[0], trace::Chunk::Straightline(_)));
                    }
                    _ => panic!("Expected frame chunk in position 1"),
                }

                match &trace.root.chunks()[3] {
                    trace::Chunk::Frame(frame) => {
                        assert_eq!(
                            frame.metrics,
                            MetricsRange::new(INNER_RANGE2.start, SAMPLE_RANGE.end)
                        );
                        assert_eq!(frame.chunks().len(), 2);
                        assert!(matches!(&frame.chunks()[1], trace::Chunk::Straightline(_)));

                        match &frame.chunks()[0] {
                            trace::Chunk::Frame(inner_frame) => {
                                assert_eq!(inner_frame.metrics, INNER_RANGE2);
                                assert_eq!(inner_frame.chunks().len(), 1);
                                assert!(matches!(
                                    &inner_frame.chunks()[0],
                                    trace::Chunk::Straightline(_)
                                ));
                            }
                            _ => panic!("Expected frame chunk in nested position 0"),
                        }
                    }
                    _ => panic!("Expected frame chunk in position 3"),
                }
            }
            BuilderResult::Builder(_) => panic!("Expected trace to be completed"),
        }
    }

    #[test]
    fn build_trace_pauses() {
        let builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        let paused = builder.pause(SAMPLE_RANGE.start + METRICS_ONE).unwrap();
        let mut resumed = paused.resume(INNER_RANGE1.start);

        resumed.push_frame(INNER_RANGE1.start + METRICS_ONE, TEST_SYMBOL.clone());
        let paused = resumed.pause(INNER_RANGE1.end).unwrap();

        let resumed = paused.resume(INNER_RANGE2.start);
        let builder = extract_builder(
            resumed
                .complete_frame(INNER_RANGE2.end - METRICS_ONE, None)
                .unwrap(),
        );

        match builder.complete_frame(INNER_RANGE2.end, None).unwrap() {
            BuilderResult::Builder(_) => panic!("Expected completed trace"),
            BuilderResult::Completed(trace) => {
                assert_eq!(trace.root_frame().chunks().len(), 5);
                assert!(matches!(
                    &trace.root_frame().chunks()[0],
                    trace::Chunk::Straightline(_)
                ));
                assert!(matches!(
                    &trace.root_frame().chunks()[2],
                    trace::Chunk::Straightline(_)
                ));
                assert!(matches!(
                    &trace.root_frame().chunks()[4],
                    trace::Chunk::Straightline(_)
                ));

                let pause = extract_pause_chunk(&trace.root_frame().chunks()[1]);
                assert_eq!(
                    pause,
                    &MetricsRange::new(SAMPLE_RANGE.start + METRICS_ONE, INNER_RANGE1.start)
                );

                let frame = extract_frame_chunk(&trace.root_frame().chunks()[3]);
                assert_eq!(
                    frame.metrics,
                    MetricsRange::new(
                        INNER_RANGE1.start + METRICS_ONE,
                        INNER_RANGE2.end - METRICS_ONE
                    )
                );
                assert_eq!(frame.chunks().len(), 3);
                assert!(matches!(
                    &trace.root_frame().chunks()[0],
                    trace::Chunk::Straightline(_)
                ));
                assert!(matches!(
                    &trace.root_frame().chunks()[2],
                    trace::Chunk::Straightline(_)
                ));

                let nested_pause = extract_pause_chunk(&frame.chunks()[1]);
                assert_eq!(
                    nested_pause,
                    &MetricsRange::new(INNER_RANGE1.end, INNER_RANGE2.start)
                );
            }
        }
    }

    #[test]
    fn build_without_plt_stubs() {
        let mut builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        builder.push_frame(
            INNER_RANGE1.start,
            SymbolInfo {
                name: "my_func@plt".to_string(),
                offset: 0,
                size: 0,
            },
        );
        builder.push_frame(
            INNER_RANGE1.start + METRICS_ONE,
            SymbolInfo {
                name: "my_func".to_string(),
                offset: 0,
                size: 0,
            },
        );
        let builder = extract_builder(
            builder
                .complete_frame(INNER_RANGE1.end, Some(FrameCompletionOptions {
                    remove_plt_stubs: true,
                }))
                .unwrap(),
        );
        let builder = extract_builder(
            builder
                .complete_frame(INNER_RANGE1.end, Some(FrameCompletionOptions {
                    remove_plt_stubs: true,
                }))
                .unwrap(),
        );
        let final_result = builder.complete_frame(SAMPLE_RANGE.end, None).unwrap();
        match final_result {
            BuilderResult::Completed(trace) => {
                assert_eq!(trace.root_frame().chunks().len(), 3);
                let frame = extract_frame_chunk(&trace.root_frame().chunks()[1]);
                assert_eq!(frame.symbol.name, "my_func");
                assert_eq!(frame.metrics, MetricsRange::new(INNER_RANGE1.start, INNER_RANGE1.end));
                assert_eq!(frame.chunks().len(), 1);
            }
            BuilderResult::Builder(_) => panic!("Expected trace to be completed"),
        }
    }

    #[test]
    fn add_events() {
        let mut builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        builder.new_event(10, "Event 1".to_string(), "Description 1".to_string());
        builder.new_event(20, "Event 2".to_string(), "Description 2".to_string());

        builder.event_occured(10, INNER_RANGE2.start);
        builder.event_occured(20, INNER_RANGE1.end);
        builder.event_occured(10, INNER_RANGE1.start);

        let result = builder.complete_frame(SAMPLE_RANGE.end, None).unwrap();
        match result {
            BuilderResult::Completed(trace) => {
                assert_eq!(trace.events.len(), 2);
                if trace.events[0].id == 10 && trace.events[1].id == 20 {
                    assert_eq!(trace.events[0].occurences().len(), 2);
                    assert_eq!(trace.events[1].occurences().len(), 1);
                } else if trace.events[0].id == 20 && trace.events[1].id == 10 {
                    assert_eq!(trace.events[0].occurences().len(), 1);
                    assert_eq!(trace.events[1].occurences().len(), 2);
                } else {
                    panic!("Unexpected event IDs");
                }
            }
            BuilderResult::Builder(_) => panic!("Expected trace to be completed"),
        }
    }

    #[test]
    fn frame_symbol_order() {
        let mut builder = TraceBuilder::new(
            SAMPLE_RANGE.start,
            SymbolInfo {
                name: "top level".to_string(),
                offset: 0,
                size: 0,
            },
        );
        builder.push_frame(
            INNER_RANGE1.start,
            SymbolInfo {
                name: "2nd level".to_string(),
                offset: 0,
                size: 0,
            },
        );
        builder.push_frame(
            INNER_RANGE1.start + METRICS_ONE,
            SymbolInfo {
                name: "3rd level".to_string(),
                offset: 0,
                size: 0,
            },
        );
        assert_eq!(builder.get_frame_symbol(0).name, "3rd level");
        assert_eq!(builder.get_frame_symbol(1).name, "2nd level");
        assert_eq!(builder.get_frame_symbol(2).name, "top level");
    }

    #[test]
    #[should_panic]
    fn non_monotonic_fails() {
        let mut builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        builder.push_frame(SAMPLE_RANGE.start - METRICS_ONE, TEST_SYMBOL.clone());
    }

    #[test]
    #[should_panic]
    fn non_monotonic_fails3() {
        let mut builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        builder.push_frame(INNER_RANGE2.start, TEST_SYMBOL.clone());
        assert!(
            builder
                .complete_frame(INNER_RANGE2.start - METRICS_ONE, None)
                .is_ok()
        );
    }
}
