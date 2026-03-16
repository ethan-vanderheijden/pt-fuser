use std::collections::HashMap;

use perfetto_rust::{
    EventName, InternedData, TracePacket, TrackDescriptor, TrackEvent,
    trace_packet::{Data, OptionalTrustedPacketSequenceId, SequenceFlags},
    track_descriptor::StaticOrDynamicName,
    track_event::{self, NameField},
};
use prost::Message;

use crate::trace::{Chunk, Frame, Trace};

// These IDs are arbitrary but must be used consistently
const TRACE_TRACK_ID: u64 = 10;
const TRACE_SEQUENCE_ID: OptionalTrustedPacketSequenceId =
    OptionalTrustedPacketSequenceId::TrustedPacketSequenceId(1);
const TRACE_TRACK_NAME: &str = "Trace";

const ERROR_TRACK_ID_BASE: u64 = 20;
const ERROR_SEQUENCE_ID_BASE: u32 = 2;

fn create_trace_track_start() -> TracePacket {
    let mut trace_start = TracePacket::default();
    trace_start.optional_trusted_packet_sequence_id = Some(TRACE_SEQUENCE_ID);
    trace_start.sequence_flags = Some(SequenceFlags::SeqIncrementalStateCleared as u32);
    trace_start.previous_packet_dropped = Some(true);
    trace_start.first_packet_on_sequence = Some(true);

    let mut description = TrackDescriptor::default();
    description.uuid = Some(TRACE_TRACK_ID);
    description.static_or_dynamic_name = Some(StaticOrDynamicName::StaticName(
        TRACE_TRACK_NAME.to_string(),
    ));
    trace_start.data = Some(Data::TrackDescriptor(description));

    trace_start
}

fn create_slice_begin(timestamp: u64, name_iid: u64) -> TracePacket {
    let mut slice_begin = TracePacket::default();
    slice_begin.optional_trusted_packet_sequence_id = Some(TRACE_SEQUENCE_ID);
    slice_begin.sequence_flags = Some(SequenceFlags::SeqNeedsIncrementalState as u32);
    slice_begin.timestamp = Some(timestamp);

    let mut slice_begin_event = TrackEvent::default();
    slice_begin_event.r#type = Some(track_event::Type::SliceBegin as i32);
    slice_begin_event.track_uuid = Some(TRACE_TRACK_ID);
    slice_begin_event.name_field = Some(NameField::NameIid(name_iid));

    slice_begin.data = Some(Data::TrackEvent(slice_begin_event));

    slice_begin
}

fn create_slice_end(timestamp: u64) -> TracePacket {
    let mut slice_end = TracePacket::default();
    slice_end.optional_trusted_packet_sequence_id = Some(TRACE_SEQUENCE_ID);
    slice_end.sequence_flags = Some(SequenceFlags::SeqNeedsIncrementalState as u32);
    slice_end.timestamp = Some(timestamp);

    let mut slice_end_event = TrackEvent::default();
    slice_end_event.r#type = Some(track_event::Type::SliceEnd as i32);
    slice_end_event.track_uuid = Some(TRACE_TRACK_ID);

    slice_end.data = Some(Data::TrackEvent(slice_end_event));

    slice_end
}

fn create_event_track_start(event_id: u32, name: &str, desc: &str) -> TracePacket {
    let mut event_start = TracePacket::default();
    event_start.optional_trusted_packet_sequence_id = Some(
        OptionalTrustedPacketSequenceId::TrustedPacketSequenceId(ERROR_SEQUENCE_ID_BASE + event_id),
    );
    event_start.sequence_flags = Some(SequenceFlags::SeqIncrementalStateCleared as u32);
    event_start.previous_packet_dropped = Some(true);
    event_start.first_packet_on_sequence = Some(true);

    let mut interned_data = InternedData::default();
    interned_data.event_names = vec![EventName {
        iid: Some(1),
        name: Some(desc.to_string()),
    }];
    event_start.interned_data = Some(interned_data);

    let mut description = TrackDescriptor::default();
    description.uuid = Some(ERROR_TRACK_ID_BASE + event_id as u64);
    description.static_or_dynamic_name = Some(StaticOrDynamicName::StaticName(name.to_string()));
    description.description = Some(desc.to_string());
    event_start.data = Some(Data::TrackDescriptor(description));

    event_start
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

    fn process_frame(&mut self, frame: &Frame) -> Vec<TracePacket> {
        let mut packets = Vec::new();

        let mut intern_data = None;
        let iid = self
            .interned_names
            .get(&frame.symbol.name)
            .copied()
            .unwrap_or_else(|| {
                self.last_iid += 1;
                self.interned_names
                    .insert(frame.symbol.name.clone(), self.last_iid);

                let mut new_intern_data = InternedData::default();
                new_intern_data.event_names = vec![EventName {
                    iid: Some(self.last_iid),
                    name: Some(frame.symbol.name.clone()),
                }];
                intern_data = Some(new_intern_data);

                self.last_iid
            });

        let mut slice_begin = create_slice_begin(frame.metrics.start.ts, iid);
        slice_begin.interned_data = intern_data;
        packets.push(slice_begin);

        for chunk in frame.chunks() {
            match chunk {
                Chunk::Frame(child) => packets.extend(self.process_frame(child)),
                Chunk::Straightline(_) => continue,
            }
        }

        let slice_end = create_slice_end(frame.metrics.end.ts);
        packets.push(slice_end);

        packets
    }
}

pub fn convert_to_perfetto(trace: &Trace) -> Vec<u8> {
    let mut converter = Converter::new();
    let mut packets = converter.process_frame(trace.root_frame());

    let trace_start = create_trace_track_start();
    packets.insert(0, trace_start);

    for event in trace.events() {
        let event_start = create_event_track_start(event.id, &event.name, &event.description);
        packets.push(event_start);

        for occurence in event.occurences() {
            let event_packet = create_event(occurence.ts, event.id);
            packets.push(event_packet);
        }
    }

    let perfetto_trace = perfetto_rust::Trace { packet: packets };
    perfetto_trace.encode_to_vec()
}
