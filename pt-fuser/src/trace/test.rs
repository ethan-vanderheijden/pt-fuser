use std::sync::LazyLock;

use super::*;

pub(crate) const SAMPLE_RANGE_END: Metrics = Metrics {
    ts: 200,
    cycles: 350,
    insn_count: 1000,
};
pub(crate) const SAMPLE_RANGE: MetricsRange = MetricsRange::new(
    Metrics {
        ts: 100,
        cycles: 50,
        insn_count: 200,
    },
    &SAMPLE_RANGE_END,
);

pub(crate) const INNER_RANGE1_END: Metrics = Metrics {
    ts: 150,
    cycles: 150,
    insn_count: 500,
};
pub(crate) const INNER_RANGE1: MetricsRange = MetricsRange::new(
    Metrics {
        ts: 120,
        cycles: 70,
        insn_count: 300,
    },
    &INNER_RANGE1_END,
);

pub(crate) const INNER_RANGE2_END: Metrics = Metrics {
    ts: 190,
    cycles: 300,
    insn_count: 900,
};
pub(crate) const INNER_RANGE2: MetricsRange = MetricsRange::new(
    Metrics {
        ts: 160,
        cycles: 200,
        insn_count: 600,
    },
    &INNER_RANGE2_END,
);

pub(crate) const TEST_SYMBOL: LazyLock<Arc<SymbolInfo>> = LazyLock::new(|| {
    Arc::new(SymbolInfo {
        name: "test".to_string(),
        offset: 0x1000,
        size: 0x100,
    })
});

pub(crate) const METRICS_ONE: Metrics = Metrics {
    ts: 1,
    cycles: 1,
    insn_count: 1,
};

/// Creates a trace with no events and a root frame that has five chunks:
/// child frame, straightline, child frame, straightline, child frame
fn test_trace() -> Trace {
    let mut outer = Frame::new(SAMPLE_RANGE, 0, TEST_SYMBOL.clone());
    let middle = Frame::new(INNER_RANGE1, 0, TEST_SYMBOL.clone());
    outer.add_child(middle).unwrap();
    let beginning = Frame::new(
        MetricsRange::new(SAMPLE_RANGE.start, &(INNER_RANGE1.start - METRICS_ONE)),
        0,
        TEST_SYMBOL.clone(),
    );
    outer.add_child(beginning).unwrap();
    let end = Frame::new(
        MetricsRange::new(INNER_RANGE1_END + METRICS_ONE, &SAMPLE_RANGE_END),
        0,
        TEST_SYMBOL.clone(),
    );
    outer.add_child(end).unwrap();

    Trace::new(vec![TEST_SYMBOL.clone()], outer, vec![])
}

fn new_frame(range: MetricsRange) -> Frame {
    Frame::new(range, 0, TEST_SYMBOL.clone())
}

#[test]
fn range_totals() {
    let chunk = Chunk::Frame(&new_frame(SAMPLE_RANGE));
    assert_eq!(SAMPLE_RANGE.total_time(), 100);
    assert_eq!(SAMPLE_RANGE.total_cycles(), 300);
    assert_eq!(SAMPLE_RANGE.total_insn(), 800);
    assert_eq!(chunk.metrics().total_time(), SAMPLE_RANGE.total_time());
    assert_eq!(chunk.metrics().total_cycles(), SAMPLE_RANGE.total_cycles());
    assert_eq!(chunk.metrics().total_insn(), SAMPLE_RANGE.total_insn());
}

#[test]
fn zero_duration_frame() {
    let chunk = Chunk::Frame(&new_frame(MetricsRange::new(
        SAMPLE_RANGE.start,
        &SAMPLE_RANGE.start,
    )));
    assert_eq!(chunk.metrics().total_time(), 0);
    assert_eq!(chunk.metrics().total_cycles(), 0);
    assert_eq!(chunk.metrics().total_insn(), 0);
}

#[test]
fn empty_frame_invariant() {
    let frame = new_frame(SAMPLE_RANGE);
    assert!(frame.check_invariant());
}

#[test]
fn fails_invariant() {
    let mut frame = new_frame(SAMPLE_RANGE);
    let inner_frame = new_frame(MetricsRange::new(
        SAMPLE_RANGE.start - METRICS_ONE,
        &SAMPLE_RANGE_END,
    ));
    frame.chunks.push(inner_frame.into());
    assert!(!frame.check_invariant());
}

