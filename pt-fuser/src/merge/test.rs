use std::sync::{Arc, LazyLock};

use crate::{
    merge,
    trace::{
        Annotation, Chunk, Event, Frame, SymbolInfo, Trace,
        builder::SymbolCache,
        metrics::{Metrics, MetricsRange},
    },
};

const DUMMY_RANGE_END: Metrics = Metrics {
    ts: 200,
    cycles: 200,
    insn_count: 200,
};
const DUMMY_RANGE: MetricsRange = MetricsRange::new(
    Metrics {
        ts: 100,
        cycles: 100,
        insn_count: 100,
    },
    &DUMMY_RANGE_END,
);

const DUMMY_SYMBOL: LazyLock<Arc<SymbolInfo>> = LazyLock::new(|| {
    Arc::new(SymbolInfo {
        name: "dummy".to_string(),
        offset: 1,
        size: 1,
    })
});

const SYMBOLS: LazyLock<Vec<Arc<SymbolInfo>>> = LazyLock::new(|| vec![DUMMY_SYMBOL.clone()]);

fn new_frame(range: MetricsRange) -> Frame {
    Frame::new(range, 0, DUMMY_SYMBOL.clone())
}

fn new_trace(root: Frame) -> Trace {
    Trace::new(SYMBOLS.clone(), root, vec![])
}

const DUMMY_FRAME: LazyLock<Frame> = LazyLock::new(|| new_frame(DUMMY_RANGE));

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct TestLCS {
    id: u32,
}

impl merge::Id for TestLCS {
    fn id(&self) -> u32 {
        self.id
    }
}

impl merge::Id for &TestLCS {
    fn id(&self) -> u32 {
        self.id
    }
}

// special symbol called "[pause]" represents a pause chunk
fn produce_chunks_from_symbols(symbols: &[&str]) -> Frame {
    let mut frame = new_frame(DUMMY_RANGE);
    for (i, &symbol) in symbols.iter().enumerate() {
        let range = MetricsRange::new(
            Metrics::new(100 + i as u64, 100 + i as u64, 100 + i as u64),
            &Metrics::new(101 + i as u64, 101 + i as u64, 101 + i as u64),
        );
        if symbol == "[pause]" {
            frame
                .add_pause(range)
                .expect("Failed to add pause to frame");
        } else {
            frame
                .add_child(Frame::new(
                    range,
                    0,
                    Arc::new(SymbolInfo {
                        name: symbol.to_string(),
                        offset: 1,
                        size: 1,
                    }),
                ))
                .expect(&format!("Failed to add child '{}' to frame", symbol));
        }
    }
    frame
}

// special symbol called "[pause]" represents a pause chunk
fn produce_frames_from_metrics(root: (u64, u64), children: &[(u64, u64, Option<&str>)]) -> Frame {
    let mut frame = new_frame(MetricsRange::new(
        Metrics::constant(root.0),
        &Metrics::constant(root.1),
    ));
    for &(start, end, symbol) in children {
        let range = MetricsRange::new(Metrics::constant(start), &Metrics::constant(end));
        if symbol.is_some() && symbol.unwrap() == "[pause]" {
            frame
                .add_pause(range)
                .expect("Failed to add pause to frame");
        } else {
            let symbol = symbol
                .map(|s| SymbolInfo {
                    name: s.to_string(),
                    offset: 1,
                    size: 1,
                })
                .map(Arc::new)
                .unwrap_or(DUMMY_SYMBOL.clone());
            frame
                .add_child(Frame::new(range, 0, symbol))
                .expect(&format!(
                    "Failed to add child with range ({}, {}) to frame",
                    start, end
                ));
        }
    }
    frame
}

fn extract_ids(frames: &[impl merge::Id]) -> Vec<u32> {
    frames.iter().map(|f| f.id()).collect()
}

fn seq(xs: &[u32]) -> Vec<TestLCS> {
    xs.iter().map(|x| TestLCS { id: *x }).collect()
}

fn extract_pause_chunk<'a>(chunk: &'a Chunk) -> &'a MetricsRange {
    match chunk {
        Chunk::Pause(pause) => pause,
        _ => panic!("Expected pause chunk"),
    }
}

fn extract_frame_chunk<'a>(chunk: &'a Chunk) -> &'a Frame {
    match chunk {
        Chunk::Frame(frame) => frame,
        _ => panic!("Expected frame chunk"),
    }
}

#[test]
fn index_empty() {
    let (n, r) = merge::index_children(&[], None, &[]);
    assert_eq!(n, 0);
    assert_eq!(r.len(), 0);
}

#[test]
fn index_single() {
    let frame = produce_chunks_from_symbols(&["a", "b", "c"]);
    let (n, r) = merge::index_children(&[&frame], None, &[0]);
    assert_eq!(n, 3);
    assert_eq!(r.len(), 1);
    assert_eq!(extract_ids(&r[0]), vec![1, 2, 3]);
}

