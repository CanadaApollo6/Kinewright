//! Write the seed corpus (structured records) to the directory in argv[1].
mod framing;

use kinewright_core::ColorDescription;
use kinewright_media_ipc::{
    Color, Kind, MediaSandboxError, Message, ProbeResult, Stream, StreamKind,
};

fn result(kind: Kind, video: Option<u32>, audio: Option<u32>) -> Message {
    let color = ColorDescription::unknown();
    let stream = |index, kind| Stream {
        index,
        kind,
        time_base: [1, 25],
        timestamp_origin: 0,
        duration: 25,
        average_frame_rate: [25, 1],
    };
    let mut streams = Vec::new();
    if let Some(index) = video {
        streams.push(stream(index, StreamKind::Video));
    }
    if let Some(index) = audio {
        streams.push(stream(index, StreamKind::Audio));
    }
    Message::ProbeResult {
        result: Box::new(ProbeResult {
            streams,
            duration_microseconds: 1_000_000,
            video_stream: video,
            audio_stream: audio,
            duration_frames: 25,
            fps: [25, 1],
            kind,
            resolution: video.map(|_| [64, 32]),
            color: Color {
                primaries: color.primaries,
                transfer: color.transfer,
                matrix: color.matrix,
                range: color.range,
                white_point: color.white_point,
                bit_depth: color.bit_depth,
                confidence_basis_points: color.confidence_basis_points,
                provenance: color.provenance,
            },
        }),
    }
}

fn records(messages: &[(Message, u8)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (message, flags) in messages {
        let (tag, payload) = encoded(message);
        out.push(tag);
        out.push(*flags);
        out.extend_from_slice(
            &u16::try_from(payload.len())
                .expect("small seed")
                .to_le_bytes(),
        );
        out.extend_from_slice(&payload);
    }
    out
}

/// The tag and payload bytes `write_frame` sends.
fn encoded(message: &Message) -> (u8, Vec<u8>) {
    let mut frame = Vec::new();
    kinewright_media_ipc::write_frame(
        &mut frame,
        &kinewright_media_ipc::Frame {
            request: 1,
            generation: 1,
            message: message.clone(),
        },
    )
    .expect("a valid seed frame");
    let payload = frame.split_off(kinewright_media_ipc::HEADER_BYTES);
    (frame[20], payload)
}

fn main() {
    let directory = std::path::PathBuf::from(std::env::args().nth(1).expect("corpus directory"));
    std::fs::create_dir_all(&directory).expect("corpus directory");
    let video = result(Kind::Video, Some(0), None);
    let av = result(Kind::AudioVideo, Some(0), Some(1));
    let seeds: Vec<(u8, Vec<(Message, u8)>)> = vec![
        (
            0,
            vec![
                (Message::Ready {}, 0),
                (video.clone(), 0),
                (Message::Complete {}, 0),
            ],
        ),
        (
            6,
            vec![
                (Message::Ready {}, 0),
                (av.clone(), 0),
                (Message::Complete {}, 0),
            ],
        ),
        (2, vec![(Message::Ready {}, 0), (av, 0)]),
        (
            0,
            vec![
                (Message::Ready {}, 0),
                (
                    Message::Failed {
                        error: MediaSandboxError::UnsupportedMedia,
                    },
                    0,
                ),
            ],
        ),
        (0, vec![(video.clone(), 0), (Message::Ready {}, 0)]),
        (0, vec![(Message::Ready {}, 1), (video, 0)]),
        (
            0,
            vec![
                (Message::Ready {}, 0),
                (result(Kind::Audio, None, Some(0)), 0),
                (Message::Close {}, 0),
            ],
        ),
        (
            0,
            vec![
                (
                    Message::Probe {
                        video: Some(0),
                        audio: None,
                    },
                    0,
                ),
                (Message::Cancel {}, 0),
            ],
        ),
    ];
    for (index, (mode, messages)) in seeds.into_iter().enumerate() {
        let mut bytes = vec![mode];
        bytes.extend(records(&messages));
        std::fs::write(directory.join(format!("structured-{index}")), &bytes).expect("seed");
        // The same exchange as a raw stream for the odd (raw) mode.
        let mut raw = vec![mode | 1];
        raw.extend(framing::stream(&framing::prefix(), &records(&messages)));
        std::fs::write(directory.join(format!("raw-{index}")), &raw).expect("seed");
    }
}
