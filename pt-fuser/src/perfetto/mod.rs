use std::collections::HashMap;

use indexmap::IndexMap;
use perfetto_rust::{
    DebugAnnotation, DebugAnnotationName, EventName, InternedData, InternedString, TracePacket,
    TrackDescriptor, TrackEvent, debug_annotation,
    trace_packet::{Data, OptionalTrustedPacketSequenceId, SequenceFlags},
    track_descriptor::{ChildTracksOrdering, StaticOrDynamicName},
    track_event::{self, NameField},
};
use prost::Message;

use crate::trace::{Annotation, Chunk, Frame, Trace};

const GLOBAL_TRACK_ID: u64 = 10;
const GLOBAL_SEQUENCE_ID: u32 = 1;
const GLOBAL_TRACK_NAME: &str = "Process";

// These IDs are arbitrary but must be used consistently
const TRACE_TRACK_ID: u64 = 20;
const TRACE_SEQUENCE_ID: u32 = 2;
const TRACE_TRACK_NAME: &str = "Trace";

const ERROR_TRACK_ID_BASE: u64 = 30;
const ERROR_SEQUENCE_ID_BASE: u32 = 3;

fn create_track(
    timestamp: u64,
    sequence_id: u32,
    uuid: u64,
    name: String,
    desc: Option<String>,
    parent_uuid: Option<u64>,
    child_ordering: Option<ChildTracksOrdering>,
    sibling_order_rank: Option<i32>,
) -> TracePacket {
    let mut track = TracePacket::default();
    track.optional_trusted_packet_sequence_id = Some(
        OptionalTrustedPacketSequenceId::TrustedPacketSequenceId(sequence_id),
    );
    track.sequence_flags = Some(SequenceFlags::SeqIncrementalStateCleared as u32);
    track.previous_packet_dropped = Some(true);
    track.first_packet_on_sequence = Some(true);
    track.timestamp = Some(timestamp);

    let mut description = TrackDescriptor::default();
    description.parent_uuid = parent_uuid;
    description.uuid = Some(uuid);
    description.static_or_dynamic_name = Some(StaticOrDynamicName::StaticName(name));
    description.description = desc;
    description.child_ordering = child_ordering.map(|x| x as i32);
    description.sibling_order_rank = sibling_order_rank;
    track.data = Some(Data::TrackDescriptor(description));

    track
}

fn create_slice_begin(
    timestamp: u64,
    sequence_id: u32,
    track_id: u64,
    name: NameField,
    debug_annotations: Option<Vec<DebugAnnotation>>,
) -> TracePacket {
    let mut slice_begin = TracePacket::default();
    slice_begin.optional_trusted_packet_sequence_id = Some(
        OptionalTrustedPacketSequenceId::TrustedPacketSequenceId(sequence_id),
    );
    slice_begin.sequence_flags = Some(SequenceFlags::SeqNeedsIncrementalState as u32);
    slice_begin.timestamp = Some(timestamp);

    let mut slice_begin_event = TrackEvent::default();
    slice_begin_event.r#type = Some(track_event::Type::SliceBegin as i32);
    slice_begin_event.track_uuid = Some(track_id);
    slice_begin_event.name_field = Some(name);
    if let Some(debug_annotations) = debug_annotations {
        slice_begin_event.debug_annotations = debug_annotations;
    }

    slice_begin.data = Some(Data::TrackEvent(slice_begin_event));

    slice_begin
}

fn create_slice_end(timestamp: u64, sequence_id: u32, track_id: u64) -> TracePacket {
    let mut slice_end = TracePacket::default();
    slice_end.optional_trusted_packet_sequence_id = Some(
        OptionalTrustedPacketSequenceId::TrustedPacketSequenceId(sequence_id),
    );
    slice_end.sequence_flags = Some(SequenceFlags::SeqNeedsIncrementalState as u32);
    slice_end.timestamp = Some(timestamp);

    let mut slice_end_event = TrackEvent::default();
    slice_end_event.r#type = Some(track_event::Type::SliceEnd as i32);
    slice_end_event.track_uuid = Some(track_id);

    slice_end.data = Some(Data::TrackEvent(slice_end_event));

    slice_end
}

