//! The reader's side of the probe protocol (the main repository's `probe-protocol.md`): the
//! handshake, request dispatch, cancellation, and shutdown, over any byte stream.
//!
//! The main thread reads the daemon's frames; one worker thread answers read requests in order,
//! so a `Cancel` is seen while a read runs. Only the worker writes an answer, and it writes
//! exactly one per request, through the request discipline shared with the daemon. A request
//! cancelled before it starts is answered `probe.cancelled` without being read.
//!
//! Two kinds of failure are told apart by type. A [`SessionFailure`] ends the session: the daemon
//! gets a session-level `Failed`, and the reader exits with the failure's exit code. A
//! [`RequestFailure`] answers one request, and the session goes on.
//!
//! The session ends when the daemon closes the stream or sends `Shutdown` (exit 0), when the
//! daemon breaks the protocol (`probe.protocol_error`, exit 3), when attaching fails (the reason,
//! and the reason's exit code), or when the reader's own bookkeeping fails (`probe.internal`,
//! exit 4).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use prost::Message;
use yata_protocol::discipline::{Breach, CancelEffect, Ledger, RequestId};
use yata_protocol::frame::{self, FrameDecoder, FrameError};
use yata_protocol::probe::{
    self, Channel, Discovery, Exit, Failed, HandshakeAck, ProbeError, ProbeErrorCode, ProbeMessage,
    Progress, ReadResult, Reading, Scope, SessionLevel, TargetProcess, failed::Subject, handshake,
    probe_error::Detail, probe_message::Kind,
};

use crate::diagnostics::Diagnostics;

/// Which process to attach to, as the daemon's handshake names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The one process whose file name is the game's.
    Discover,
    /// The process with this id, whatever its name.
    Pid(NonZeroU32),
}

impl Target {
    /// The target a handshake names, or why it names none.
    pub fn of(t: Option<&handshake::Target>) -> Result<Target, &'static str> {
        match t {
            None => Err("the handshake names no target"),
            Some(handshake::Target::Discover(_)) => Ok(Target::Discover),
            Some(handshake::Target::Pid(pid)) => NonZeroU32::new(*pid)
                .map(Target::Pid)
                .ok_or("the handshake names pid 0"),
        }
    }

    /// The handshake field for this target.
    pub fn wire(self) -> handshake::Target {
        match self {
            Target::Discover => handshake::Target::Discover(probe::Discover {}),
            Target::Pid(pid) => handshake::Target::Pid(pid.get()),
        }
    }
}

/// What attaching found.
#[derive(Debug, Clone, PartialEq)]
pub struct Attached {
    pub engine: String,
    pub channel: Channel,
    pub target: TargetProcess,
}

/// Why a session ends. Each reason has the code the daemon is told and the reader's exit code.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionReason {
    NotFound,
    /// More than one process could be the game; the daemon is told which.
    Ambiguous {
        candidates: Vec<TargetProcess>,
    },
    ElevationRequired,
    AccessDenied,
    ProcessExited,
    UnsupportedEnvironment,
    LayoutMismatch,
    ProtocolUnsupported,
    ProtocolError,
    Internal,
}

impl SessionReason {
    pub fn code(&self) -> ProbeErrorCode {
        match self {
            SessionReason::NotFound => ProbeErrorCode::NotFound,
            SessionReason::Ambiguous { .. } => ProbeErrorCode::AmbiguousTarget,
            SessionReason::ElevationRequired => ProbeErrorCode::ElevationRequired,
            SessionReason::AccessDenied => ProbeErrorCode::AccessDenied,
            SessionReason::ProcessExited => ProbeErrorCode::ProcessExited,
            SessionReason::UnsupportedEnvironment => ProbeErrorCode::UnsupportedEnvironment,
            SessionReason::LayoutMismatch => ProbeErrorCode::LayoutMismatch,
            SessionReason::ProtocolUnsupported => ProbeErrorCode::ProtocolUnsupported,
            SessionReason::ProtocolError => ProbeErrorCode::ProtocolError,
            SessionReason::Internal => ProbeErrorCode::Internal,
        }
    }

    /// The reader's exit code after this failure (`probe-protocol.md`, "Error codes").
    pub fn exit(&self) -> Exit {
        match self {
            SessionReason::NotFound
            | SessionReason::Ambiguous { .. }
            | SessionReason::AccessDenied
            | SessionReason::ProcessExited => Exit::NotAttached,
            SessionReason::ElevationRequired => Exit::ElevationRequired,
            SessionReason::UnsupportedEnvironment | SessionReason::LayoutMismatch => {
                Exit::NoStrategy
            }
            SessionReason::ProtocolUnsupported | SessionReason::ProtocolError => Exit::Protocol,
            SessionReason::Internal => Exit::Internal,
        }
    }
}

/// A failure that ends the session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionFailure {
    pub reason: SessionReason,
    pub message: String,
    /// The operating-system error the failure came from, when there is one.
    pub os_error: Option<NonZeroU32>,
}

impl SessionFailure {
    pub fn new(reason: SessionReason, message: impl Into<String>) -> SessionFailure {
        SessionFailure {
            reason,
            message: message.into(),
            os_error: None,
        }
    }

