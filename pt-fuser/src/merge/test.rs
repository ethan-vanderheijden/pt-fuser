use std::sync::LazyLock;

use crate::{
    merge::{self, FrameIndexed, Id},
    trace::{
        Event, Frame, SymbolInfo, Trace,
        metrics::{Metrics, MetricsRange},
    },
};

const DUMMY_RANGE: MetricsRange = MetricsRange {
    start: Metrics {
        ts: 100,
        cycles: 100,
        insn_count: 100,
    },
    end: Metrics {
        ts: 200,
        cycles: 200,
        insn_count: 200,
    },
};

const DUMMY_SYMBOL: LazyLock<SymbolInfo> = LazyLock::new(|| SymbolInfo {
    name: "dummy".to_string(),
    offset: 1,
    size: 1,
});

const DUMMY_FRAME: LazyLock<Frame> =
    LazyLock::new(|| Frame::new(DUMMY_RANGE, DUMMY_SYMBOL.clone()));

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct TestLCS {
    id: u32,
}

impl Id for TestLCS {
    fn id(&self) -> u32 {
        self.id
    }
}

fn produce_frame(symbols: &[&str]) -> Frame {
    let mut frame = Frame::new(DUMMY_RANGE, DUMMY_SYMBOL.clone());
    for (i, &symbol) in symbols.iter().enumerate() {
        frame
            .add_child(Frame::new(
                MetricsRange {
                    start: Metrics::new(100 + i as u64, 100 + i as u64, 100 + i as u64),
                    end: Metrics::new(101 + i as u64, 101 + i as u64, 101 + i as u64),
                },
                SymbolInfo {
                    name: symbol.to_string(),
                    offset: 1,
                    size: 1,
                },
            ))
            .expect(&format!("Failed to add child '{}' to frame", symbol));
    }
    frame
}

fn produce_frame_metrics(root: (u64, u64), children: &[(u64, u64, Option<&SymbolInfo>)]) -> Frame {
    let mut frame = Frame::new(
        MetricsRange::new(Metrics::constant(root.0), Metrics::constant(root.1)),
        DUMMY_SYMBOL.clone(),
    );
    for &(start, end, symbol) in children {
        frame
            .add_child(Frame::new(
                MetricsRange::new(Metrics::constant(start), Metrics::constant(end)),
                symbol.unwrap_or(&DUMMY_SYMBOL).clone(),
            ))
            .expect(&format!(
                "Failed to add child with range ({}, {}) to frame",
                start, end
            ));
    }
    frame
}

fn extract_ids(frames: &Vec<FrameIndexed>) -> Vec<u32> {
    frames.iter().map(|f| f.id).collect()
}

fn seq(xs: &[u32]) -> Vec<TestLCS> {
    xs.iter().map(|x| TestLCS { id: *x }).collect()
}

#[test]
fn index_empty() {
    let (n, r) = merge::index_children(&[]);
    assert_eq!(n, 0);
    assert_eq!(r.len(), 0);
}

#[test]
fn index_single() {
    let frame = produce_frame(&["a", "b", "c"]);
    let (n, r) = merge::index_children(&[&frame]);
    assert_eq!(n, 3);
    assert_eq!(r.len(), 1);
    assert_eq!(extract_ids(&r[0]), vec![1, 2, 3]);
}

#[test]
fn index_3_no_repeat() {
    let frame1 = produce_frame(&["a", "b", "c", "d"]);
    let frame2 = produce_frame(&["b", "c", "e", "g", "h", "d"]);
    let frame3 = produce_frame(&["f", "a", "d", "e"]);
    let (n, r) = merge::index_children(&[&frame1, &frame2, &frame3]);
    assert_eq!(n, 8);
    assert_eq!(r.len(), 3);
    assert_eq!(extract_ids(&r[0]), vec![1, 2, 3, 4]);
    assert_eq!(extract_ids(&r[1]), vec![2, 3, 5, 6, 7, 4]);
    assert_eq!(extract_ids(&r[2]), vec![8, 1, 4, 5]);
}

