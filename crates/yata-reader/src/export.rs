//! Export mode (the main repository's ADR-0008): attach, read the souls once, and write one
//! `ProbeExport` as proto3 JSON to the file the user named. No daemon is involved; the file is
//! what a Mac user imports, and what the maintainer keeps as a recording of one reading.

use std::io::Write;
use std::path::Path;

use yata_protocol::export::{self, Unwritable};
use yata_protocol::failure::{Exit, ProbeCode};
use yata_protocol::probe::{self, ProbeExport};

use crate::session::{Backend, ReadScope, RequestFailure, SessionFailure, Target};

/// Why an export was not read.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportFailure {
    /// Attaching failed.
    Attach(SessionFailure),
    /// The read failed. Nothing cancels an export's read and it reads a scope the reader has,
    /// so this is the reader's own fault.
    Read(RequestFailure),
}

impl ExportFailure {
    pub fn code(&self) -> ProbeCode {
        match self {
            ExportFailure::Attach(f) => ProbeCode::Session(f.code()),
            ExportFailure::Read(f) => ProbeCode::Request(f.code),
        }
    }

    pub fn message(&self) -> &str {
        match self {
            ExportFailure::Attach(f) => &f.message,
            ExportFailure::Read(f) => &f.message,
        }
    }

    /// The exit code: never [`Exit::Clean`], since no export was written.
    pub fn exit(&self) -> Exit {
        match self {
            ExportFailure::Attach(f) => f.exit(),
            ExportFailure::Read(_) => Exit::Internal,
        }
    }
}

/// Attach and read the souls into an export.
pub fn read(
    backend: &mut impl Backend,
    target: Target,
    build_id: &str,
    captured_at: &str,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<ProbeExport, ExportFailure> {
    let attached = backend.attach(target).map_err(ExportFailure::Attach)?;
    let reading = backend
        .read(ReadScope::Souls, progress, &|| false)
        .map_err(ExportFailure::Read)?;
    Ok(ProbeExport {
        protocol_version: Some(probe::VERSION),
        probe_build_id: build_id.to_owned(),
        engine: attached.engine,
        channel: attached.channel.into(),
        readings: vec![reading],
        captured_at: Some(captured_at.to_owned()),
        target: Some(attached.target),
    })
}

/// Why an export could not be written.
#[derive(Debug)]
pub enum WriteError {
    Json(Unwritable),
    Io(std::io::Error),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::Json(e) => write!(f, "the export is not writable as JSON: {e:?}"),
            WriteError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// Write the export to a new file: an existing file is never overwritten.
pub fn write(path: &Path, e: &ProbeExport) -> Result<usize, WriteError> {
    let text = export::to_json(e).map_err(WriteError::Json)?;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(WriteError::Io)?;
    f.write_all(text.as_bytes()).map_err(WriteError::Io)?;
    f.flush().map_err(WriteError::Io)?;
    Ok(text.len())
}