    pub fn exit(&self) -> Exit {
        self.reason.exit()
    }

    pub fn error(&self) -> ProbeError {
        ProbeError {
            code: self.reason.code().into(),
            message: self.message.clone(),
            os_error: self.os_error.map(NonZeroU32::get),
            detail: match &self.reason {
                SessionReason::Ambiguous { candidates } => Some(Detail::Discovery(Discovery {
                    candidates: candidates.clone(),
                })),
                _ => None,
            },
        }
    }
}

/// Why one request is answered with a failure while the session goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestReason {
    Cancelled,
    ScopeUnsupported,
    NotAttached,
}

impl RequestReason {
    pub fn code(self) -> ProbeErrorCode {
        match self {
            RequestReason::Cancelled => ProbeErrorCode::Cancelled,
            RequestReason::ScopeUnsupported => ProbeErrorCode::ScopeUnsupported,
            RequestReason::NotAttached => ProbeErrorCode::NotAttached,
        }
    }
}

/// A failure that answers one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestFailure {
    pub reason: RequestReason,
    pub message: String,
}

impl RequestFailure {
    pub fn new(reason: RequestReason, message: impl Into<String>) -> RequestFailure {
        RequestFailure {
            reason,
            message: message.into(),
        }
    }

    pub fn error(&self) -> ProbeError {
        ProbeError {
            code: self.reason.code().into(),
            message: self.message.clone(),
            os_error: None,
            detail: None,
        }
    }
}

