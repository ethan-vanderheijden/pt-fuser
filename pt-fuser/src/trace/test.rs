use std::sync::LazyLock;

use indexmap::IndexMap;

use crate::trace::frame::Annotation;

use super::*;

pub(crate) const SAMPLE_RANGE_END: Metrics = Metrics {
    ts: 200,
    cycles: 350,
    insn_count: 1000,
};
pub(crate) static SAMPLE_RANGE: LazyLock<MetricsRange> = LazyLock::new(|| {
    MetricsRange::new(
        Metrics {
            ts: 100,
            cycles: 50,
            insn_count: 200,
        },
        &SAMPLE_RANGE_END,
    )
});

pub(crate) const INNER_RANGE1_END: Metrics = Metrics {
    ts: 150,
    cycles: 150,
    insn_count: 500,
};
pub(crate) static INNER_RANGE1: LazyLock<MetricsRange> = LazyLock::new(|| {
    MetricsRange::new(
        Metrics {
            ts: 120,
            cycles: 70,
            insn_count: 300,
        },
        &INNER_RANGE1_END,
    )
});

pub(crate) const INNER_RANGE2_END: Metrics = Metrics {
    ts: 190,
    cycles: 300,
    insn_count: 900,
};
pub(crate) static INNER_RANGE2: LazyLock<MetricsRange> = LazyLock::new(|| {
    MetricsRange::new(
        Metrics {
            ts: 160,
            cycles: 200,
            insn_count: 600,
        },
        &INNER_RANGE2_END,
    )
});

pub(crate) static TEST_SYMBOL: LazyLock<Arc<SymbolInfo>> = LazyLock::new(|| {
    Arc::new(SymbolInfo {
        name: "test".to_string(),
        vaddr_start: 0x1000,
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
pub(crate) fn test_trace() -> Trace {
    let mut outer = Frame::new(SAMPLE_RANGE.clone(), 0, TEST_SYMBOL.clone());
    let middle = Frame::new(INNER_RANGE1.clone(), 0, TEST_SYMBOL.clone());
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

pub(crate) fn new_frame(range: &MetricsRange) -> Frame {
    Frame::new(range.clone(), 0, TEST_SYMBOL.clone())
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
    let frame = new_frame(&SAMPLE_RANGE);
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
fn serialize_round_trip_nocompress() {
    let trace = test_trace();
    let mut data_buf = Vec::new();
    trace.bin_serialize(&mut data_buf, false).unwrap();
    let deserialized = Trace::bin_deserialize(&mut data_buf.as_slice()).unwrap();

    assert_eq!(deserialized.root_frame().chunks().count(), 5);
    assert!(deserialized.root_frame().check_invariant());
}

#[test]
fn serialize_round_trip_compress() {
    let trace = test_trace();
    let mut data_buf = Vec::new();
    trace.bin_serialize(&mut data_buf, true).unwrap();
    let deserialized = Trace::bin_deserialize(&mut data_buf.as_slice()).unwrap();

    assert_eq!(deserialized.root_frame().chunks().count(), 5);
    assert!(deserialized.root_frame().check_invariant());
}

#[test]
fn serialize_round_trip_compress_annotations() {
    let mut trace = test_trace();
    let root = &mut trace.root;
    let mut inner_map = IndexMap::new();
    inner_map.insert("x".to_string(), Annotation::String("y".to_string()));
    let annotations = root.get_or_default_annotation();
    annotations.insert("a".to_string(), Annotation::Bool(true));
    annotations.insert(
        "z".to_string(),
        Annotation::Array(vec![Annotation::Int64(123), Annotation::Int64(456)]),
    );
    annotations.insert("b".to_string(), Annotation::Double(2.5));
    annotations.insert("y".to_string(), Annotation::Map(inner_map));

    let ground_truth = annotations.clone();

    let mut data_buf = Vec::new();
    trace.bin_serialize(&mut data_buf, true).unwrap();

    let deserialized = Trace::bin_deserialize(&mut data_buf.as_slice()).unwrap();
    let deserialized_root = &deserialized.root;
    let deserialized_annotations = deserialized_root.annotations.as_ref().unwrap();
    assert!(deserialized_annotations.keys().eq(ground_truth.keys()));
    assert!(deserialized_annotations.values().eq(ground_truth.values()));
}
