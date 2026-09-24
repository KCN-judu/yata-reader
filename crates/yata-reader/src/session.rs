//! The reader's side of the probe protocol (the main repository's `probe-protocol.md`): the
//! handshake, request dispatch, cancellation, and shutdown, over any byte stream.
//!
//! The main thread reads the daemon's frames; one worker thread answers read requests in order,
//! so a `Cancel` is seen while a read runs. Only the worker writes an answer, and it writes
//! exactly one per request, through the request discipline shared with the daemon. A request
//! cancelled before it starts is answered `probe.cancelled` without being read.
//!
//! The session ends when the daemon closes the stream or sends `Shutdown` (exit 0), when the
//! daemon breaks the protocol (`Failed` with `probe.protocol_error`, exit 3), or when attaching
//! fails (`Failed` with the reason, and the reason's exit code).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use prost::Message;
use yata_protocol::discipline::{CancelEffect, Ledger};
use yata_protocol::frame::{self, FrameDecoder};
use yata_protocol::probe::{
    self, Channel, Failed, HandshakeAck, ProbeError, ProbeMessage, Progress, ReadResult, Scope,
    TargetProcess, code, exit, probe_message::Kind,
};

use crate::diagnostics::Diagnostics;

/// What attaching found.
#[derive(Debug, Clone, PartialEq)]
pub struct Attached {
    pub engine: String,
    pub channel: Channel,
    pub target: TargetProcess,
}

/// A failure the daemon is told about.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
    pub candidates: Vec<TargetProcess>,
    pub os_error: u32,
    /// The exit code, when the failure ends the reader.
    pub exit: u8,
}

impl Failure {
    pub fn new(code: &'static str, message: impl Into<String>, exit: u8) -> Failure {
        Failure {
            code,
            message: message.into(),
            candidates: Vec::new(),
            os_error: 0,
            exit,
        }
    }

    pub fn error(&self) -> ProbeError {
        ProbeError {
            code: self.code.to_owned(),
            message: self.message.clone(),
            details: Vec::new(),
            candidates: self.candidates.clone(),
            os_error: self.os_error,
        }
    }
}

/// How the reader reaches the game: the desktop channel, or synthetic memory in tests.
pub trait Backend: Send + 'static {
    fn attach(&mut self, target_pid: u32) -> Result<Attached, Failure>;

    /// Read a scope. `progress` gets `(done, total)`; `cancelled` is checked at checkpoints.
    fn read(
        &mut self,
        scope: Scope,
        progress: &mut dyn FnMut(u64, u64),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ReadResult, Failure>;
}

/// Where the reader writes frames; shared by the main thread and the worker.
pub type Outgoing = Arc<Mutex<Box<dyn Write + Send>>>;

/// Write one message as one frame.
pub fn send(out: &Outgoing, kind: Kind) -> std::io::Result<()> {
    let payload = ProbeMessage { kind: Some(kind) }.encode_to_vec();
    let bytes = frame::encode(&payload).map_err(|e| std::io::Error::other(format!("{e:?}")))?;
    let mut w = out
        .lock()
        .map_err(|_| std::io::Error::other("the frame writer is poisoned"))?;
    w.write_all(&bytes)?;
    w.flush()
}

fn failed(out: &Outgoing, request_id: u64, error: ProbeError) {
    let _ = send(
        out,
        Kind::Failed(Failed {
            request_id,
            error: Some(error),
        }),
    );
}