/// How the reader reaches the game: the desktop channel, or synthetic memory in tests.
pub trait Backend: Send + 'static {
    fn attach(&mut self, target: Target) -> Result<Attached, SessionFailure>;

    /// Read a scope. `progress` gets `(done, total)`; `cancelled` is checked at checkpoints.
    fn read(
        &mut self,
        scope: Scope,
        progress: &mut dyn FnMut(u64, u64),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Reading, RequestFailure>;
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

/// Tell the daemon the session is over, and why.
pub fn send_session_failure(out: &Outgoing, f: &SessionFailure) {
    let _ = send(
        out,
        Kind::Failed(Failed {
            subject: Some(Subject::Session(SessionLevel {})),
            error: Some(f.error()),
        }),
    );
}

/// Every open request's cancel flag, beside the ledger that says which requests are open. One
/// lock holds both, so they never disagree, and a flag leaves with its request's answer.
#[derive(Default)]
struct Requests {
    ledger: Ledger,
    cancels: HashMap<RequestId, Arc<AtomicBool>>,
}

/// Run a session until it ends; the result is the reader's exit.
pub fn run(
    mut backend: impl Backend,
    mut incoming: impl Read,
    out: Outgoing,
    build_id: &str,
    diag: &Diagnostics,
) -> Exit {
    let mut decoder = FrameDecoder::new();
    let end = |f: SessionFailure| {
        diag.line(&format!(
            "session failed: {} {}",
            f.reason.code().name().unwrap_or("?"),
            f.message
        ));
        send_session_failure(&out, &f);
        f.exit()
    };
    let protocol_error =
        |message: String| SessionFailure::new(SessionReason::ProtocolError, message);
    let handshake = match next(&mut incoming, &mut decoder) {
        Ok(Some(Kind::Handshake(h))) => h,
        Ok(None) => return Exit::Clean,
        Ok(Some(other)) => {
            return end(protocol_error(format!(
                "expected Handshake, got {}",
                name(&other)
            )));
        }
        Err(e) => return end(protocol_error(e.to_string())),
    };
    let Some(version) = handshake.version else {
        return end(protocol_error("the handshake has no version".to_owned()));
    };
    if !probe::accepts(version) {
        return end(SessionFailure::new(
            SessionReason::ProtocolUnsupported,
            format!(
                "the daemon speaks {}.{}, this reader {}.{}",
                version.major,
                version.minor,
                probe::VERSION.major,
                probe::VERSION.minor
            ),
        ));
    }
    let target = match Target::of(handshake.target.as_ref()) {
        Ok(t) => t,
        Err(e) => return end(protocol_error(e.to_owned())),
    };
    let attached = match backend.attach(target) {
        Ok(a) => a,
        Err(f) => return end(f),
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
        return Exit::Clean;
    }

    let requests = Arc::new(Mutex::new(Requests::default()));
    // Set once, by the worker, when its own bookkeeping fails: the exit the session then ends
    // with, whenever the main thread next looks.
    let broken: Arc<OnceLock<Exit>> = Arc::new(OnceLock::new());
    let (jobs, queue) = mpsc::channel::<(RequestId, Scope, Arc<AtomicBool>)>();
    let worker = Worker {
        out: out.clone(),
        requests: requests.clone(),
        broken: broken.clone(),
    };
    std::thread::spawn(move || worker.serve(&mut backend, queue));

    let internal = |message: &str| end(SessionFailure::new(SessionReason::Internal, message));
    loop {
        let kind = next(&mut incoming, &mut decoder);
        if let Some(&exit) = broken.get() {
            return exit;
        }
        let kind = match kind {
            Ok(Some(k)) => k,
            Ok(None) => {
                diag.line("the daemon closed the pipe");
                return Exit::Clean;
            }
            Err(e) => return end(protocol_error(e.to_string())),
        };
        let breach = |b: Breach| end(protocol_error(format!("{b:?}")));
        match kind {
            Kind::ReadRequest(r) => {
                let id = match RequestId::new(r.request_id) {
                    Ok(id) => id,
                    Err(b) => return breach(b),
                };
                let flag = Arc::new(AtomicBool::new(false));
                let Ok(mut state) = requests.lock() else {
                    return internal("the request state is poisoned");
                };
                if let Err(b) = state.ledger.issue(id) {
                    return breach(b);
                }
                state.cancels.insert(id, flag.clone());
                drop(state);
                if jobs.send((id, r.scope(), flag)).is_err() {
                    return internal("the worker has stopped");
                }
            }
            Kind::Cancel(c) => {
                let id = match RequestId::new(c.request_id) {
                    Ok(id) => id,
                    Err(b) => return breach(b),
                };
                let Ok(state) = requests.lock() else {
                    return internal("the request state is poisoned");
                };
                match state.ledger.cancel(id) {
                    Ok(CancelEffect::Stop) => {
                        if let Some(f) = state.cancels.get(&id) {
                            f.store(true, Ordering::SeqCst);
                        }
                    }
                    Ok(CancelEffect::Ignore) => {}
                    Err(b) => return breach(b),
                }
            }
            Kind::Shutdown(_) => {
                diag.line("shutdown requested");
                return Exit::Clean;
            }
            other => return end(protocol_error(format!("unexpected {}", name(&other)))),
        }
    }
}

/// The thread that answers read requests, one at a time, in order.
struct Worker {
    out: Outgoing,
    requests: Arc<Mutex<Requests>>,
    broken: Arc<OnceLock<Exit>>,
}

impl Worker {
    fn serve(
        self,
        backend: &mut impl Backend,
        queue: mpsc::Receiver<(RequestId, Scope, Arc<AtomicBool>)>,
    ) {
        for (id, scope, cancel) in queue {
            let answer = if cancel.load(Ordering::SeqCst) {
                Err(RequestFailure::new(
                    RequestReason::Cancelled,
                    "cancelled before it started",
                ))
            } else {
                backend.read(
                    scope,
                    &mut |done, total| {
                        let _ = send(
                            &self.out,
                            Kind::Progress(Progress {
                                request_id: id.get(),
                                done,
                                total: Some(total),
                            }),
                        );
                    },
                    &|| cancel.load(Ordering::SeqCst),
                )
            };
            // The ledger records the answer before it is written, so a Cancel that arrives after
            // it is ignored rather than stopping a finished request.
            if let Err(message) = self.settle(id) {
                let f = SessionFailure::new(SessionReason::Internal, message);
                let _ = self.broken.set(f.exit());
                send_session_failure(&self.out, &f);
                return;
            }
            let message = match answer {
                Ok(reading) => Kind::ReadResult(ReadResult {
                    request_id: id.get(),
                    reading: Some(reading),
                }),
                Err(f) => Kind::Failed(Failed {
                    subject: Some(Subject::RequestId(id.get())),
                    error: Some(f.error()),
                }),
            };
            if send(&self.out, message).is_err() {
                return;
            }
        }
    }

    /// Record a request's answer and drop its cancel flag.
    fn settle(&self, id: RequestId) -> Result<(), String> {
        let mut state = self
            .requests
            .lock()
            .map_err(|_| "the request state is poisoned".to_owned())?;
        state
            .ledger
            .answer(id)
            .map_err(|b| format!("answering request {}: {b:?}", id.get()))?;
        state.cancels.remove(&id);
        Ok(())
    }
}

/// Why the daemon's stream could not be read as messages.
#[derive(Debug)]
pub enum StreamError {
    Frame(FrameError),
    Decode(prost::DecodeError),
    NoKind,
    Io(std::io::Error),
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamError::Frame(e) => write!(f, "a malformed frame: {e:?}"),
            StreamError::Decode(e) => write!(f, "a frame that is not a message: {e}"),
            StreamError::NoKind => write!(f, "a message with no kind"),
            StreamError::Io(e) => write!(f, "the stream failed: {e}"),
        }
    }
}

/// The next message, or `None` at a clean end of stream.
fn next(incoming: &mut impl Read, decoder: &mut FrameDecoder) -> Result<Option<Kind>, StreamError> {
    let mut buf = [0u8; 16 * 1024];
    loop {
        if let Some(payload) = decoder.next_frame().map_err(StreamError::Frame)? {
            let m = ProbeMessage::decode(payload.as_slice()).map_err(StreamError::Decode)?;
            return m.kind.map(Some).ok_or(StreamError::NoKind);
        }
        let n = match incoming.read(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => 0,
            Err(e) => return Err(StreamError::Io(e)),
        };
        if n == 0 {
            return std::mem::take(decoder)
                .finish()
                .map(|()| None)
                .map_err(StreamError::Frame);
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