#[test]
fn index_3_repeating() {
    let frame1 = produce_frame(&["a", "b", "a", "c", "d", "c"]);
    let frame2 = produce_frame(&["b", "c", "a", "a", "e", "g", "e", "h"]);
    let frame3 = produce_frame(&["c", "a", "c", "f", "h", "a", "d", "e"]);
    let (n, r) = merge::index_children(&[&frame1, &frame2, &frame3]);
    assert_eq!(n, 11);
    assert_eq!(r.len(), 3);
    assert_eq!(extract_ids(&r[0]), vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(extract_ids(&r[1]), vec![2, 4, 1, 3, 7, 8, 9, 10]);
    assert_eq!(extract_ids(&r[2]), vec![4, 1, 6, 11, 10, 3, 5, 7]);
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
    merge::find_frequent_frames(1, &[&seq], -0.1);
}

#[test]
#[should_panic]
fn common_thresh_1() {
    let seq = seq(&[]);
    merge::find_frequent_frames(1, &[&seq], 1.1);
}

#[test]
fn common_simple_none() {
    let seq1 = seq(&[1]);
    let seq2 = seq(&[2]);
    let seq3 = seq(&[]);
    let seq4 = seq(&[1]);
    let seqs1 = vec![seq1.as_slice(), &seq2, &seq3, &seq4];
    let seqs2 = vec![seq2.as_slice(), &seq1, &seq4, &seq3];
    let result1 = merge::find_frequent_frames(2, &seqs1, 0.7);
    let result2 = merge::find_frequent_frames(2, &seqs2, 0.7);
    assert_eq!(result1.len(), 0);
    assert_eq!(result2.len(), 0);
}

#[test]
fn common_simple_one() {
    let seq1 = seq(&[1]);
    let seq2 = seq(&[1]);
    let seq3 = seq(&[]);
    let seq4 = seq(&[1]);
    let result = &[1];
    let seqs1 = vec![seq1.as_slice(), &seq2, &seq3, &seq4];
    let seqs2 = vec![seq2.as_slice(), &seq1, &seq4, &seq3];
    let result1 = merge::find_frequent_frames(1, &seqs1, 0.7);
    let result2 = merge::find_frequent_frames(1, &seqs2, 0.7);
    assert_eq!(result1, result);
    assert_eq!(result2, result);
}

#[test]
fn common_three1() {
    let seq1 = seq(&[4, 5, 1, 2, 6, 3]);
    let seq2 = seq(&[7, 1, 8, 9, 2, 10, 3]);
    let seq3 = seq(&[6, 1, 10, 5, 3, 11]);
    let seq4 = seq(&[11, 2, 12, 4, 3]);
    let answer = &[1, 2, 3];
    let seqs1 = vec![seq1.as_slice(), &seq2, &seq3, &seq4];
    let seqs2 = vec![seq2.as_slice(), &seq1, &seq4, &seq3];
    let result1 = merge::find_frequent_frames(12, &seqs1, 0.7);
    let result2 = merge::find_frequent_frames(12, &seqs2, 0.7);
    assert_eq!(result1, answer);
    assert_eq!(result2, answer);
}

#[test]
fn common_three2() {
    let seq1 = seq(&[1, 2, 3, 4, 5, 6, 7]);
    let seq2 = seq(&[8, 2, 10, 4, 12, 13, 14]);
    let seq3 = seq(&[15, 16, 17, 18, 4, 7, 21]);
    let seq4 = seq(&[22, 23, 24, 2, 4, 7, 28]);
    let answer = &[2, 4, 7];
    let seqs1 = vec![seq1.as_slice(), &seq2, &seq3, &seq4];
    let seqs2 = vec![seq2.as_slice(), &seq1, &seq4, &seq3];
    let result1 = merge::find_frequent_frames(28, &seqs1, 0.7);
    let result2 = merge::find_frequent_frames(28, &seqs2, 0.7);
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
    let answer = &[4, 5];
    let seqs1 = vec![seq1.as_slice(), &seq2, &seq3, &seq4];
    let seqs2 = vec![seq2.as_slice(), &seq1, &seq4, &seq3];
    let result1 = merge::find_frequent_frames(22, &seqs1, 0.7);
    let result2 = merge::find_frequent_frames(22, &seqs2, 0.7);
    assert_eq!(result1, answer);
    assert_eq!(result2, answer);
}

#[test]
fn merge_frame_no_children() {
    let frame1 = produce_frame_metrics((500, 590), &[]);
    let frame2 = produce_frame_metrics((300, 380), &[]);
    let frame3 = produce_frame_metrics((400, 464), &[]);
    let merged = merge::merge_frames(
        &[&frame1, &frame2, &frame3],
        Metrics::constant(50),
        &mut Vec::new(),
        0.7,
    );
    assert_eq!(merged.metrics.start, Metrics::constant(50));
    assert_eq!(
        merged.metrics.end,
        Metrics::constant(50 + (90 + 80 + 64) / 3)
    );
}

#[test]
fn merge_frame_common_children() {
    let frame1 = produce_frame_metrics((500, 590), &[(520, 540, None), (550, 558, None)]);
    let frame2 = produce_frame_metrics((300, 380), &[(310, 335, None), (340, 352, None)]);
    let frame3 = produce_frame_metrics((400, 464), &[(415, 430, None), (445, 458, None)]);
    let merged = merge::merge_frames(
        &[&frame1, &frame2, &frame3],
        Metrics::constant(50),
        &mut Vec::new(),
        0.7,
    );
    assert_eq!(merged.metrics.start, Metrics::constant(50));
    assert_eq!(
        merged.metrics.end,
        Metrics::constant(50 + (90 + 80 + 64) / 3)
    );
    assert_eq!(merged.chunks().len(), 5);
    let child1 = &merged.chunks()[1];
    let child2 = &merged.chunks()[3];
    match (child1, child2) {
        (merge::Chunk::Frame(child_frame1), merge::Chunk::Frame(child_frame2)) => {
            assert_eq!(child_frame1.metrics.start, Metrics::constant(50 + 15));
            assert_eq!(child_frame1.metrics.end, Metrics::constant(50 + 15 + 20));
            assert_eq!(child_frame2.metrics.start, Metrics::constant(50 + 45));
            assert_eq!(child_frame2.metrics.end, Metrics::constant(50 + 45 + 11));
        }
        _ => panic!("Expected children to be framesi"),
    }
}

#[test]
fn merge_frame_frequent_children() {
    let common = SymbolInfo {
        name: "common".to_string(),
        offset: 1,
        size: 1,
    };
    let a = SymbolInfo {
        name: "a".to_string(),
        offset: 1,
        size: 1,
    };
    let b = SymbolInfo {
        name: "b".to_string(),
        offset: 1,
        size: 1,
    };
    let c = SymbolInfo {
        name: "c".to_string(),
        offset: 1,
        size: 1,
    };
    let frame1 = produce_frame_metrics(
        (500, 590),
        &[
            (540, 541, Some(&common)),
            (510, 518, Some(&a)),
            (560, 570, Some(&c)),
        ],
    );
    let frame2 = produce_frame_metrics(
        (300, 380),
        &[
            (340, 341, Some(&common)),
            (314, 324, Some(&a)),
            (354, 364, Some(&b)),
        ],
    );
    let frame3 = produce_frame_metrics(
        (400, 464),
        &[
            (440, 441, Some(&common)),
            (450, 456, Some(&b)),
            (400, 410, Some(&c)),
        ],
    );
    let merged = merge::merge_frames(
        &[&frame1, &frame2, &frame3],
        Metrics::constant(50),
        &mut Vec::new(),
        0.6,
    );
    assert_eq!(merged.metrics.start, Metrics::constant(50));
    assert_eq!(
        merged.metrics.end,
        Metrics::constant(50 + (90 + 80 + 64) / 3)
    );
    assert_eq!(merged.chunks().len(), 7);
    let child1 = &merged.chunks()[1];
    let child2 = &merged.chunks()[3];
    let child3 = &merged.chunks()[5];
    match (child1, child2, child3) {
        (
            merge::Chunk::Frame(child_frame1),
            merge::Chunk::Frame(child_frame2),
            merge::Chunk::Frame(child_frame3),
        ) => {
            assert_eq!(child_frame1.metrics.start, Metrics::constant(50 + 12));
            assert_eq!(child_frame1.metrics.end, Metrics::constant(50 + 12 + 9));
            assert_eq!(child_frame1.symbol, a);
            assert_eq!(child_frame2.metrics.start, Metrics::constant(50 + 40));
            assert_eq!(child_frame2.metrics.end, Metrics::constant(50 + 40 + 1));
            assert_eq!(child_frame2.symbol, common);
            assert_eq!(child_frame3.metrics.start, Metrics::constant(50 + 52));
            assert_eq!(child_frame3.metrics.end, Metrics::constant(50 + 52 + 8));
            assert_eq!(child_frame3.symbol, b);
        }
        _ => panic!("Expected children to be framesi"),
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

    let trace1 = Trace::new(DUMMY_FRAME.clone(), vec![event_a1]);
    let trace2 = Trace::new(DUMMY_FRAME.clone(), vec![event_a2, event_b1]);
    let trace3 = Trace::new(DUMMY_FRAME.clone(), vec![event_b2, event_c1]);
    let merged_events = merge::merge_events(&[&trace1, &trace2, &trace3], DUMMY_RANGE);
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
        Frame::new(
            MetricsRange::new(Metrics::constant(250), Metrics::constant(500)),
            DUMMY_SYMBOL.clone(),
        ),
        vec![event_a],
    );
    let trace2 = Trace::new(
        Frame::new(
            MetricsRange::new(Metrics::constant(325), Metrics::constant(425)),
            DUMMY_SYMBOL.clone(),
        ),
        vec![event_b],
    );
    let merged_events = merge::merge_events(
        &[&trace1, &trace2],
        MetricsRange::new(Metrics::constant(20), Metrics::constant(100)),
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
        Frame::new(
            MetricsRange::new(Metrics::constant(250), Metrics::constant(500)),
            DUMMY_SYMBOL.clone(),
        ),
        vec![event_a1],
    );
    let trace2 = Trace::new(
        Frame::new(
            MetricsRange::new(Metrics::constant(325), Metrics::constant(425)),
            DUMMY_SYMBOL.clone(),
        ),
        vec![event_a2],
    );
    let merged_events = merge::merge_events(
        &[&trace1, &trace2],
        MetricsRange::new(Metrics::constant(20), Metrics::constant(100)),
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
