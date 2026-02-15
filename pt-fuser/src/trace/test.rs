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

#[test]
fn range_totals() {
    let frame = Chunk::Frame(Frame::new(SAMPLE_RANGE, TEST_SYMBOL.clone()));
    assert_eq!(SAMPLE_RANGE.total_time(), 101);
    assert_eq!(SAMPLE_RANGE.total_cycles(), 301);
    assert_eq!(SAMPLE_RANGE.total_insn(), 801);
    assert_eq!(frame.total_time(), SAMPLE_RANGE.total_time());
    assert_eq!(frame.total_cycles(), SAMPLE_RANGE.total_cycles());
    assert_eq!(frame.total_insn(), SAMPLE_RANGE.total_insn());
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
    let mut outer = Frame::new(SAMPLE_RANGE, TEST_SYMBOL.clone());
    let middle = Frame::new(INNER_RANGE1, TEST_SYMBOL.clone());
    outer.add_child(middle).unwrap();
    let beginning = Frame::new(
        MetricsRange::from(
            SAMPLE_RANGE.start,
            INNER_RANGE1.start - METRICS_ONE - METRICS_ONE,
        ),
        TEST_SYMBOL.clone(),
    );
    outer.add_child(beginning).unwrap();
    let end = Frame::new(
        MetricsRange::from(
            INNER_RANGE1.end + METRICS_ONE + METRICS_ONE,
            SAMPLE_RANGE.end,
        ),
        TEST_SYMBOL.clone(),
    );
    outer.add_child(end).unwrap();
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
        MetricsRange::from(
            SAMPLE_RANGE.start - METRICS_ONE,
            INNER_RANGE1.end,
        ),
        TEST_SYMBOL.clone(),
    );
    let too_late = Frame::new(
        MetricsRange::from(
            INNER_RANGE2.start,
            SAMPLE_RANGE.end + METRICS_ONE,
        ),
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
        MetricsRange::from(
            SAMPLE_RANGE.start + METRICS_ONE,
            INNER_RANGE1.start,
        ),
        TEST_SYMBOL.clone(),
    );
    let end = Frame::new(
        MetricsRange::from(
            INNER_RANGE1.end,
            SAMPLE_RANGE.end - METRICS_ONE,
        ),
        TEST_SYMBOL.clone(),
    );
    assert!(outer.add_child(beginning).is_err());
    assert!(outer.add_child(end).is_err());
}

#[test]
fn sort_events() {
    let events = vec![
        Event {
            metrics: Metrics {
                ts: 150,
                cycles: 100,
                insn_count: 400,
            },
            description: "Event 1".to_string(),
        },
        Event {
            metrics: Metrics {
                ts: 120,
                cycles: 70,
                insn_count: 300,
            },
            description: "Event 2".to_string(),
        },
        Event {
            metrics: Metrics {
                ts: 160,
                cycles: 200,
                insn_count: 600,
            },
            description: "Event 3".to_string(),
        },
    ];
    let frame = Frame::new(SAMPLE_RANGE, TEST_SYMBOL.clone());
    let trace = Trace::new(frame, events);
    assert!(
        trace
            .get_events()
            .is_sorted_by(|e1, e2| e1.metrics.ts <= e2.metrics.ts)
    );
}