#[test]
fn child_frame_invariant() {
    let mut frame = new_frame(SAMPLE_RANGE);
    let child1 = new_frame(INNER_RANGE1);
    let child2 = new_frame(INNER_RANGE2);
    frame.add_child(child1).unwrap();
    frame.add_child(child2).unwrap();
    assert!(frame.check_invariant());
}

#[test]
fn child_overlaps_parent() {
    let mut outer = new_frame(SAMPLE_RANGE);
    let inner = new_frame(SAMPLE_RANGE);
    outer.add_child(inner).unwrap();
    assert_eq!(outer.chunks().count(), 1);
    assert!(outer.check_invariant());
}

#[test]
fn child_overlapping_complex() {
    let trace = test_trace();
    let outer = trace.root_frame();
    let chunks = outer.chunks().collect::<Vec<_>>();
    assert_eq!(chunks.len(), 5);
    assert!(outer.check_invariant());
    assert!(matches!(chunks[0], Chunk::Frame(_)));
    assert!(matches!(chunks[1], Chunk::Straightline(_)));
    assert!(matches!(chunks[2], Chunk::Frame(_)));
    assert!(matches!(chunks[3], Chunk::Straightline(_)));
    assert!(matches!(chunks[4], Chunk::Frame(_)));
}

#[test]
fn add_invalid_child() {
    let mut frame = new_frame(SAMPLE_RANGE);
    let too_early = new_frame(MetricsRange::new(
        SAMPLE_RANGE.start - METRICS_ONE,
        &INNER_RANGE1_END,
    ));
    let too_late = new_frame(MetricsRange::new(
        INNER_RANGE2.start,
        &(SAMPLE_RANGE_END + METRICS_ONE),
    ));
    assert!(frame.add_child(too_early).is_err());
    assert!(frame.add_child(too_late).is_err());
}

#[test]
fn add_child_no_space() {
    let mut outer = new_frame(SAMPLE_RANGE);
    let middle = new_frame(INNER_RANGE1);
    outer.add_child(middle).unwrap();
    let beginning = new_frame(MetricsRange::new(
        SAMPLE_RANGE.start + METRICS_ONE,
        &(INNER_RANGE1.start + METRICS_ONE),
    ));
    let end = new_frame(MetricsRange::new(
        INNER_RANGE1_END - METRICS_ONE,
        &(SAMPLE_RANGE_END - METRICS_ONE),
    ));
    assert!(outer.add_child(beginning).is_err());
    assert!(outer.add_child(end).is_err());
}

#[test]
fn add_child_instant() {
    let mut outer = new_frame(SAMPLE_RANGE);
    let child = new_frame(MetricsRange::new(
        SAMPLE_RANGE.start + METRICS_ONE,
        &(SAMPLE_RANGE.start + METRICS_ONE),
    ));
    assert!(outer.add_child(child).is_ok());
}

#[test]
fn add_child_nested_instant() {
    let mut outer = new_frame(SAMPLE_RANGE);
    let range = MetricsRange::new(
        SAMPLE_RANGE.start + METRICS_ONE,
        &(SAMPLE_RANGE.start + METRICS_ONE),
    );
    let mut child1 = new_frame(range.clone());
    let mut child2 = new_frame(range.clone());
    let child3 = new_frame(range.clone());
    assert!(child2.add_child(child3).is_ok());
    assert!(child1.add_child(child2).is_ok());
    assert!(outer.add_child(child1).is_ok());
}

#[test]
fn add_child_multiple_instant() {
    let mut outer = new_frame(SAMPLE_RANGE);
    let range = MetricsRange::new(
        SAMPLE_RANGE.start + METRICS_ONE,
        &(SAMPLE_RANGE.start + METRICS_ONE),
    );
    let child1 = new_frame(range.clone());
    let child2 = new_frame(range.clone());
    assert!(outer.add_child(child1).is_ok());
    assert!(outer.add_child(child2).is_ok());
}

