//! The `yata-reader` binary. `pipe` is what the daemon starts; `export` and `processes` are for
//! a user at a terminal. In `pipe` mode nothing is printed: the pipe carries frames only, and
//! diagnostics go to the reader's log file (the main repository's ADR-0006, rule 8).

use std::path::Path;
use std::process::ExitCode;

use yata_protocol::probe::exit;
use yata_reader::backend::DesktopBackend;
use yata_reader::diagnostics::Diagnostics;
use yata_reader::{BUILD_ID, clock, export};

const USAGE: &str = "usage: yata-reader <command>

commands:
  pipe <name>                  serve the daemon on its named pipe (the daemon starts this)
  export <out.json> [--pid <n>]
                               read the souls once and write them to a new export file
  processes                    list the processes the reader would consider as the game

The reader only reads the game, from outside; it never writes to it.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let code = match words.as_slice() {
        ["pipe", name] => pipe(name),
        ["export", out] => export_to(Path::new(out), 0),
        ["export", out, "--pid", pid] => match pid.parse() {
            Ok(pid) => export_to(Path::new(out), pid),
            Err(_) => usage(),
        },
        ["processes"] => processes(),
        _ => usage(),
    };
    ExitCode::from(code)
}

fn usage() -> u8 {
    eprintln!("{USAGE}");
    exit::PROTOCOL
}

#[cfg(windows)]
fn pipe(name: &str) -> u8 {
    use std::sync::{Arc, Mutex};

    use interprocess::os::windows::named_pipe::{DuplexPipeStream, pipe_mode};
    use yata_reader::session::{self, Outgoing};

    let diag = Diagnostics::open_default();
    if !yata_reader::valid_pipe_name(name) {
        diag.line(&format!(
            "refused a pipe name that is not the daemon's: {name:?}"
        ));
        return exit::PROTOCOL;
    }
    let stream = match DuplexPipeStream::<pipe_mode::Bytes>::connect_by_path(name) {
        Ok(s) => s,
        Err(e) => {
            diag.line(&format!("could not connect to the daemon's pipe: {e}"));
            return exit::INTERNAL;
        }
    };
    let (incoming, outgoing) = stream.split();
    let out: Outgoing = Arc::new(Mutex::new(Box::new(outgoing)));
    yata_reader::install_panic_report(out.clone());
    diag.line(&format!("session started, build {BUILD_ID}"));
    let code = session::run(DesktopBackend::default(), incoming, out, BUILD_ID, &diag);
    diag.line(&format!("session ended with exit code {code}"));
    code
}

#[cfg(not(windows))]
fn pipe(_name: &str) -> u8 {
    eprintln!("the reader's pipe is a Windows named pipe; this is not Windows");
    exit::NO_STRATEGY
}

fn export_to(out: &Path, pid: u32) -> u8 {
    let diag = Diagnostics::open_default();
    let mut backend = DesktopBackend::default();
    let mut last = u64::MAX;
    let read = export::read(
        &mut backend,
        pid,
        BUILD_ID,
        &clock::now_rfc3339(),
        &mut |d, t| {
            if d * 10 / t.max(1) != last {
                last = d * 10 / t.max(1);
                eprintln!("reading {}%", 100 * d / t.max(1));
            }
        },
    );
    let e = match read {
        Ok(e) => e,
        Err(f) => {
            diag.line(&format!("export failed: {} {}", f.code, f.message));
            eprintln!("{}: {}", f.code, f.message);
            for c in &f.candidates {
                eprintln!("  candidate: pid {} {}", c.pid, c.image_name);
            }
            if f.code == yata_protocol::probe::code::ELEVATION_REQUIRED {
                eprintln!("run this command from a terminal started as administrator");
            }
            return f.exit;
        }
    };
    match export::write(out, &e) {
        Ok(bytes) => {
            let souls = e
                .results
                .iter()
                .map(|r| match &r.records {
                    Some(yata_protocol::probe::read_result::Records::Souls(s)) => s.souls.len(),
                    None => 0,
                })
                .sum::<usize>();
            eprintln!("wrote {souls} souls, {bytes} bytes, to {}", out.display());
            exit::CLEAN
        }
        Err(err) => {
            eprintln!("could not write {}: {err}", out.display());
            exit::INTERNAL
        }
    }
}

#[cfg(windows)]
fn processes() -> u8 {
    use yata_reader::desktop;
    let all = match desktop::candidates() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}", e.message());
            return exit::INTERNAL;
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
    for c in games {
        let t = desktop::target_of(c);
        println!(
            "pid {:>6}  parent {:>6}  session {}  {}-bit  started {}  {}",
            t.pid,
            t.parent_pid,
            t.session_id,
            t.pointer_bits,
            clock::rfc3339(t.created_unix_ms / 1000),
            t.image_name
        );
    }
    exit::CLEAN
}

#[cfg(not(windows))]
fn processes() -> u8 {
    eprintln!("the desktop channel reads the Windows game; this is not Windows");
    exit::NO_STRATEGY
}
