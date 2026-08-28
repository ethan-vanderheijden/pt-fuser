mod converter;

use std::{
    fmt::Display,
    fs::File,
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use clap::ValueEnum;
use prost::Message;

use crate::{perfetto::converter::Converter, trace::Trace};

const QUEUED_PACKETS: usize = 512;

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum PauseRenderOption {
    Gap,
    Block,
}

impl Display for PauseRenderOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PauseRenderOption::Gap => write!(f, "gap"),
            PauseRenderOption::Block => write!(f, "block"),
        }
    }
}

pub fn convert_to_perfetto(trace: &Trace, output_file: &str, render_pauses: PauseRenderOption) {
    let (sender, receiver) = mpsc::sync_channel::<perfetto_rust::TracePacket>(QUEUED_PACKETS);
    let stop_flag = Arc::new(AtomicBool::new(false));

    let mut converter = Converter::new(trace, render_pauses, stop_flag.clone());
    ctrlc::set_handler(move || {
        stop_flag.store(true, Ordering::Relaxed);
        println!("Ending conversion as-is due to Ctrl-C");
    })
    .expect("Error setting Ctrl-C handler");

    let mut file = File::create(output_file).expect("Failed to create output file");
    let writer = thread::spawn(move || {
        for packet in receiver {
            let packet = perfetto_rust::Trace {
                packet: vec![packet],
            };
            let encoded = packet.encode_to_vec();
            file.write_all(&encoded)
                .expect("Failed to write encoded data to file.");
        }
    });
    converter.start(sender);
    writer.join().expect("Failed to join file writing thread");
}
