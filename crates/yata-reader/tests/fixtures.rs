//! The public fixtures: a session recording and an export of the synthetic inventory, made by the
//! reader's own session and export code over synthetic memory. Decoding must stay deterministic:
//! the same build reads the same bytes into the same frames, byte for byte.
//!
//! The main repository keeps a pinned copy of the recording beside the daemon test that replays
//! it. After an intended change, rewrite both files with
//! `cargo test --test fixtures -- --ignored bless` and copy the recording there.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use prost::Message;
use yata_protocol::export;
use yata_protocol::failure::Exit;
use yata_protocol::frame::{self, FrameDecoder};
use yata_protocol::probe::{
    Handshake, PointerWidth, ProbeMessage, ReadRequest, Scope, Shutdown, TargetProcess, VERSION,
    probe_message::Kind, reading::Records,
};
use yata_reader::backend::ImageBackend;
use yata_reader::diagnostics::Diagnostics;
use yata_reader::layout::cpython::DictKeys;
use yata_reader::layout::fixture::inventory;
use yata_reader::session::{self, Outgoing, Target};

const RECORDING: &str = "synthetic-souls.frames";
const EXPORT: &str = "synthetic-souls.export.json";
const BUILD: &str = "yata-reader synthetic-fixture";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn target() -> TargetProcess {
    TargetProcess {
        pid: 4242,
        image_name: "synthetic.exe".into(),
        pointer_width: Some(PointerWidth::PointerWidth64.into()),
        ..TargetProcess::default()
    }
}

#[derive(Clone, Default)]
struct Tape(Arc<Mutex<Vec<u8>>>);

impl Write for Tape {
    #[allow(
        clippy::expect_used,
        reason = "test helper: a failure here is the test failing"
    )]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("tape").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[allow(
    clippy::expect_used,
    reason = "test helper: a failure here is the test failing"
)]
fn frame_of(kind: Kind) -> Vec<u8> {
    frame::encode(&ProbeMessage { kind: Some(kind) }.encode_to_vec()).expect("encodable")
}

#[allow(
    clippy::expect_used,
    reason = "test helper: a failure here is the test failing"
)]
fn backend() -> ImageBackend {
    ImageBackend::new(inventory(DictKeys::Logged).expect("disjoint"), target())
}

/// The frames a reader sends in a whole session: handshake, one read of the souls, shutdown.
#[allow(
    clippy::expect_used,
    reason = "test helper: a failure here is the test failing"
)]
fn session_recording() -> Vec<u8> {
    let (reader_in, mut to_reader) = std::io::pipe().expect("pipe");
    let tape = Tape::default();
    let out: Outgoing = Arc::new(Mutex::new(Box::new(tape.clone())));
    let reader = std::thread::spawn(move || {
        session::run(backend(), reader_in, out, BUILD, &Diagnostics::none())
    });
    to_reader
        .write_all(&frame_of(Kind::Handshake(Handshake {
            version: Some(VERSION),
            expected_engine: None,
            target: Some(Target::Discover.wire()),
        })))
        .expect("written");
    to_reader
        .write_all(&frame_of(Kind::ReadRequest(ReadRequest {
            request_id: 1,
            scope: Scope::Souls.into(),
        })))
        .expect("written");
    // Shutdown finishes nothing in flight, so it is sent only once the answer is recorded.
    loop {
        let bytes = tape.0.lock().expect("tape").clone();
        let mut d = FrameDecoder::new();
        d.push(&bytes);
        let mut answered = false;
        while let Ok(Some(p)) = d.next_frame() {
            if let Ok(ProbeMessage {
                kind: Some(Kind::ReadResult(_)),
            }) = ProbeMessage::decode(p.as_slice())
            {
                answered = true;
            }
        }
        if answered {
            break;
        }
        std::thread::yield_now();
    }
    to_reader
        .write_all(&frame_of(Kind::Shutdown(Shutdown {})))
        .expect("written");
    assert_eq!(reader.join().expect("reader"), Exit::Clean);
    tape.0.lock().expect("tape").clone()
}

#[allow(
    clippy::expect_used,
    reason = "test helper: a failure here is the test failing"
)]
fn export_text() -> String {
    let e = yata_reader::export::read(
        &mut backend(),
        Target::Discover,
        BUILD,
        "2026-09-25T00:00:00Z",
        &mut |_, _| (),
    )
    .expect("read");
    export::to_json(&e).expect("json")
}

#[test]
fn the_session_recording_is_reproduced_byte_for_byte() {
    let expected = std::fs::read(fixture(RECORDING)).expect("the committed recording");
    assert!(session_recording() == expected, "{RECORDING} is stale");
}

#[test]
fn the_export_is_reproduced_byte_for_byte() {
    let expected = std::fs::read_to_string(fixture(EXPORT)).expect("the committed export");
    // Git may check a text fixture out with Windows line ends.
    assert_eq!(export_text(), expected.replace("\r\n", "\n"));
}

#[test]
fn the_export_is_a_valid_export_of_four_souls() {
    let e = export::from_json(export_text().as_bytes()).expect("valid");
    let Some(Records::Souls(s)) = &e.readings[0].records else {
        panic!("souls")
    };
    assert_eq!(s.souls.len(), 4);
}

#[test]
#[ignore = "rewrites the committed fixtures"]
fn bless() {
    std::fs::write(fixture(RECORDING), session_recording()).expect("written");
    std::fs::write(fixture(EXPORT), export_text()).expect("written");
}

#[test]
fn a_recording_is_read_to_its_end() {
    let bytes = session_recording();
    let mut d = FrameDecoder::new();
    let mut r = bytes.as_slice();
    let mut buf = [0u8; 7];
    loop {
        let n = r.read(&mut buf).expect("readable");
        if n == 0 {
            break;
        }
        d.push(&buf[..n]);
        while d.next_frame().expect("framed").is_some() {}
    }
    assert_eq!(d.finish(), Ok(()));
}
