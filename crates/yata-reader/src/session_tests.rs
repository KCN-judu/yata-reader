use std::io::{Read, Write};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use yata_protocol::probe::{
    Cancel, Handshake, ProtocolVersion, ReadRequest, Shutdown, read_result::Records,
};

use super::*;
use crate::backend::ImageBackend;
use crate::layout::cpython::DictKeys;
use crate::layout::fixture::inventory;

/// A backend whose read runs until it is cancelled, reporting progress as it goes, or fails
/// attaching with a given failure.
struct Stub {
    attach: Result<(), Failure>,
    reads: Arc<Mutex<u32>>,
    started: Arc<AtomicBool>,
}

impl Backend for Stub {
    fn attach(&mut self, _pid: u32) -> Result<Attached, Failure> {
        self.attach.clone().map(|()| Attached {
            engine: "stub".into(),
            channel: Channel::DesktopMemory,
            target: TargetProcess::default(),
        })
    }

    fn read(
        &mut self,
        _scope: Scope,
        progress: &mut dyn FnMut(u64, u64),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ReadResult, Failure> {
        *self.reads.lock().expect("count") += 1;
        self.started.store(true, Ordering::SeqCst);
        let mut done = 0;
        while !cancelled() {
            done += 1;
            progress(done, 0);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        Err(Failure::new(code::CANCELLED, "cancelled", 0))
    }
}

fn stub(attach: Result<(), Failure>) -> (Stub, Arc<Mutex<u32>>, Arc<AtomicBool>) {
    let reads = Arc::new(Mutex::new(0));
    let started = Arc::new(AtomicBool::new(false));
    (
        Stub {
            attach,
            reads: reads.clone(),
            started: started.clone(),
        },
        reads,
        started,
    )
}

fn frame_of(kind: Kind) -> Vec<u8> {
    frame::encode(&ProbeMessage { kind: Some(kind) }.encode_to_vec()).expect("encodable")
}

fn handshake(major: u32) -> Kind {
    Kind::Handshake(Handshake {
        version: Some(ProtocolVersion { major, minor: 0 }),
        expected_engine: String::new(),
        target_pid: 0,
    })
}

fn request(id: u64, scope: Scope) -> Kind {
    Kind::ReadRequest(ReadRequest {
        request_id: id,
        scope: scope.into(),
    })
}

/// A daemon end: what it writes goes to the reader, and it reads what the reader writes.
struct Daemon {
    to_reader: std::io::PipeWriter,
    from_reader: std::io::PipeReader,
    decoder: FrameDecoder,
}

impl Daemon {
    fn send(&mut self, kind: Kind) {
        self.to_reader.write_all(&frame_of(kind)).expect("written");
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.to_reader.write_all(bytes).expect("written");
    }

    fn receive(&mut self) -> Option<Kind> {
        let mut buf = [0u8; 4096];
        loop {
            if let Some(p) = self.decoder.next_frame().expect("framed") {
                return ProbeMessage::decode(p.as_slice()).expect("decodable").kind;
            }
            let n = self.from_reader.read(&mut buf).expect("readable");
            if n == 0 {
                return None;
            }
            self.decoder.push(&buf[..n]);
        }
    }

    /// Skip progress to the next other message.
    fn answer(&mut self) -> Option<Kind> {
        loop {
            match self.receive() {
                Some(Kind::Progress(_)) => continue,
                other => return other,
            }
        }
    }
}

fn start(backend: impl Backend) -> (Daemon, std::thread::JoinHandle<u8>) {
    let (reader_in, to_reader) = std::io::pipe().expect("pipe");
    let (from_reader, reader_out) = std::io::pipe().expect("pipe");
    let out: Outgoing = Arc::new(Mutex::new(Box::new(reader_out)));
    let reader =
        std::thread::spawn(move || run(backend, reader_in, out, "test", &Diagnostics::none()));
    (
        Daemon {
            to_reader,
            from_reader,
            decoder: FrameDecoder::new(),
        },
        reader,
    )
}

fn failure(kind: Option<Kind>) -> (u64, ProbeError) {
    match kind {
        Some(Kind::Failed(f)) => (f.request_id, f.error.expect("an error")),
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn a_session_reads_the_souls_and_shuts_down_cleanly() {
    let backend = ImageBackend::new(inventory(DictKeys::Logged), TargetProcess::default());
    let (mut d, reader) = start(backend);
    d.send(handshake(1));
    let Some(Kind::HandshakeAck(ack)) = d.receive() else {
        panic!("ack")
    };
    assert_eq!(ack.engine, "cpython-3.11");
    assert_eq!(ack.probe_build_id, "test");
    d.send(request(1, Scope::Souls));
    let mut progress = 0;
    let result = loop {
        match d.receive() {
            Some(Kind::Progress(p)) => {
                assert_eq!(p.request_id, 1);
                progress += 1;
            }
            Some(Kind::ReadResult(r)) => break r,
            other => panic!("unexpected {other:?}"),
        }
    };
    assert!(progress > 0);
    assert_eq!(result.request_id, 1);
    let Some(Records::Souls(s)) = result.records else {
        panic!("souls")
    };
    assert_eq!(s.souls.len(), 4);
    d.send(Kind::Shutdown(Shutdown {}));
    assert_eq!(reader.join().expect("reader"), exit::CLEAN);
}

#[test]
fn an_attach_failure_is_reported_with_its_exit_code() {
    let (backend, _, _) = stub(Err(Failure::new(
        code::ELEVATION_REQUIRED,
        "elevated game",
        exit::ELEVATION_REQUIRED,
    )));
    let (mut d, reader) = start(backend);
    d.send(handshake(1));
    let (id, e) = failure(d.receive());
    assert_eq!(id, 0);
    assert_eq!(e.code, code::ELEVATION_REQUIRED);
    assert_eq!(reader.join().expect("reader"), exit::ELEVATION_REQUIRED);
}

#[test]
fn another_major_version_is_refused() {
    let (backend, _, _) = stub(Ok(()));
    let (mut d, reader) = start(backend);
    d.send(handshake(2));
    let (_, e) = failure(d.receive());
    assert_eq!(e.code, code::PROTOCOL_UNSUPPORTED);
    assert_eq!(reader.join().expect("reader"), exit::PROTOCOL);
}

#[test]
fn a_malformed_frame_is_a_protocol_error() {
    let (backend, _, _) = stub(Ok(()));
    let (mut d, reader) = start(backend);
    d.raw(&[0xff, 0xff, 0xff, 0xff]);
    let (id, e) = failure(d.receive());
    assert_eq!((id, e.code.as_str()), (0, code::PROTOCOL_ERROR));
    assert_eq!(reader.join().expect("reader"), exit::PROTOCOL);
}

#[test]
fn a_request_before_the_handshake_is_a_protocol_error() {
    let (backend, _, _) = stub(Ok(()));
    let (mut d, reader) = start(backend);
    d.send(request(1, Scope::Souls));
    let (_, e) = failure(d.receive());
    assert_eq!(e.code, code::PROTOCOL_ERROR);
    assert_eq!(reader.join().expect("reader"), exit::PROTOCOL);
}

#[test]
fn a_cancel_stops_a_running_read_with_one_answer() {
    let (backend, reads, _) = stub(Ok(()));
    let (mut d, reader) = start(backend);
    d.send(handshake(1));
    assert!(matches!(d.receive(), Some(Kind::HandshakeAck(_))));
    d.send(request(1, Scope::Souls));
    assert!(matches!(d.receive(), Some(Kind::Progress(_))));
    d.send(Kind::Cancel(Cancel { request_id: 1 }));
    let (id, e) = failure(d.answer());
    assert_eq!((id, e.code.as_str()), (1, code::CANCELLED));
    // A cancel for an answered request is ignored.
    d.send(Kind::Cancel(Cancel { request_id: 1 }));
    d.send(Kind::Shutdown(Shutdown {}));
    assert_eq!(reader.join().expect("reader"), exit::CLEAN);
    assert_eq!(*reads.lock().expect("count"), 1);
}

#[test]
fn a_request_cancelled_before_it_starts_is_never_read() {
    let (backend, reads, started) = stub(Ok(()));
    let (mut d, reader) = start(backend);
    d.send(handshake(1));
    assert!(matches!(d.receive(), Some(Kind::HandshakeAck(_))));
    d.send(request(1, Scope::Souls));
    d.send(request(2, Scope::Souls));
    while !started.load(Ordering::SeqCst) {
        std::thread::yield_now();
    }
    d.send(Kind::Cancel(Cancel { request_id: 2 }));
    d.send(Kind::Cancel(Cancel { request_id: 1 }));
    let answers = [failure(d.answer()), failure(d.answer())];
    assert_eq!(answers[0].0, 1);
    assert_eq!(answers[1].0, 2);
    assert_eq!(answers[1].1.message, "cancelled before it started");
    d.send(Kind::Shutdown(Shutdown {}));
    assert_eq!(reader.join().expect("reader"), exit::CLEAN);
    assert_eq!(*reads.lock().expect("count"), 1);
}

#[test]
fn a_cancel_for_an_unknown_request_is_a_protocol_error() {
    let (backend, _, _) = stub(Ok(()));
    let (mut d, reader) = start(backend);
    d.send(handshake(1));
    assert!(matches!(d.receive(), Some(Kind::HandshakeAck(_))));
    d.send(Kind::Cancel(Cancel { request_id: 9 }));
    let (_, e) = failure(d.receive());
    assert_eq!(e.code, code::PROTOCOL_ERROR);
    assert_eq!(reader.join().expect("reader"), exit::PROTOCOL);
}

#[test]
fn a_reused_request_id_is_a_protocol_error() {
    let backend = ImageBackend::new(inventory(DictKeys::Sized), TargetProcess::default());
    let (mut d, reader) = start(backend);
    d.send(handshake(1));
    assert!(matches!(d.receive(), Some(Kind::HandshakeAck(_))));
    d.send(request(1, Scope::Souls));
    assert!(matches!(d.answer(), Some(Kind::ReadResult(_))));
    d.send(request(1, Scope::Souls));
    let (_, e) = failure(d.receive());
    assert_eq!(e.code, code::PROTOCOL_ERROR);
    assert_eq!(reader.join().expect("reader"), exit::PROTOCOL);
}

#[test]
fn an_unknown_scope_is_answered_unsupported() {
    let backend = ImageBackend::new(inventory(DictKeys::Logged), TargetProcess::default());
    let (mut d, reader) = start(backend);
    d.send(handshake(1));
    assert!(matches!(d.receive(), Some(Kind::HandshakeAck(_))));
    d.send(request(1, Scope::Unspecified));
    let (id, e) = failure(d.answer());
    assert_eq!((id, e.code.as_str()), (1, code::SCOPE_UNSUPPORTED));
    drop(d);
    assert_eq!(reader.join().expect("reader"), exit::CLEAN);
}

#[test]
fn a_closed_pipe_ends_the_session_cleanly() {
    let (backend, _, _) = stub(Ok(()));
    let (d, reader) = start(backend);
    drop(d);
    assert_eq!(reader.join().expect("reader"), exit::CLEAN);
}
