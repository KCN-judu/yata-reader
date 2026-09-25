//! The `yata-reader` binary. `pipe` is what the daemon starts; `export` and `processes` are for
//! a user at a terminal. In `pipe` mode nothing is printed: the pipe carries frames only, and
//! diagnostics go to the reader's log file (the main repository's ADR-0006, rule 8).

use std::num::NonZeroU32;
use std::path::Path;
use std::process::ExitCode;

use yata_protocol::failure::{Exit, ProbeCode, SessionCode};
use yata_reader::backend::DesktopBackend;
use yata_reader::diagnostics::Diagnostics;
use yata_reader::export::ExportFailure;
use yata_reader::session::{SessionFailure, SessionReason, Target};
use yata_reader::{BUILD_ID, clock, export};

const USAGE: &str = "usage: yata-reader <command>

commands:
  pipe <name>                  serve the daemon on its named pipe (the daemon starts this)
  export <out.json> [--pid <n>]
                               read the souls once and write them to a new export file
  processes                    list the processes the reader would consider as the game

The reader only reads the game, from outside; it never writes to it.";

fn main() -> ExitCode {
    let args: Option<Vec<String>> = std::env::args_os()
        .skip(1)
        .map(|a| a.into_string().ok())
        .collect();
    let Some(args) = args else {
        eprintln!("an argument is not Unicode");
        return exit(Exit::Protocol);
    };
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let code = match words.as_slice() {
        ["pipe", name] => pipe(name),
        ["export", out] => export_to(Path::new(out), Target::Discover),
        ["export", out, "--pid", pid] => match pid.parse::<NonZeroU32>() {
            Ok(pid) => export_to(Path::new(out), Target::Pid(pid)),
            Err(_) => usage(),
        },
        ["processes"] => processes(),
        _ => usage(),
    };
    exit(code)
}

fn exit(e: Exit) -> ExitCode {
    ExitCode::from(e.code())
}

fn usage() -> Exit {
    eprintln!("{USAGE}");
    Exit::Protocol
}

#[cfg(windows)]
fn pipe(name: &str) -> Exit {
    use std::sync::{Arc, Mutex};

    use interprocess::os::windows::named_pipe::{DuplexPipeStream, pipe_mode};
    use yata_reader::session::{self, Outgoing};

    let diag = Diagnostics::open_default();
    let Some(name) = yata_reader::PipeName::parse(name) else {
        diag.line(&format!(
            "refused a pipe name that is not the daemon's: {name:?}"
        ));
        return Exit::Protocol;
    };
    let stream = match DuplexPipeStream::<pipe_mode::Bytes>::connect_by_path(name.as_str()) {
        Ok(s) => s,
        Err(e) => {
            diag.line(&format!("could not connect to the daemon's pipe: {e}"));
            return Exit::Internal;
        }
    };
    let (incoming, outgoing) = stream.split();
    let out: Outgoing = Arc::new(Mutex::new(Box::new(outgoing)));
    yata_reader::install_panic_report(out.clone());
    diag.line(&format!("session started, build {BUILD_ID}"));
    let code = session::run(DesktopBackend::default(), incoming, out, BUILD_ID, &diag);
    diag.line(&format!("session ended with exit {code:?}"));
    code
}

#[cfg(not(windows))]
fn pipe(_name: &str) -> Exit {
    eprintln!("the reader's pipe is a Windows named pipe; this is not Windows");
    Exit::NoStrategy
}

fn export_to(out: &Path, target: Target) -> Exit {
    let diag = Diagnostics::open_default();
    let mut backend = DesktopBackend::default();
    let mut last: Option<u64> = None;
    let read = export::read(
        &mut backend,
        target,
        BUILD_ID,
        &clock::now_rfc3339(),
        &mut |d, t| {
            let tenth = d * 10 / t.max(1);
            if last != Some(tenth) {
                last = Some(tenth);
                eprintln!("reading {}%", 100 * d / t.max(1));
            }
        },
    );
    let e = match read {
        Ok(e) => e,
        Err(f) => {
            let name = f.code().name();
            diag.line(&format!("export failed: {name} {}", f.message()));
            eprintln!("{name}: {}", f.message());
            if let ExportFailure::Attach(SessionFailure {
                reason: SessionReason::Ambiguous { candidates },
                ..
            }) = &f
            {
                for c in candidates {
                    eprintln!("  candidate: pid {} {}", c.pid, c.image_name);
                }
            }
            if f.code() == ProbeCode::Session(SessionCode::ElevationRequired) {
                eprintln!("run this command from a terminal started as administrator");
            }
            return f.exit();
        }
    };
    match export::write(out, &e) {
        Ok(bytes) => {
            let souls = e
                .readings
                .iter()
                .map(|r| match &r.records {
                    Some(yata_protocol::probe::reading::Records::Souls(s)) => s.souls.len(),
                    None => 0,
                })
                .sum::<usize>();
            eprintln!("wrote {souls} souls, {bytes} bytes, to {}", out.display());
            Exit::Clean
        }
        Err(err) => {
            eprintln!("could not write {}: {err}", out.display());
            Exit::Internal
        }
    }
}

#[cfg(windows)]
fn processes() -> Exit {
    use yata_protocol::probe::PointerWidth;
    use yata_reader::desktop;

    let all = match desktop::candidates() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}", e.message());
            return Exit::Internal;
        }
    };
    let games: Vec<_> = all
        .iter()
        .filter(|c| desktop::is_game(&c.image_name))
        .collect();
    if games.is_empty() {
        println!(
            "no running process is named {}",
            desktop::IMAGE_NAMES.join(" or ")
        );
    }
    let unknown = || "?".to_owned();
    for c in games {
        let t = desktop::target_of(c);
        let width = match t.pointer_width.and_then(|w| PointerWidth::try_from(w).ok()) {
            Some(PointerWidth::PointerWidth32) => "32-bit",
            Some(PointerWidth::PointerWidth64) => "64-bit",
            Some(PointerWidth::Unspecified) | None => "width unknown",
        };
        println!(
            "pid {:>6}  parent {:>6}  session {}  {width}  started {}  {}",
            t.pid,
            t.parent_pid.map_or_else(unknown, |p| p.to_string()),
            t.session_id.map_or_else(unknown, |s| s.to_string()),
            t.created_unix_ms
                .map_or_else(unknown, |ms| clock::rfc3339(ms / 1000)),
            t.image_name
        );
    }
    Exit::Clean
}

#[cfg(not(windows))]
fn processes() -> Exit {
    eprintln!("the desktop channel reads the Windows game; this is not Windows");
    Exit::NoStrategy
}