fn create_event(timestamp: u64, event_id: u32) -> TracePacket {
    let mut event = TracePacket::default();
    event.optional_trusted_packet_sequence_id = Some(
        OptionalTrustedPacketSequenceId::TrustedPacketSequenceId(ERROR_SEQUENCE_ID_BASE + event_id),
    );
    event.sequence_flags = Some(SequenceFlags::SeqNeedsIncrementalState as u32);
    event.timestamp = Some(timestamp);

    let mut instant_event = TrackEvent::default();
    instant_event.r#type = Some(track_event::Type::Instant as i32);
    instant_event.track_uuid = Some(ERROR_TRACK_ID_BASE + event_id as u64);
    instant_event.name_field = Some(NameField::NameIid(1));

    event.data = Some(Data::TrackEvent(instant_event));

    event
}

struct StackFrame {
    iid: u64,
    annotations: Option<Vec<DebugAnnotation>>,
}

struct Converter {
    interned_names: HashMap<String, u64>,
    last_iid: u64,
}

impl Converter {
    fn new() -> Self {
        Self {
            interned_names: HashMap::new(),
            last_iid: 0,
        }
    }

    fn intern_string(&mut self, s: &str) -> (bool, u64) {
        if let Some(&iid) = self.interned_names.get(s) {
            (false, iid)
        } else {
            self.last_iid += 1;
            self.interned_names.insert(s.to_string(), self.last_iid);
            (true, self.last_iid)
        }
    }

    /// Precondition: annotation cannot be a Map or an Array
    fn convert_basic_annotation(
        &mut self,
        annotation: &Annotation,
        interned_data: &mut InternedData,
    ) -> debug_annotation::Value {
        match annotation {
            Annotation::Bool(b) => debug_annotation::Value::BoolValue(*b),
            Annotation::Uint64(i) => debug_annotation::Value::UintValue(*i),
            Annotation::Int64(i) => debug_annotation::Value::IntValue(*i),
            Annotation::Double(d) => debug_annotation::Value::DoubleValue(*d),
            Annotation::Pointer(p) => debug_annotation::Value::PointerValue(*p),
            Annotation::String(s) => {
                let (is_new, iid) = self.intern_string(s);
                if is_new {
                    interned_data
                        .debug_annotation_string_values
                        .push(InternedString {
                            iid: Some(iid),
                            str: Some(s.clone().into_bytes()),
                        });
                }
                debug_annotation::Value::StringValueIid(iid)
            }
            _ => panic!("Unsupported annotation type for basic conversion"),
        }
    }

    fn convert_annotation_map(
        &mut self,
        annotations: &IndexMap<String, Annotation>,
        interned_data: &mut InternedData,
    ) -> Vec<DebugAnnotation> {
        let mut result = Vec::new();
        for (key, value) in annotations {
            let (is_new, key_iid) = self.intern_string(key);
            if is_new {
                interned_data
                    .debug_annotation_names
                    .push(DebugAnnotationName {
                        iid: Some(key_iid),
                        name: Some(key.clone()),
                    });
            }

            let mut debug_annotation = DebugAnnotation::default();
            debug_annotation.name_field = Some(debug_annotation::NameField::NameIid(key_iid));
            match value {
                Annotation::Map(map) => {
                    debug_annotation.dict_entries = self.convert_annotation_map(map, interned_data);
                }
                Annotation::Array(arr) => {
                    debug_annotation.array_values =
                        self.convert_annotation_array(arr, interned_data);
                }
                _ => {
                    debug_annotation.value =
                        Some(self.convert_basic_annotation(value, interned_data));
                }
            }
            result.push(debug_annotation);
        }
        result
    }

    fn convert_annotation_array(
        &mut self,
        annotations: &Vec<Annotation>,
        interned_data: &mut InternedData,
    ) -> Vec<DebugAnnotation> {
        annotations
            .iter()
            .map(|elem| {
                let mut debug_annotation = DebugAnnotation::default();
                match elem {
                    Annotation::Map(map) => {
                        debug_annotation.dict_entries =
                            self.convert_annotation_map(map, interned_data);
                    }
                    Annotation::Array(arr) => {
                        debug_annotation.array_values =
                            self.convert_annotation_array(arr, interned_data);
                    }
                    _ => {
                        debug_annotation.value =
                            Some(self.convert_basic_annotation(elem, interned_data));
                    }
                }
                debug_annotation
            })
            .collect()
    }

