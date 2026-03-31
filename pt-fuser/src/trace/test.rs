use std::sync::LazyLock;

use super::*;

pub(crate) const SAMPLE_RANGE: MetricsRange = MetricsRange {
    start: Metrics {
        ts: 100,
        cycles: 50,
        insn_count: 200,
    },
    end: Metrics {
        ts: 200,
        cycles: 350,
        insn_count: 1000,
    },
};

pub(crate) const INNER_RANGE1: MetricsRange = MetricsRange {
    start: Metrics {
        ts: 120,
        cycles: 70,
        insn_count: 300,
    },
    end: Metrics {
        ts: 150,
        cycles: 150,
        insn_count: 500,
    },
};

pub(crate) const INNER_RANGE2: MetricsRange = MetricsRange {
    start: Metrics {
        ts: 160,
        cycles: 200,
        insn_count: 600,
    },
    end: Metrics {
        ts: 190,
        cycles: 300,
        insn_count: 900,
    },
};

pub(crate) const TEST_SYMBOL: LazyLock<SymbolInfo> = LazyLock::new(|| SymbolInfo {
    name: "test".to_string(),
    offset: 0x1000,
    size: 0x100,
});

pub(crate) const METRICS_ONE: Metrics = Metrics {
    ts: 1,
    cycles: 1,
    insn_count: 1,
};

/// Creates a trace with no events and a root frame that has five chunks:
/// child frame, straightline, child frame, straightline, child frame
fn test_trace() -> Trace {
    let mut outer = Frame::new(SAMPLE_RANGE, TEST_SYMBOL.clone());
    let middle = Frame::new(INNER_RANGE1, TEST_SYMBOL.clone());
    outer.add_child(middle).unwrap();
    let beginning = Frame::new(
        MetricsRange::new(SAMPLE_RANGE.start, INNER_RANGE1.start - METRICS_ONE),
        TEST_SYMBOL.clone(),
    );
    outer.add_child(beginning).unwrap();
    let end = Frame::new(
        MetricsRange::new(INNER_RANGE1.end + METRICS_ONE, SAMPLE_RANGE.end),
        TEST_SYMBOL.clone(),
    );
    outer.add_child(end).unwrap();

    Trace::new(outer, vec![])
}

#[test]
fn range_totals() {
    let frame = Chunk::Frame(Frame::new(SAMPLE_RANGE, TEST_SYMBOL.clone()));
    assert_eq!(SAMPLE_RANGE.total_time(), 100);
    assert_eq!(SAMPLE_RANGE.total_cycles(), 300);
    assert_eq!(SAMPLE_RANGE.total_insn(), 800);
    assert_eq!(frame.total_time(), SAMPLE_RANGE.total_time());
    assert_eq!(frame.total_cycles(), SAMPLE_RANGE.total_cycles());
    assert_eq!(frame.total_insn(), SAMPLE_RANGE.total_insn());
}

#[test]
fn zero_duration_frame() {
    let frame = MetricsRange::new(SAMPLE_RANGE.start, SAMPLE_RANGE.start);
    assert_eq!(frame.total_time(), 0);
    assert_eq!(frame.total_cycles(), 0);
    assert_eq!(frame.total_insn(), 0);
}

#[test]
fn empty_frame_invariant() {
    let frame = Frame::new(SAMPLE_RANGE, TEST_SYMBOL.clone());
    assert!(frame.check_invariant());
}

#[test]
fn fails_invariant() {
    let mut frame = Frame::new(SAMPLE_RANGE, TEST_SYMBOL.clone());
    match &mut frame.chunks[0] {
        Chunk::Frame(_) => unreachable!(),
        Chunk::Straightline(straightline) => {
            straightline.metrics.end -= METRICS_ONE;
        }
    }
    assert!(!frame.check_invariant());
}

#[test]
fn child_frame_invariant() {
    let mut frame = Frame::new(SAMPLE_RANGE, TEST_SYMBOL.clone());
    let child1 = Frame::new(INNER_RANGE1, TEST_SYMBOL.clone());
    let child2 = Frame::new(INNER_RANGE2, TEST_SYMBOL.clone());
    frame.add_child(child1).unwrap();
    frame.add_child(child2).unwrap();
    assert!(frame.check_invariant());
}

#[test]
fn child_overlaps_parent() {
    let mut outer = Frame::new(SAMPLE_RANGE, TEST_SYMBOL.clone());
    let inner = Frame::new(SAMPLE_RANGE, TEST_SYMBOL.clone());
    outer.add_child(inner).unwrap();
    assert_eq!(outer.chunks.len(), 1);
    assert!(outer.check_invariant());
}

#[test]
fn child_overlapping_complex() {
    let trace = test_trace();
    let outer = trace.root_frame();
    assert_eq!(outer.chunks.len(), 5);
    assert!(outer.check_invariant());
    assert!(matches!(&outer.chunks[0], Chunk::Frame(_)));
    assert!(matches!(&outer.chunks[1], Chunk::Straightline(_)));
    assert!(matches!(&outer.chunks[2], Chunk::Frame(_)));
    assert!(matches!(&outer.chunks[3], Chunk::Straightline(_)));
    assert!(matches!(&outer.chunks[4], Chunk::Frame(_)));
}

#[test]
fn add_invalid_child() {
    let mut frame = Frame::new(SAMPLE_RANGE, TEST_SYMBOL.clone());
    let too_early = Frame::new(
        MetricsRange::new(SAMPLE_RANGE.start - METRICS_ONE, INNER_RANGE1.end),
        TEST_SYMBOL.clone(),
    );
    let too_late = Frame::new(
        MetricsRange::new(INNER_RANGE2.start, SAMPLE_RANGE.end + METRICS_ONE),
        TEST_SYMBOL.clone(),
    );
    assert!(frame.add_child(too_early).is_err());
    assert!(frame.add_child(too_late).is_err());
}

#[test]
fn add_child_no_space() {
    let mut outer = Frame::new(SAMPLE_RANGE, TEST_SYMBOL.clone());
    let middle = Frame::new(INNER_RANGE1, TEST_SYMBOL.clone());
    outer.add_child(middle).unwrap();
    let beginning = Frame::new(
        MetricsRange::new(
            SAMPLE_RANGE.start + METRICS_ONE,
            INNER_RANGE1.start + METRICS_ONE,
        ),
        TEST_SYMBOL.clone(),
    );
    let end = Frame::new(
        MetricsRange::new(
            INNER_RANGE1.end - METRICS_ONE,
            SAMPLE_RANGE.end - METRICS_ONE,
        ),
        TEST_SYMBOL.clone(),
    );
    assert!(outer.add_child(beginning).is_err());
    assert!(outer.add_child(end).is_err());
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
    let frame = Frame::new(SAMPLE_RANGE, TEST_SYMBOL.clone());
    let trace = Trace::new(
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

    assert_eq!(deserialized.root_frame().chunks.len(), 5);
    assert!(deserialized.root_frame().check_invariant());
}

#[test]
fn serialize_round_trip_gzip() {
    let trace = test_trace();
    let data = trace.bin_serialize(true).unwrap();
    let deserialized = Trace::bin_deserialize(&data, true).unwrap();

    assert_eq!(deserialized.root_frame().chunks.len(), 5);
    assert!(deserialized.root_frame().check_invariant());
}