/// Run a session until it ends; the result is the process exit code.
pub fn run(
    mut backend: impl Backend,
    mut incoming: impl Read,
    out: Outgoing,
    build_id: &str,
    diag: &Diagnostics,
) -> u8 {
    let mut decoder = FrameDecoder::new();
    let protocol_error = |message: String| {
        diag.line(&format!("protocol error: {message}"));
        failed(
            &out,
            0,
            Failure::new(code::PROTOCOL_ERROR, message, exit::PROTOCOL).error(),
        );
        exit::PROTOCOL
    };
    let handshake = match next(&mut incoming, &mut decoder) {
        Ok(Some(Kind::Handshake(h))) => h,
        Ok(None) => return exit::CLEAN,
        Ok(Some(other)) => {
            return protocol_error(format!("expected Handshake, got {}", name(&other)));
        }
        Err(e) => return protocol_error(e),
    };
    let version = handshake.version.unwrap_or_default();
    if !probe::accepts(version) {
        let f = Failure::new(
            code::PROTOCOL_UNSUPPORTED,
            format!(
                "the daemon speaks {}.{}, this reader {}.{}",
                version.major,
                version.minor,
                probe::VERSION.major,
                probe::VERSION.minor
            ),
            exit::PROTOCOL,
        );
        diag.line(&f.message);
        failed(&out, 0, f.error());
        return f.exit;
    }
    let attached = match backend.attach(handshake.target_pid) {
        Ok(a) => a,
        Err(f) => {
            diag.line(&format!("attach failed: {} {}", f.code, f.message));
            failed(&out, 0, f.error());
            return f.exit;
        }
    };
    diag.line(&format!(
        "attached to {} (pid {}), engine {}",
        attached.target.image_name, attached.target.pid, attached.engine
    ));
    if send(
        &out,
        Kind::HandshakeAck(HandshakeAck {
            version: Some(probe::VERSION),
            engine: attached.engine,
            probe_build_id: build_id.to_owned(),
            channel: attached.channel.into(),
            target: Some(attached.target),
        }),
    )
    .is_err()
    {
        return exit::CLEAN;
    }

    let ledger = Arc::new(Mutex::new(Ledger::new()));
    let (jobs, queue) = mpsc::channel::<(u64, Scope, Arc<AtomicBool>)>();
    let worker_out = out.clone();
    let worker_ledger = ledger.clone();
    std::thread::spawn(move || {
        for (id, scope, cancel) in queue {
            let answer = if cancel.load(Ordering::SeqCst) {
                Err(Failure::new(
                    code::CANCELLED,
                    "cancelled before it started",
                    0,
                ))
            } else {
                let progress_out = worker_out.clone();
                backend.read(
                    scope,
                    &mut |done, total| {
                        let _ = send(
                            &progress_out,
                            Kind::Progress(Progress {
                                request_id: id,
                                done,
                                total,
                            }),
                        );
                    },
                    &|| cancel.load(Ordering::SeqCst),
                )
            };
            // The ledger records the answer before it is written, so a Cancel that arrives after
            // it is ignored rather than stopping a finished request.
            if let Ok(mut l) = worker_ledger.lock() {
                let _ = l.answer(id);
            }
            let sent = match answer {
                Ok(r) => send(
                    &worker_out,
                    Kind::ReadResult(ReadResult {
                        request_id: id,
                        ..r
                    }),
                ),
                Err(f) => send(
                    &worker_out,
                    Kind::Failed(Failed {
                        request_id: id,
                        error: Some(f.error()),
                    }),
                ),
            };
            if sent.is_err() {
                return;
            }
        }
    });

    let mut flags: HashMap<u64, Arc<AtomicBool>> = HashMap::new();
    loop {
        let kind = match next(&mut incoming, &mut decoder) {
            Ok(Some(k)) => k,
            Ok(None) => {
                diag.line("the daemon closed the pipe");
                return exit::CLEAN;
            }
            Err(e) => return protocol_error(e),
        };
        match kind {
            Kind::ReadRequest(r) => {
                let issued = ledger.lock().map(|mut l| l.issue(r.request_id));
                match issued {
                    Ok(Ok(())) => {}
                    Ok(Err(b)) => return protocol_error(format!("{b:?}")),
                    Err(_) => return exit::INTERNAL,
                }
                let flag = Arc::new(AtomicBool::new(false));
                flags.insert(r.request_id, flag.clone());
                let scope = r.scope();
                if jobs.send((r.request_id, scope, flag)).is_err() {
                    return exit::INTERNAL;
                }
            }
            Kind::Cancel(c) => {
                let effect = ledger.lock().map(|l| l.cancel(c.request_id));
                match effect {
                    Ok(Ok(CancelEffect::Stop)) => {
                        if let Some(f) = flags.get(&c.request_id) {
                            f.store(true, Ordering::SeqCst);
                        }
                    }
                    Ok(Ok(CancelEffect::Ignore)) => {}
                    Ok(Err(b)) => return protocol_error(format!("{b:?}")),
                    Err(_) => return exit::INTERNAL,
                }
            }
            Kind::Shutdown(_) => {
                diag.line("shutdown requested");
                return exit::CLEAN;
            }
            other => return protocol_error(format!("unexpected {}", name(&other))),
        }
    }
}

/// The next message, `None` at a clean end of stream, or the protocol error the stream is.
fn next(incoming: &mut impl Read, decoder: &mut FrameDecoder) -> Result<Option<Kind>, String> {
    let mut buf = [0u8; 16 * 1024];
    loop {
        if let Some(payload) = decoder.next_frame().map_err(|e| format!("{e:?}"))? {
            let m = ProbeMessage::decode(payload.as_slice()).map_err(|e| e.to_string())?;
            return m
                .kind
                .map(Some)
                .ok_or_else(|| "a message with no kind".to_owned());
        }
        let n = match incoming.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => 0,
            Err(e) => return Err(e.to_string()),
        };
        if n == 0 {
            return std::mem::take(decoder)
                .finish()
                .map(|()| None)
                .map_err(|e| format!("{e:?}"));
        }
        decoder.push(&buf[..n]);
    }
}

fn name(kind: &Kind) -> &'static str {
    match kind {
        Kind::Handshake(_) => "Handshake",
        Kind::ReadRequest(_) => "ReadRequest",
        Kind::Cancel(_) => "Cancel",
        Kind::Shutdown(_) => "Shutdown",
        Kind::HandshakeAck(_) => "HandshakeAck",
        Kind::ReadResult(_) => "ReadResult",
        Kind::Progress(_) => "Progress",
        Kind::Failed(_) => "Failed",
        Kind::Log(_) => "Log",
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