    fn process_frame(
        &mut self,
        frame: &Frame,
        stack_iid: &mut Vec<StackFrame>,
    ) -> Vec<TracePacket> {
        let mut packets = Vec::new();

        let (is_new, iid) = self.intern_string(&frame.symbol.name);
        let mut intern_data = InternedData::default();
        if is_new {
            intern_data.event_names = vec![EventName {
                iid: Some(iid),
                name: Some(frame.symbol.name.clone()),
            }];
        }

        let annotations = frame.annotations.as_ref().map(|a| {
            self.convert_annotation_map(a, &mut intern_data)
        });
        let mut slice_begin = create_slice_begin(
            frame.metrics.start.ts,
            TRACE_SEQUENCE_ID,
            TRACE_TRACK_ID,
            NameField::NameIid(iid),
            annotations.clone(),
        );
        slice_begin.interned_data = Some(intern_data);
        packets.push(slice_begin);

        stack_iid.push(StackFrame {
            iid,
            annotations: annotations,
        });

        for chunk in frame.chunks() {
            match chunk {
                Chunk::Frame(child) => packets.extend(self.process_frame(child, stack_iid)),
                Chunk::Straightline(_) => continue,
                Chunk::Pause(metrics) => {
                    // pretend all previous stack frames end here
                    for _ in 0..stack_iid.len() {
                        let slice_end =
                            create_slice_end(metrics.start.ts, TRACE_SEQUENCE_ID, TRACE_TRACK_ID);
                        packets.push(slice_end);
                    }

                    // previous stack frames resume once pause is over
                    // in Perfetto, this appears as a blank gap, indicating that tracing was paused
                    let resume = metrics.end().ts;
                    for stack_frame in stack_iid.iter() {
                        let slice_begin = create_slice_begin(
                            resume,
                            TRACE_SEQUENCE_ID,
                            TRACE_TRACK_ID,
                            NameField::NameIid(stack_frame.iid),
                            stack_frame.annotations.clone(),
                        );
                        packets.push(slice_begin);
                    }
                }
            }
        }

        stack_iid.pop();

        let slice_end = create_slice_end(frame.metrics.end().ts, TRACE_SEQUENCE_ID, TRACE_TRACK_ID);
        packets.push(slice_end);

        packets
    }
}

pub fn convert_to_perfetto(trace: &Trace) -> Vec<u8> {
    let mut converter = Converter::new();

    let mut packets = Vec::new();
    packets.push(create_track(
        trace.root_frame().metrics.start.ts,
        GLOBAL_SEQUENCE_ID,
        GLOBAL_TRACK_ID,
        GLOBAL_TRACK_NAME.to_string(),
        None,
        None,
        Some(ChildTracksOrdering::Explicit),
        None,
    ));
    packets.push(create_slice_begin(
        trace.root_frame().metrics.start.ts,
        GLOBAL_SEQUENCE_ID,
        GLOBAL_TRACK_ID,
        NameField::Name("Overall Latency".to_string()),
        None,
    ));
    packets.push(create_slice_end(
        trace.root_frame().metrics.end().ts,
        GLOBAL_SEQUENCE_ID,
        GLOBAL_TRACK_ID,
    ));

    packets.push(create_track(
        trace.root_frame().metrics.start.ts,
        TRACE_SEQUENCE_ID,
        TRACE_TRACK_ID,
        TRACE_TRACK_NAME.to_string(),
        None,
        Some(GLOBAL_TRACK_ID),
        None,
        Some(i32::MAX),
    ));
    packets.extend(converter.process_frame(trace.root_frame(), &mut Vec::new()));

    for event in trace.events() {
        if let Some(first_occurence) = event.occurences().first() {
            // Map 0..=u32::MAX to i32::MIN..=i32::MAX while preserving order
            let scaled_id = (event.id ^ 0x80000000) as i32;
            let mut event_start = create_track(
                first_occurence.ts,
                ERROR_SEQUENCE_ID_BASE + event.id,
                ERROR_TRACK_ID_BASE + event.id as u64,
                event.name.clone(),
                Some(event.description.clone()),
                Some(GLOBAL_TRACK_ID),
                None,
                Some(scaled_id),
            );

            let mut interned_data = InternedData::default();
            interned_data.event_names = vec![EventName {
                iid: Some(1),
                name: Some(event.description.clone()),
            }];
            event_start.interned_data = Some(interned_data);

            packets.push(event_start);

            for occurence in event.occurences() {
                let event_packet = create_event(occurence.ts, event.id);
                packets.push(event_packet);
            }
        }
    }

    let perfetto_trace = perfetto_rust::Trace { packet: packets };
    perfetto_trace.encode_to_vec()
}