#[test]
fn index_3_no_repeat() {
    let frame1 = produce_chunks_from_symbols(&["a", "b", "c", "d"]);
    let frame2 = produce_chunks_from_symbols(&["b", "c", "e", "g", "h", "d"]);
    let frame3 = produce_chunks_from_symbols(&["f", "a", "d", "e"]);
    let (n, r) = merge::index_children(&[&frame1, &frame2, &frame3], None, &[0, 1, 2]);
    assert_eq!(n, 8);
    assert_eq!(r.len(), 3);
    assert_eq!(extract_ids(&r[0]), vec![1, 2, 3, 4]);
    assert_eq!(extract_ids(&r[1]), vec![2, 3, 5, 6, 7, 4]);
    assert_eq!(extract_ids(&r[2]), vec![8, 1, 4, 5]);
}

#[test]
fn index_3_repeating() {
    let frame1 = produce_chunks_from_symbols(&["a", "b", "a", "c", "d", "c"]);
    let frame2 = produce_chunks_from_symbols(&["b", "c", "a", "a", "e", "g", "e", "h"]);
    let frame3 = produce_chunks_from_symbols(&["c", "a", "c", "f", "h", "a", "d", "e"]);
    let (n, r) = merge::index_children(&[&frame1, &frame2, &frame3], None, &[0, 1, 2]);
    assert_eq!(n, 11);
    assert_eq!(r.len(), 3);
    assert_eq!(extract_ids(&r[0]), vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(extract_ids(&r[1]), vec![2, 4, 1, 3, 7, 8, 9, 10]);
    assert_eq!(extract_ids(&r[2]), vec![4, 1, 6, 11, 10, 3, 5, 7]);
}

#[test]
fn index_3_with_pauses() {
    let frame1 = produce_chunks_from_symbols(&["a", "b", "[pause]", "a", "[pause]", "c", "d", "c"]);
    let frame2 = produce_chunks_from_symbols(&["b", "c", "a", "a", "e", "g", "e", "[pause]", "h"]);
    let frame3 = produce_chunks_from_symbols(&[
        "[pause]", "[pause]", "c", "a", "c", "f", "[pause]", "h", "a", "d", "e",
    ]);
    let (n, r) = merge::index_children(&[&frame1, &frame2, &frame3], None, &[0, 1, 2]);
    assert_eq!(n, 14);
    assert_eq!(r.len(), 3);
    assert_eq!(extract_ids(&r[0]), vec![1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(extract_ids(&r[1]), vec![2, 6, 1, 4, 9, 10, 11, 3, 12]);
    assert_eq!(extract_ids(&r[2]), vec![3, 5, 6, 1, 8, 13, 14, 12, 4, 7, 9]);
}

#[test]
#[should_panic]
fn lcs_empty() {
    merge::find_lcs::<TestLCS>(0, &[]);
}

#[test]
fn lcs_single() {
    let sequence = seq(&[1, 2, 3, 4, 5]);
    assert_eq!(merge::find_lcs(5, &[&sequence]), &[1, 2, 3, 4, 5]);
}

#[test]
fn lcs_2_identical() {
    let seq1 = seq(&[1, 2, 3, 4, 5]);
    let seq2 = seq(&[1, 2, 3, 4, 5]);
    assert_eq!(merge::find_lcs(5, &[&seq1, &seq2]), &[1, 2, 3, 4, 5]);
}

#[test]
fn lcs_2_different() {
    let sequence1 = seq(&[1, 2, 3, 4, 5]);
    let sequence2 = seq(&[1, 2, 4, 7, 5, 6]);
    assert_eq!(merge::find_lcs(7, &[&sequence1, &sequence2]), &[1, 2, 4, 5]);
}

#[test]
fn lcs_3_different() {
    let seq1 = seq(&[1, 2, 3, 4, 5, 6, 7]);
    let seq2 = seq(&[2, 1, 3, 4, 9, 12, 5, 7]);
    let seq3 = seq(&[12, 2, 3, 6, 5, 11, 7, 1]);
    let answer = &[2, 3, 5, 7];
    let seqs1 = vec![seq1.as_slice(), &seq2, &seq3];
    let seqs2 = vec![seq3.as_slice(), &seq2, &seq1];
    let seqs3 = vec![seq2.as_slice(), &seq3, &seq1];
    assert_eq!(merge::find_lcs(12, &seqs1), answer);
    assert_eq!(merge::find_lcs(12, &seqs2), answer);
    assert_eq!(merge::find_lcs(12, &seqs3), answer);
}

#[test]
fn lcs_4_different() {
    let seq1 = seq(&[1, 3, 5, 7, 9, 11]);
    let seq2 = seq(&[2, 1, 3, 4, 5, 6, 7, 8, 9, 11, 10]);
    let seq3 = seq(&[8, 5, 1, 6, 3, 7, 9, 11]);
    let seq4 = seq(&[1, 2, 4, 6, 3, 5, 7, 8, 10, 11]);
    let answer = &[1, 3, 7, 11];
    let seqs1 = vec![seq1.as_slice(), &seq2, &seq3, &seq4];
    let seqs2 = vec![seq3.as_slice(), &seq2, &seq1, &seq4];
    let seqs3 = vec![seq4.as_slice(), &seq3, &seq2, &seq1];
    assert_eq!(merge::find_lcs(11, &seqs1), answer);
    assert_eq!(merge::find_lcs(11, &seqs2), answer);
    assert_eq!(merge::find_lcs(11, &seqs3), answer);
}

#[test]
fn lcs_5_identical() {
    let sequence = seq(&[1, 2, 3, 4, 5]);
    let seqs = vec![sequence.as_slice(); 5];
    assert_eq!(merge::find_lcs(5, &seqs), &[1, 2, 3, 4, 5]);
}

#[test]
#[should_panic]
fn common_thresh_0() {
    let seq = seq(&[]);
    merge::find_frequent_children(1, &[&seq], &mut |_| {}, -0.1);
}

#[test]
#[should_panic]
fn common_thresh_1() {
    let seq = seq(&[]);
    merge::find_frequent_children(1, &[&seq], &mut |_| {}, 1.1);
}

#[test]
fn common_simple_none() {
    let seq1 = seq(&[1]);
    let seq2 = seq(&[2]);
    let seq3 = seq(&[]);
    let seq4 = seq(&[1]);
    let seqs1 = vec![seq1.as_slice(), &seq2, &seq3, &seq4];
    let seqs2 = vec![seq2.as_slice(), &seq1, &seq4, &seq3];
    let mut result1 = Vec::new();
    merge::find_frequent_children(2, &seqs1, &mut |x| result1.push(extract_ids(x)), 0.7);
    let mut result2 = Vec::new();
    merge::find_frequent_children(2, &seqs2, &mut |x| result2.push(extract_ids(x)), 0.7);
    assert_eq!(result1.len(), 0);
    assert_eq!(result2.len(), 0);
}

#[test]
fn common_simple_one() {
    let seq1 = seq(&[1]);
    let seq2 = seq(&[1]);
    let seq3 = seq(&[]);
    let seq4 = seq(&[1, 1, 1]);
    let result = &[vec![1, 1, 1]];
    let seqs1 = vec![seq1.as_slice(), &seq2, &seq3, &seq4];
    let seqs2 = vec![seq2.as_slice(), &seq1, &seq4, &seq3];
    let mut result1 = Vec::new();
    merge::find_frequent_children(1, &seqs1, &mut |x| result1.push(extract_ids(x)), 0.7);
    let mut result2 = Vec::new();
    merge::find_frequent_children(1, &seqs2, &mut |x| result2.push(extract_ids(x)), 0.7);
    assert_eq!(result1, result);
    assert_eq!(result2, result);
}

#[test]
fn common_three1() {
    let seq1 = seq(&[4, 5, 1, 2, 6, 3]);
    let seq2 = seq(&[7, 1, 8, 9, 2, 10, 3]);
    let seq3 = seq(&[6, 1, 10, 5, 3, 11]);
    let seq4 = seq(&[11, 2, 12, 4, 3]);
    let answer = &[vec![1, 1, 1], vec![2, 2, 2], vec![3, 3, 3, 3]];
    let seqs1 = vec![seq1.as_slice(), &seq2, &seq3, &seq4];
    let seqs2 = vec![seq2.as_slice(), &seq1, &seq4, &seq3];
    let mut result1 = Vec::new();
    let mut result2 = Vec::new();
    merge::find_frequent_children(12, &seqs1, &mut |x| result1.push(extract_ids(x)), 0.7);
    merge::find_frequent_children(12, &seqs2, &mut |x| result2.push(extract_ids(x)), 0.7);
    assert_eq!(result1, answer);
    assert_eq!(result2, answer);
}

#[test]
fn common_three2() {
    let seq1 = seq(&[1, 2, 3, 4, 5, 6, 7]);
    let seq2 = seq(&[8, 2, 10, 4, 12, 13, 14]);
    let seq3 = seq(&[15, 16, 17, 18, 4, 7, 21]);
    let seq4 = seq(&[22, 23, 24, 2, 4, 7, 28]);
    let answer = &[vec![2, 2, 2], vec![4, 4, 4, 4], vec![7, 7, 7]];
    let seqs1 = vec![seq1.as_slice(), &seq2, &seq3, &seq4];
    let seqs2 = vec![seq2.as_slice(), &seq1, &seq4, &seq3];
    let mut result1 = Vec::new();
    merge::find_frequent_children(28, &seqs1, &mut |x| result1.push(extract_ids(x)), 0.7);
    let mut result2 = Vec::new();
    merge::find_frequent_children(28, &seqs2, &mut |x| result2.push(extract_ids(x)), 0.7);
    assert_eq!(result1, answer);
    assert_eq!(result2, answer);
}

#[test]
fn common_slicing_heuristic() {
    let seq1 = seq(&[1, 2, 3, 4, 5, 6]);
    let seq2 = seq(&[1, 8, 9, 4, 11, 7]);
    let seq3 = seq(&[12, 13, 14, 15, 5, 1]);
    let seq4 = seq(&[18, 19, 20, 4, 5, 22]);
    // [1, 4] and [4, 5] are both decent answers, but heuristics will
    // cust seq3 into [12, 13, 14] and [15, 5, 1], so [4, 5] will be chosen
    let answer = &[vec![4, 4, 4], vec![5, 5, 5]];
    let seqs1 = vec![seq1.as_slice(), &seq2, &seq3, &seq4];
    let seqs2 = vec![seq2.as_slice(), &seq1, &seq4, &seq3];
    let mut result1 = Vec::new();
    merge::find_frequent_children(22, &seqs1, &mut |x| result1.push(extract_ids(x)), 0.7);
    let mut result2 = Vec::new();
    merge::find_frequent_children(22, &seqs2, &mut |x| result2.push(extract_ids(x)), 0.7);
    assert_eq!(result1, answer);
    assert_eq!(result2, answer);
}

#[test]
fn merge_traces_no_children() {
    let frame1 = produce_frames_from_metrics((500, 590), &[]);
    let trace1 = new_trace(frame1);
    let frame2 = produce_frames_from_metrics((300, 380), &[]);
    let trace2 = new_trace(frame2);
    let frame3 = produce_frames_from_metrics((400, 464), &[]);
    let trace3 = new_trace(frame3);
    let merged = merge::merge_traces(&[&trace1, &trace2, &trace3], None, false);
    assert_eq!(merged.root_frame().metrics.start, Metrics::constant(0));
    assert_eq!(
        merged.root_frame().metrics.end(),
        Metrics::constant((90 + 80 + 64) / 3)
    );
}

#[test]
fn merge_traces_common_children() {
    let frame1 = produce_frames_from_metrics((500, 590), &[(520, 540, None), (550, 558, None)]);
    let trace1 = new_trace(frame1);
    let frame2 = produce_frames_from_metrics((300, 380), &[(310, 335, None), (340, 352, None)]);
    let trace2 = new_trace(frame2);
    let frame3 = produce_frames_from_metrics((400, 464), &[(415, 430, None), (445, 458, None)]);
    let trace3 = new_trace(frame3);
    let merged = merge::merge_traces(&[&trace1, &trace2, &trace3], None, false);
    assert_eq!(merged.root_frame().metrics.start, Metrics::constant(0));
    assert_eq!(
        merged.root_frame().metrics.end(),
        Metrics::constant((90 + 80 + 64) / 3)
    );

    let chunks = merged.root_frame().chunks().collect::<Vec<_>>();
    assert_eq!(chunks.len(), 5);
    let child_frame1 = extract_frame_chunk(&chunks[1]);
    let child_frame2 = extract_frame_chunk(&chunks[3]);
    assert_eq!(child_frame1.metrics.start, Metrics::constant(15));
    assert_eq!(child_frame1.metrics.end(), Metrics::constant(15 + 20));
    assert_eq!(child_frame2.metrics.start, Metrics::constant(45));
    assert_eq!(child_frame2.metrics.end(), Metrics::constant(45 + 11));
}

#[test]
fn merge_frame_frequent_children() {
    let frame1 = produce_frames_from_metrics(
        (500, 590),
        &[
            (540, 541, Some("common")),
            (510, 518, Some("a")),
            (560, 570, Some("c")),
        ],
    );
    let frame2 = produce_frames_from_metrics(
        (300, 380),
        &[
            (340, 341, Some("common")),
            (314, 324, Some("a")),
            (354, 364, Some("b")),
        ],
    );
    let frame3 = produce_frames_from_metrics(
        (400, 464),
        &[
            (440, 441, Some("common")),
            (450, 456, Some("b")),
            (400, 410, Some("c")),
        ],
    );
    let mut symbol_cache = SymbolCache::new(10);
    let mut merged = new_frame(MetricsRange::new(
        Metrics::constant(50),
        &Metrics::constant(50 + (90 + 80 + 64) / 3),
    ));
    merge::merge_children(
        &mut merged,
        &[&frame1, &frame2, &frame3],
        None,
        &[0, 1, 2],
        None,
        &mut symbol_cache,
        &mut Vec::new(),
        0.6,
    );

    let chunks = merged.chunks().collect::<Vec<_>>();
    assert_eq!(chunks.len(), 7);
    let child_frame1 = extract_frame_chunk(&chunks[1]);
    let child_frame2 = extract_frame_chunk(&chunks[3]);
    let child_frame3 = extract_frame_chunk(&chunks[5]);
    assert_eq!(child_frame1.metrics.start, Metrics::constant(50 + 12));
    assert_eq!(child_frame1.metrics.end(), Metrics::constant(50 + 12 + 9));
    assert_eq!(child_frame1.symbol.name, "a");
    assert_eq!(child_frame2.metrics.start, Metrics::constant(50 + 40));
    assert_eq!(child_frame2.metrics.end(), Metrics::constant(50 + 40 + 1));
    assert_eq!(child_frame2.symbol.name, "common");
    assert_eq!(child_frame3.metrics.start, Metrics::constant(50 + 52));
    assert_eq!(child_frame3.metrics.end(), Metrics::constant(50 + 52 + 8));
    assert_eq!(child_frame3.symbol.name, "b");
}

#[test]
fn merge_frame_with_pauses() {
    let frame1 = produce_frames_from_metrics(
        (500, 600),
        &[
            (510, 520, Some("a")),
            (530, 540, Some("[pause]")),
            (550, 560, Some("[pause]")),
            (580, 590, Some("b")),
        ],
    );
    let frame2 = produce_frames_from_metrics(
        (300, 400),
        &[
            (310, 320, Some("b")),
            (330, 340, Some("[pause]")),
            (350, 360, Some("[pause]")),
            (380, 390, Some("c")),
        ],
    );
    let frame3 = produce_frames_from_metrics(
        (100, 300),
        &[
            (130, 150, Some("a")),
            (160, 180, Some("[pause]")),
            (250, 260, Some("c")),
        ],
    );
    let mut symbol_cache = SymbolCache::new(10);
    let mut merged = new_frame(MetricsRange::new(
        Metrics::constant(0),
        &Metrics::constant(133),
    ));
    merge::merge_children(
        &mut merged,
        &[&frame1, &frame2, &frame3],
        None,
        &[0, 1, 2],
        None,
        &mut symbol_cache,
        &mut Vec::new(),
        0.6,
    );
    // result: "a" "[pause]" "[pause]" "c"
    // where the two pause chunks are contiguous

    let chunks = merged.chunks().collect::<Vec<_>>();
    assert_eq!(chunks.len(), 8);
    let a = extract_frame_chunk(&chunks[1]);
    let pause1 = extract_pause_chunk(&chunks[3]);
    let pause2 = extract_pause_chunk(&chunks[4]);
    let c = extract_frame_chunk(&chunks[6]);
    assert_eq!(a.metrics.start, Metrics::constant(20));
    assert_eq!(a.metrics.end(), Metrics::constant(20 + 15));
    assert_eq!(a.symbol.name, "a");
    assert_eq!(pause1.start, Metrics::constant(40));
    assert_eq!(pause1.end(), Metrics::constant(40 + 13));
    assert_eq!(pause2.start, Metrics::constant(53));
    assert_eq!(pause2.end(), Metrics::constant(53 + 10 - 3));
    assert_eq!(c.metrics.start, Metrics::constant(115));
    assert_eq!(c.metrics.end(), Metrics::constant(115 + 10));
    assert_eq!(c.symbol.name, "c");
}

#[test]
fn merge_frame_with_anotations() {
    let root_frames = vec![
        produce_frames_from_metrics((0, 100), &[(50, 70, Some("a"))]),
        produce_frames_from_metrics((0, 98), &[(50, 69, Some("a"))]),
        produce_frames_from_metrics((0, 96), &[(50, 68, Some("a"))]),
        produce_frames_from_metrics((0, 94), &[(50, 67, Some("a"))]),
        produce_frames_from_metrics((0, 92), &[(50, 66, Some("a"))]),
        produce_frames_from_metrics((0, 90), &[(50, 65, Some("a"))]),
        produce_frames_from_metrics((0, 88), &[(50, 64, Some("a"))]),
        produce_frames_from_metrics((0, 86), &[(50, 63, Some("a"))]),
        produce_frames_from_metrics((0, 84), &[(50, 62, Some("a"))]),
        produce_frames_from_metrics((0, 82), &[(50, 61, Some("a"))]),
        produce_frames_from_metrics((0, 80), &[]),
        produce_frames_from_metrics((0, 78), &[]),
    ];
    let traces = root_frames
        .iter()
        .map(|frame| new_trace(frame.clone()))
        .collect::<Vec<_>>();
    let merged = merge::merge_traces(&traces.iter().collect::<Vec<_>>(), None, true);
    assert_eq!(merged.root_frame().chunks().count(), 3);

    let root_anotations = merged.root_frame().annotations.as_ref().unwrap();
    let root_stats = match &root_anotations[super::ANNOTATION_STATS_NAME] {
        Annotation::Map(stats) => stats,
        _ => panic!("Expected stats annotation to be a map"),
    };

    let child = merged.root_frame().chunks().nth(1).unwrap();
    let child_anotations = extract_frame_chunk(&child).annotations.as_ref().unwrap();
    let child_stats = match &child_anotations[super::ANNOTATION_STATS_NAME] {
        Annotation::Map(stats) => stats,
        _ => panic!("Expected stats annotation to be a map"),
    };

    assert_eq!(
        root_anotations[super::ANNOTATION_COUNT_NAME],
        Annotation::Uint64(12)
    );
    assert_eq!(root_stats["Min"], Annotation::Double(78.0));
    assert_eq!(root_stats["Q1"], Annotation::Double(83.0));
    assert_eq!(root_stats["Median"], Annotation::Double(89.0));
    assert_eq!(root_stats["Q3"], Annotation::Double(95.0));
    assert_eq!(root_stats["Max"], Annotation::Double(100.0));
    assert_eq!(root_stats["Mean"], Annotation::Double(89.0));
    match &root_stats["Std Dev"] {
        Annotation::Double(std_dev) => {
            assert_eq!(
                (std_dev * 100.0).round(),
                721.0,
                "Expected std dev to be approximately 7.21"
            );
        }
        _ => panic!("Expected std dev annotation to be a double"),
    }
    assert_eq!(
        root_stats[super::ANNOTATION_NOISE_CONTRIBUTION_NAME],
        Annotation::Double(1.0)
    );

    assert_eq!(
        child_anotations[super::ANNOTATION_COUNT_NAME],
        Annotation::Uint64(10)
    );
    assert_eq!(child_stats["Min"], Annotation::Double(11.0));
    assert_eq!(child_stats["Q1"], Annotation::Double(13.0));
    assert_eq!(child_stats["Median"], Annotation::Double(15.5));
    assert_eq!(child_stats["Q3"], Annotation::Double(18.0));
    assert_eq!(child_stats["Max"], Annotation::Double(20.0));
    assert_eq!(child_stats["Mean"], Annotation::Double(15.5));
    match &child_stats["Std Dev"] {
        Annotation::Double(std_dev) => {
            assert_eq!(
                (std_dev * 100.0).round(),
                303.0,
                "Expected std dev to be approximately 3.03"
            );
        }
        _ => panic!("Expected std dev annotation to be a double"),
    }
    match &child_stats[super::ANNOTATION_NOISE_CONTRIBUTION_NAME] {
        Annotation::Double(noise_contribution) => {
            assert!(
                (noise_contribution - 0.5717235169650965).abs() < 1e-12,
                "Expected NC to include zero latency for the two traces without the child"
            );
        }
        _ => panic!("Expected noise contribution annotation to be a double"),
    }
}

#[test]
fn merge_frame_omits_noise_contribution_when_not_requested() {
    let root_frames = (0..10)
        .map(|i| produce_frames_from_metrics((0, 100 + i), &[(10, 20 + i, Some("a"))]))
        .collect::<Vec<_>>();
    let traces = root_frames
        .iter()
        .map(|frame| new_trace(frame.clone()))
        .collect::<Vec<_>>();

    let merged = merge::merge_traces(&traces.iter().collect::<Vec<_>>(), None, false);
    let root_annotations = merged.root_frame().annotations.as_ref().unwrap();
    let root_stats = match &root_annotations[super::ANNOTATION_STATS_NAME] {
        Annotation::Map(stats) => stats,
        _ => panic!("Expected root stats annotation to be a map"),
    };
    assert!(!root_stats.contains_key(super::ANNOTATION_NOISE_CONTRIBUTION_NAME));

    let child = merged.root_frame().chunks().nth(1).unwrap();
    let child_annotations = extract_frame_chunk(&child).annotations.as_ref().unwrap();
    let child_stats = match &child_annotations[super::ANNOTATION_STATS_NAME] {
        Annotation::Map(stats) => stats,
        _ => panic!("Expected child stats annotation to be a map"),
    };
    assert!(!child_stats.contains_key(super::ANNOTATION_NOISE_CONTRIBUTION_NAME));
}

#[test]
fn merge_frame_omits_noise_contribution_when_e2e_stddev_is_zero() {
    let root_frames = (0..10)
        .map(|i| produce_frames_from_metrics((0, 100), &[(10, 20 + i, Some("a"))]))
        .collect::<Vec<_>>();
    let traces = root_frames
        .iter()
        .map(|frame| new_trace(frame.clone()))
        .collect::<Vec<_>>();

    let merged = merge::merge_traces(&traces.iter().collect::<Vec<_>>(), None, true);
    let root_annotations = merged.root_frame().annotations.as_ref().unwrap();
    let root_stats = match &root_annotations[super::ANNOTATION_STATS_NAME] {
        Annotation::Map(stats) => stats,
        _ => panic!("Expected root stats annotation to be a map"),
    };
    assert_eq!(root_stats["Std Dev"], Annotation::Double(0.0));
    assert!(!root_stats.contains_key(super::ANNOTATION_NOISE_CONTRIBUTION_NAME));

    let child = merged.root_frame().chunks().nth(1).unwrap();
    let child_annotations = extract_frame_chunk(&child).annotations.as_ref().unwrap();
    let child_stats = match &child_annotations[super::ANNOTATION_STATS_NAME] {
        Annotation::Map(stats) => stats,
        _ => panic!("Expected child stats annotation to be a map"),
    };
    assert!(!child_stats.contains_key(super::ANNOTATION_NOISE_CONTRIBUTION_NAME));
}

#[test]
fn merge_frame_adds_noise_contribution_to_nested_frame_with_missing_traces() {
    let root_frames = (0..10)
        .map(|i| {
            let mut outer = if i < 7 {
                produce_frames_from_metrics((10, 80), &[(20, 30 + i, Some("a"))])
            } else {
                produce_frames_from_metrics((10, 80), &[])
            };
            outer.symbol = Arc::new(SymbolInfo {
                name: "outer".to_string(),
                offset: outer.symbol.offset,
                size: outer.symbol.size,
            });

            let mut root = new_frame(MetricsRange::new(
                Metrics::constant(0),
                &Metrics::constant(100 + 2 * i),
            ));
            root.add_child(outer).unwrap();
            root
        })
        .collect::<Vec<_>>();
    let traces = root_frames
        .iter()
        .map(|frame| new_trace(frame.clone()))
        .collect::<Vec<_>>();

    let merged = merge::merge_traces(&traces.iter().collect::<Vec<_>>(), None, true);
    let outer = merged.root_frame().chunks().nth(1).unwrap();
    let outer = extract_frame_chunk(&outer);
    let nested = outer.chunks().nth(1).unwrap();
    let nested = extract_frame_chunk(&nested);
    let nested_annotations = nested.annotations.as_ref().unwrap();
    assert_eq!(
        nested_annotations[super::ANNOTATION_COUNT_NAME],
        Annotation::Uint64(7)
    );
    let nested_stats = match &nested_annotations[super::ANNOTATION_STATS_NAME] {
        Annotation::Map(stats) => stats,
        _ => panic!("Expected nested stats annotation to be a map"),
    };
    match &nested_stats[super::ANNOTATION_NOISE_CONTRIBUTION_NAME] {
        Annotation::Double(noise_contribution) => assert!(noise_contribution.is_finite()),
        _ => panic!("Expected nested noise contribution annotation to be a double"),
    }
}

#[test]
fn merge_traces_export_raw() {
    let frame1 = produce_frames_from_metrics((500, 590), &[(520, 540, Some("a"))]);
    let mut root1 = new_frame(MetricsRange::new(
        Metrics::constant(500),
        &Metrics::constant(600),
    ));
    root1.add_child(frame1).unwrap();
    let trace1 = new_trace(root1);

    let frame2 = produce_frames_from_metrics((300, 380), &[(310, 335, Some("a"))]);
    let mut root2 = new_frame(MetricsRange::new(
        Metrics::constant(300),
        &Metrics::constant(400),
    ));
    root2.add_child(frame2).unwrap();
    let trace2 = new_trace(root2);

    let frame3 = produce_frames_from_metrics((400, 464), &[]);
    let mut root3 = new_frame(MetricsRange::new(
        Metrics::constant(400),
        &Metrics::constant(500),
    ));
    root3.add_child(frame3).unwrap();
    let trace3 = new_trace(root3);

    let frame4 = produce_frames_from_metrics((0, 80), &[(10, 60, Some("a"))]);
    let mut root4 = new_frame(MetricsRange::new(
        Metrics::constant(0),
        &Metrics::constant(100),
    ));
    root4.add_child(frame4).unwrap();
    let trace4 = new_trace(root4);

    let merged = merge::merge_traces(
        &[&trace1, &trace2, &trace3, &trace4],
        Some(&["t1", "t2", "t3", "t4"]),
        false,
    );
    let root_frame = merged.root_frame();
    let root_raw_data = root_frame
        .annotations
        .as_ref()
        .unwrap()
        .get(merge::ANNOTATION_RAW_DATA_NAME)
        .expect("Root frame should have raw data annotation");
    match root_raw_data {
        Annotation::Map(map) => {
            assert_eq!(map.get("t1"), Some(&Annotation::Uint64(100)));
            assert_eq!(map.get("t2"), Some(&Annotation::Uint64(100)));
            assert_eq!(map.get("t3"), Some(&Annotation::Uint64(100)));
            assert_eq!(map.get("t4"), Some(&Annotation::Uint64(100)));
        }
        _ => panic!("Expected root raw data annotation to be a map"),
    }

    let child = root_frame.chunks().next().unwrap();
    let child = extract_frame_chunk(&child);
    let child_raw_data = child
        .annotations
        .as_ref()
        .unwrap()
        .get(merge::ANNOTATION_RAW_DATA_NAME)
        .expect("Child frame should have raw data annotation");
    match child_raw_data {
        Annotation::Map(map) => {
            assert_eq!(map.get("t1"), Some(&Annotation::Uint64(90)));
            assert_eq!(map.get("t2"), Some(&Annotation::Uint64(80)));
            assert_eq!(map.get("t3"), Some(&Annotation::Uint64(64)));
            assert_eq!(map.get("t4"), Some(&Annotation::Uint64(80)));
        }
        _ => panic!("Expected child raw data annotation to be a map"),
    }

    let grandchild = &child.chunks().nth(1).unwrap();
    let grandchild = extract_frame_chunk(&grandchild);
    let grandchild_raw_data = grandchild
        .annotations
        .as_ref()
        .unwrap()
        .get(merge::ANNOTATION_RAW_DATA_NAME)
        .expect("Grandchild frame should have raw data annotation");
    match grandchild_raw_data {
        Annotation::Map(map) => {
            assert_eq!(map.get("t1"), Some(&Annotation::Uint64(20)));
            assert_eq!(map.get("t2"), Some(&Annotation::Uint64(25)));
            assert!(!map.contains_key("t3"));
            assert_eq!(map.get("t4"), Some(&Annotation::Uint64(50)));
        }
        _ => panic!("Expected grandchild raw data annotation to be a map"),
    }
}

#[test]
fn merge_events_simple() {
    let mut event_a1 = Event::new(1, "A".to_string(), "Desc".to_string());
    let mut event_a2 = event_a1.clone();
    event_a1.add_occurence(Metrics::constant(125));
    event_a2.add_occurence(Metrics::constant(150));

    let mut event_b1 = Event::new(2, "B".to_string(), "Desc".to_string());
    let mut event_b2 = event_b1.clone();
    event_b1.add_occurence(Metrics::constant(130));
    event_b2.add_occurence(Metrics::constant(170));

    let mut event_c1 = Event::new(3, "C".to_string(), "Desc".to_string());
    event_c1.add_occurence(Metrics::constant(140));

    let trace1 = Trace::new(SYMBOLS.clone(), DUMMY_FRAME.clone(), vec![event_a1]);
    let trace2 = Trace::new(
        SYMBOLS.clone(),
        DUMMY_FRAME.clone(),
        vec![event_a2, event_b1],
    );
    let trace3 = Trace::new(
        SYMBOLS.clone(),
        DUMMY_FRAME.clone(),
        vec![event_b2, event_c1],
    );
    let merged_events = merge::merge_events(&[&trace1, &trace2, &trace3], &DUMMY_RANGE);
    assert_eq!(merged_events.len(), 3);

    let merged_event_a = merged_events
        .iter()
        .find(|e| e.id == 1)
        .expect("Merged event A not found");
    let merged_event_b = merged_events
        .iter()
        .find(|e| e.id == 2)
        .expect("Merged event B not found");
    let merged_event_c = merged_events
        .iter()
        .find(|e| e.id == 3)
        .expect("Merged event C not found");
    assert_eq!(
        merged_event_a.occurences(),
        &[Metrics::constant(125), Metrics::constant(150)]
    );
    assert_eq!(
        merged_event_b.occurences(),
        &[Metrics::constant(130), Metrics::constant(170)]
    );
    assert_eq!(merged_event_c.occurences(), &[Metrics::constant(140)]);
}

#[test]
fn merge_events_scaling() {
    let mut event_a = Event::new(1, "A".to_string(), "Desc".to_string());
    event_a.add_occurence(Metrics::constant(300));
    event_a.add_occurence(Metrics::constant(325));

    let mut event_b = Event::new(2, "B".to_string(), "Desc".to_string());
    event_b.add_occurence(Metrics::constant(375));
    event_b.add_occurence(Metrics::constant(405));

    let trace1 = Trace::new(
        SYMBOLS.clone(),
        new_frame(MetricsRange::new(
            Metrics::constant(250),
            &Metrics::constant(500),
        )),
        vec![event_a],
    );
    let trace2 = Trace::new(
        SYMBOLS.clone(),
        new_frame(MetricsRange::new(
            Metrics::constant(325),
            &Metrics::constant(425),
        )),
        vec![event_b],
    );
    let merged_events = merge::merge_events(
        &[&trace1, &trace2],
        &MetricsRange::new(Metrics::constant(20), &Metrics::constant(100)),
    );

    assert_eq!(merged_events.len(), 2);

    let merged_event_a = merged_events
        .iter()
        .find(|e| e.id == 1)
        .expect("Merged event A not found");
    let merged_event_b = merged_events
        .iter()
        .find(|e| e.id == 2)
        .expect("Merged event B not found");
    assert_eq!(
        merged_event_a.occurences(),
        &[Metrics::constant(36), Metrics::constant(44)]
    );
    assert_eq!(
        merged_event_b.occurences(),
        &[Metrics::constant(60), Metrics::constant(84)]
    );
}

#[test]
fn merge_events_zipped_scaled() {
    let mut event_a1 = Event::new(1, "A".to_string(), "Desc".to_string());
    event_a1.add_occurence(Metrics::constant(300));
    event_a1.add_occurence(Metrics::constant(400));
    let mut event_a2 = Event::new(1, "A".to_string(), "Desc".to_string());
    event_a2.add_occurence(Metrics::constant(365));
    event_a2.add_occurence(Metrics::constant(405));

    let trace1 = Trace::new(
        SYMBOLS.clone(),
        new_frame(MetricsRange::new(
            Metrics::constant(250),
            &Metrics::constant(500),
        )),
        vec![event_a1],
    );
    let trace2 = Trace::new(
        SYMBOLS.clone(),
        new_frame(MetricsRange::new(
            Metrics::constant(325),
            &Metrics::constant(425),
        )),
        vec![event_a2],
    );
    let merged_events = merge::merge_events(
        &[&trace1, &trace2],
        &MetricsRange::new(Metrics::constant(20), &Metrics::constant(100)),
    );
    assert_eq!(merged_events.len(), 1);

    let merged_event_a = merged_events
        .iter()
        .find(|e| e.id == 1)
        .expect("Merged event A not found");
    assert_eq!(
        merged_event_a.occurences(),
        &[
            Metrics::constant(36),
            Metrics::constant(52),
            Metrics::constant(68),
            Metrics::constant(84)
        ]
    );
}