#[test]
fn add_child_multiple_nested_instant() {
    let mut outer = new_frame(SAMPLE_RANGE);
    let range = MetricsRange::new(
        SAMPLE_RANGE.start + METRICS_ONE,
        &(SAMPLE_RANGE.start + METRICS_ONE),
    );
    let mut child1 = new_frame(range.clone());
    let child2 = new_frame(range.clone());
    let child3 = new_frame(range.clone());
    assert!(child1.add_child(child2).is_ok());
    assert!(child1.add_child(child3).is_ok());
    assert!(outer.add_child(child1).is_ok());
}

#[test]
fn add_child_adjacent_ends_no_straightline() {
    let mut outer = new_frame(SAMPLE_RANGE);
    let child1 = new_frame(SAMPLE_RANGE);
    let beginning = new_frame(MetricsRange::new(SAMPLE_RANGE.start, &SAMPLE_RANGE.start));
    let end = new_frame(MetricsRange::new(SAMPLE_RANGE_END, &SAMPLE_RANGE_END));
    outer.add_child(child1).unwrap();
    assert!(outer.add_child(beginning).is_ok());
    assert!(outer.add_child(end).is_ok());
}

#[test]
fn add_adjacent_start_invariant() {
    let start = Metrics {
        ts: 100,
        cycles: 100,
        insn_count: 100,
    };
    let start_off = Metrics {
        ts: 100,
        cycles: 110,
        insn_count: 110,
    };
    let end = Metrics {
        ts: 200,
        cycles: 200,
        insn_count: 200,
    };
    let mut outer = new_frame(MetricsRange::new(start, &end));
    let child = new_frame(MetricsRange::new(start_off, &(start_off + METRICS_ONE)));
    outer.add_child(child).unwrap();
    assert!(outer.check_invariant());
}

#[test]
fn add_adjacent_end_invariant() {
    let start = Metrics {
        ts: 100,
        cycles: 100,
        insn_count: 100,
    };
    let end = Metrics {
        ts: 200,
        cycles: 200,
        insn_count: 200,
    };
    let end_off = Metrics {
        ts: 200,
        cycles: 190,
        insn_count: 190,
    };
    let mut outer = new_frame(MetricsRange::new(start, &end));
    let child = new_frame(MetricsRange::new(end_off - METRICS_ONE, &end_off));
    outer.add_child(child).unwrap();
    assert!(outer.check_invariant());
}

#[test]
fn event_sorts() {
    let mut event = Event::new(10, "Test Event".to_string(), "Description".to_string());
    event.add_occurence(SAMPLE_RANGE.start);
    event.add_occurence(SAMPLE_RANGE.start - METRICS_ONE);
    event.add_occurence(SAMPLE_RANGE.start + METRICS_ONE);
    assert_eq!(event.occurences().len(), 3);
    assert_eq!(event.occurences()[0], SAMPLE_RANGE.start - METRICS_ONE);
    assert_eq!(event.occurences()[1], SAMPLE_RANGE.start);
    assert_eq!(event.occurences()[2], SAMPLE_RANGE.start + METRICS_ONE);
}

#[test]
fn find_event() {
    let frame = new_frame(SAMPLE_RANGE);
    let trace = Trace::new(
        vec![TEST_SYMBOL.clone()],
        frame,
        vec![
            Event::new(20, "Another Event".to_string(), "Description".to_string()),
            Event::new(10, "Test Event".to_string(), "Description".to_string()),
        ],
    );
    assert_eq!(trace.events.len(), 2);
    assert!(trace.get_event(10).is_some());
    assert!(trace.get_event(20).is_some());
    assert!(trace.get_event(30).is_none());
}

#[test]
fn serialize_round_trip_nogzip() {
    let trace = test_trace();
    let data = trace.bin_serialize(false).unwrap();
    let deserialized = Trace::bin_deserialize(&data, false).unwrap();

    assert_eq!(deserialized.root_frame().chunks().count(), 5);
    assert!(deserialized.root_frame().check_invariant());
}

#[test]
fn serialize_round_trip_gzip() {
    let trace = test_trace();
    let data = trace.bin_serialize(true).unwrap();
    let deserialized = Trace::bin_deserialize(&data, true).unwrap();

    assert_eq!(deserialized.root_frame().chunks().count(), 5);
    assert!(deserialized.root_frame().check_invariant());
}
