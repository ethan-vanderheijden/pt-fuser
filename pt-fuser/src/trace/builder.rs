use crate::trace::{self, Event, Frame, Metrics, MetricsRange, SymbolInfo, Trace};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct IncompleteFrame {
    start_metrics: Metrics,
    child_frames: Vec<Frame>,
    symbol: SymbolInfo,
}

impl IncompleteFrame {
    fn complete(self, end_metrics: Metrics) -> Result<Frame, trace::Error> {
        let mut completed = Frame::new(
            MetricsRange::from(self.start_metrics, end_metrics),
            self.symbol,
        );
        for child in self.child_frames.into_iter().rev() {
            completed.add_child(child)?;
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

impl TraceBuilder {
    fn ensure_monotonic(&self, new_metrics: Metrics) {
        if new_metrics.ts < self.last_metrics.ts
            || new_metrics.cycles < self.last_metrics.cycles
            || new_metrics.insn_count < self.last_metrics.insn_count
        {
            panic!(
                "Metrics must increase monotonically. Previous: {}, New: {}",
                self.last_metrics, new_metrics
            );
        }
    }

    pub fn new(start_metrics: Metrics, symbol: SymbolInfo) -> Self {
        Self {
            last_metrics: start_metrics,
            current_frame: IncompleteFrame {
                start_metrics,
                child_frames: Vec::new(),
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
            symbol,
        };
        let old_frame = std::mem::replace(&mut self.current_frame, new_frame);
        self.callstack.push(old_frame);
        self.last_metrics = metrics;
    }

    pub fn complete_frame(mut self, end_metrics: Metrics) -> Result<BuilderResult, trace::Error> {
        self.ensure_monotonic(end_metrics);
        if self.callstack.is_empty() {
            let completed_frame = self.current_frame.complete(end_metrics)?;
            Ok(BuilderResult::Completed(Trace::new(
                completed_frame,
                self.events,
            )))
        } else {
            let prev = self.callstack.pop().unwrap();
            let current_frame = std::mem::replace(&mut self.current_frame, prev);
            let completed_frame = current_frame.complete(end_metrics)?;
            self.current_frame.child_frames.push(completed_frame);
            self.last_metrics = end_metrics;
            Ok(BuilderResult::Builder(self))
        }
    }

    pub fn push_event(&mut self, metrics: Metrics, description: String) {
        self.ensure_monotonic(metrics);
        self.events.push(Event {
            metrics,
            description,
        });
        self.last_metrics = metrics;
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
}

pub enum BuilderResult {
    Builder(TraceBuilder),
    Completed(Trace),
}

#[cfg(test)]
mod test {
    use super::*;
    use trace::test::{INNER_RANGE1, INNER_RANGE2, METRICS_ONE, SAMPLE_RANGE, TEST_SYMBOL};

    #[test]
    fn complete_empty_frame() {
        let incomplete = IncompleteFrame {
            start_metrics: SAMPLE_RANGE.start,
            child_frames: Vec::new(),
            symbol: TEST_SYMBOL.clone(),
        };
        let completed = incomplete.complete(SAMPLE_RANGE.end).unwrap();
        assert_eq!(completed.chunks.len(), 1);
        assert!(completed.check_invariant());
    }

    #[test]
    fn complete_frame_with_chunks() {
        let inner1 = Frame::new(INNER_RANGE1, TEST_SYMBOL.clone());
        let inner2 = Frame::new(INNER_RANGE2, TEST_SYMBOL.clone());
        let incomplete = IncompleteFrame {
            start_metrics: SAMPLE_RANGE.start,
            child_frames: vec![inner1, inner2],
            symbol: TEST_SYMBOL.clone(),
        };
        let completed = incomplete.complete(SAMPLE_RANGE.end).unwrap();
        assert_eq!(completed.chunks.len(), 5);
        assert!(completed.check_invariant());
    }

    #[test]
    fn build_trace_simple() {
        let builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        let result = builder.complete_frame(SAMPLE_RANGE.end).unwrap();
        match result {
            BuilderResult::Completed(trace) => {
                assert_eq!(trace.root.chunks.len(), 1);
                assert_eq!(trace.root.chunks[0].total_time(), SAMPLE_RANGE.total_time());
            }
            BuilderResult::Builder(_) => panic!("Expected trace to be completed"),
        }
    }

    fn extract_builder(result: BuilderResult) -> TraceBuilder {
        match result {
            BuilderResult::Builder(builder) => builder,
            BuilderResult::Completed(_) => panic!("Expected builder, got completed trace"),
        }
    }

    #[test]
    fn build_trace_nested() {
        let mut builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        builder.push_frame(INNER_RANGE1.start, TEST_SYMBOL.clone());
        let mut builder = extract_builder(builder.complete_frame(INNER_RANGE1.end).unwrap());
        builder.push_frame(INNER_RANGE2.start, TEST_SYMBOL.clone());
        builder.push_frame(INNER_RANGE2.start, TEST_SYMBOL.clone());
        let builder = extract_builder(builder.complete_frame(INNER_RANGE2.end).unwrap());
        let builder = extract_builder(builder.complete_frame(SAMPLE_RANGE.end).unwrap());
        match builder.complete_frame(SAMPLE_RANGE.end).unwrap() {
            BuilderResult::Completed(trace) => {
                assert_eq!(trace.root.chunks.len(), 4);
                assert!(matches!(
                    &trace.root.chunks[0],
                    trace::Chunk::Straightline(_)
                ));
                assert!(matches!(
                    &trace.root.chunks[2],
                    trace::Chunk::Straightline(_)
                ));

                match &trace.root.chunks[1] {
                    trace::Chunk::Frame(frame) => {
                        assert_eq!(frame.metrics, INNER_RANGE1);
                        assert_eq!(frame.chunks.len(), 1);
                        assert!(matches!(&frame.chunks[0], trace::Chunk::Straightline(_)));
                    }
                    _ => panic!("Expected frame chunk in position 1"),
                }

                match &trace.root.chunks[3] {
                    trace::Chunk::Frame(frame) => {
                        assert_eq!(
                            frame.metrics,
                            MetricsRange::from(INNER_RANGE2.start, SAMPLE_RANGE.end)
                        );
                        assert_eq!(frame.chunks.len(), 2);
                        assert!(matches!(&frame.chunks[1], trace::Chunk::Straightline(_)));

                        match &frame.chunks[0] {
                            trace::Chunk::Frame(inner_frame) => {
                                assert_eq!(inner_frame.metrics, INNER_RANGE2);
                                assert_eq!(inner_frame.chunks.len(), 1);
                                assert!(matches!(
                                    &inner_frame.chunks[0],
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
    fn add_events() {
        let mut builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        builder.push_event(INNER_RANGE1.start, "Event 1".to_string());
        builder.push_event(INNER_RANGE2.start, "Event 2".to_string());
        let result = builder.complete_frame(SAMPLE_RANGE.end).unwrap();
        match result {
            BuilderResult::Completed(trace) => {
                assert_eq!(trace.get_events().len(), 2);
                assert_eq!(trace.get_events()[0].metrics, INNER_RANGE1.start);
                assert_eq!(trace.get_events()[1].metrics, INNER_RANGE2.start);
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
        builder.push_frame(INNER_RANGE1.start + METRICS_ONE, SymbolInfo {
            name: "3rd level".to_string(),
            offset: 0,
            size: 0,
        });
        assert_eq!(builder.get_frame_symbol(0).name, "3rd level");
        assert_eq!(builder.get_frame_symbol(1).name, "2nd level");
        assert_eq!(builder.get_frame_symbol(2).name, "top level");
    }

    #[test]
    #[should_panic]
    fn non_monotonic_fails1() {
        let mut builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        builder.push_frame(SAMPLE_RANGE.start - METRICS_ONE, TEST_SYMBOL.clone());
    }

    #[test]
    #[should_panic]
    fn non_monotonic_fails2() {
        let mut builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        builder.push_event(INNER_RANGE1.start, "Event 1".to_string());
        builder.push_event(INNER_RANGE1.start - METRICS_ONE, "Event 2".to_string());
    }

    #[test]
    #[should_panic]
    fn non_monotonic_fails3() {
        let mut builder = TraceBuilder::new(SAMPLE_RANGE.start, TEST_SYMBOL.clone());
        builder.push_frame(INNER_RANGE2.start, TEST_SYMBOL.clone());
        assert!(
            builder
                .complete_frame(INNER_RANGE2.start - METRICS_ONE)
                .is_ok()
        );
    }
}
