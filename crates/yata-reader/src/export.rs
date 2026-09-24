//! Export mode (the main repository's ADR-0008): attach, read the souls once, and write one
//! `ProbeExport` as proto3 JSON to the file the user named. No daemon is involved; the file is
//! what a Mac user imports, and what the maintainer keeps as a recording of one reading.

use std::io::Write;
use std::path::Path;

use yata_protocol::export;
use yata_protocol::probe::{self, ProbeExport, Scope};

use crate::session::{Backend, Failure};

/// Attach and read the souls into an export.
pub fn read(
    backend: &mut impl Backend,
    target_pid: u32,
    build_id: &str,
    captured_at: &str,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<ProbeExport, Failure> {
    let attached = backend.attach(target_pid)?;
    let result = backend.read(Scope::Souls, progress, &|| false)?;
    Ok(ProbeExport {
        protocol_version: Some(probe::VERSION),
        probe_build_id: build_id.to_owned(),
        engine: attached.engine,
        channel: attached.channel.into(),
        results: vec![result],
        captured_at: captured_at.to_owned(),
        target: Some(attached.target),
    })
}

/// Write the export to a new file: an existing file is never overwritten.
pub fn write(path: &Path, e: &ProbeExport) -> std::io::Result<usize> {
    let text = export::to_json(e);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(text.as_bytes())?;
    f.flush()?;
    Ok(text.len())
}
