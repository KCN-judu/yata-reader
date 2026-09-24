//! The desktop channel: choosing the game process, opening it with read rights only, and its
//! memory as the parsers see it.
//!
//! Choosing is deterministic and never guesses. With a pid from the daemon, that process is the
//! target, whatever its name. Without one, the processes whose file name is one of
//! [`IMAGE_NAMES`] are the candidates: none is `probe.not_found`, more than one is
//! `probe.ambiguous_target` with every candidate listed, and only exactly one is chosen.
//!
//! Opening classifies the operating system's answer: access denied to an unelevated reader is
//! `probe.elevation_required`, to an elevated one `probe.access_denied`; a pid that is gone is
//! `probe.process_exited`; a 32-bit target is an unsupported environment. The target's memory
//! then goes to [`crate::layout`], which checks its layout before anything is read.

use yata_protocol::probe::{TargetProcess, code, exit};

use crate::layout::cpython::LayoutError;

/// The game's executable names, from the prior tool: a hypothesis, with no recording of this
/// project behind it yet. A name the game does not use finds nothing and is reported as such.
pub const IMAGE_NAMES: [&str; 2] = ["onmyoji.exe", "onmyoji_future.exe"];

/// `ERROR_ACCESS_DENIED` and `ERROR_INVALID_PARAMETER`, as `OpenProcess` reports them.
pub const OS_ACCESS_DENIED: i32 = 5;
pub const OS_INVALID_PARAMETER: i32 = 87;

/// A process discovery considered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub pid: u32,
    pub parent_pid: u32,
    pub image_name: String,
}

/// Why the reader could not attach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachError {
    /// No candidate, or no process with the given pid.
    NotFound { pid: u32 },
    /// More than one candidate.
    Ambiguous { candidates: Vec<Candidate> },
    /// The game refused an unelevated reader.
    ElevationRequired,
    /// The game refused an elevated reader.
    AccessDenied,
    /// The process was gone by the time it was opened.
    Exited,
    /// A host or a target this reader has no strategy for.
    Unsupported { reason: String },
    /// The target's memory is not the layout this reader reads.
    Layout(LayoutError),
    /// Any other operating-system failure.
    Os {
        os_error: i32,
        context: &'static str,
    },
}

impl AttachError {
    pub fn code(&self) -> &'static str {
        match self {
            AttachError::NotFound { .. } => code::NOT_FOUND,
            AttachError::Ambiguous { .. } => code::AMBIGUOUS_TARGET,
            AttachError::ElevationRequired => code::ELEVATION_REQUIRED,
            AttachError::AccessDenied => code::ACCESS_DENIED,
            AttachError::Exited => code::PROCESS_EXITED,
            AttachError::Unsupported { .. } => code::UNSUPPORTED_ENVIRONMENT,
            AttachError::Layout(_) => code::LAYOUT_MISMATCH,
            AttachError::Os { .. } => code::INTERNAL,
        }
    }

    /// The reader's exit code for this failure (`probe-protocol.md`, "Frame").
    pub fn exit(&self) -> u8 {
        match self {
            AttachError::NotFound { .. }
            | AttachError::Ambiguous { .. }
            | AttachError::AccessDenied
            | AttachError::Exited => exit::NOT_ATTACHED,
            AttachError::ElevationRequired => exit::ELEVATION_REQUIRED,
            AttachError::Unsupported { .. } | AttachError::Layout(_) => exit::NO_STRATEGY,
            AttachError::Os { .. } => exit::INTERNAL,
        }
    }

    pub fn message(&self) -> String {
        match self {
            AttachError::NotFound { pid: 0 } => {
                format!("no running process is named {}", IMAGE_NAMES.join(" or "))
            }
            AttachError::NotFound { pid } => format!("no process has pid {pid}"),
            AttachError::Ambiguous { candidates } => {
                format!("{} processes match; choose one by pid", candidates.len())
            }
            AttachError::ElevationRequired => {
                "the game refused access; it runs elevated and the reader does not".to_owned()
            }
            AttachError::AccessDenied => "the game refused access to an elevated reader".to_owned(),
            AttachError::Exited => "the process exited before it could be opened".to_owned(),
            AttachError::Unsupported { reason } => reason.clone(),
            AttachError::Layout(e) => format!("the game's memory is not a known layout: {e:?}"),
            AttachError::Os { os_error, context } => {
                format!("{context} failed with system error {os_error}")
            }
        }
    }

    /// The os error number the failure came from; 0 when there is none.
    pub fn os_error(&self) -> u32 {
        match self {
            AttachError::ElevationRequired | AttachError::AccessDenied => OS_ACCESS_DENIED as u32,
            AttachError::Exited => OS_INVALID_PARAMETER as u32,
            AttachError::Os { os_error, .. } => u32::try_from(*os_error).unwrap_or(0),
            _ => 0,
        }
    }
}

/// Whether a file name is one of the game's.
pub fn is_game(image_name: &str) -> bool {
    IMAGE_NAMES
        .iter()
        .any(|n| n.eq_ignore_ascii_case(image_name))
}

/// Choose the target among all processes: the one with `pid`, or the one game process.
pub fn select(all: &[Candidate], pid: u32) -> Result<Candidate, AttachError> {
    if pid != 0 {
        return all
            .iter()
            .find(|c| c.pid == pid)
            .cloned()
            .ok_or(AttachError::NotFound { pid });
    }
    let mut games: Vec<Candidate> = all
        .iter()
        .filter(|c| is_game(&c.image_name))
        .cloned()
        .collect();
    games.sort_by_key(|c| c.pid);
    match games.len() {
        0 => Err(AttachError::NotFound { pid: 0 }),
        1 => Ok(games.remove(0)),
        _ => Err(AttachError::Ambiguous { candidates: games }),
    }
}

/// What an `OpenProcess` failure means, given whether this reader is elevated.
pub fn classify_open(os_error: i32, elevated: bool) -> AttachError {
    match os_error {
        OS_ACCESS_DENIED if elevated => AttachError::AccessDenied,
        OS_ACCESS_DENIED => AttachError::ElevationRequired,
        OS_INVALID_PARAMETER => AttachError::Exited,
        _ => AttachError::Os {
            os_error,
            context: "opening the game",
        },
    }
}

/// A candidate as the wire reports it, with what the system tells about it.
pub fn target_of(c: &Candidate) -> TargetProcess {
    let t = TargetProcess {
        pid: c.pid,
        image_name: c.image_name.clone(),
        parent_pid: c.parent_pid,
        ..TargetProcess::default()
    };
    #[cfg(windows)]
    {
        let f = crate::platform::windows::facts(c.pid);
        TargetProcess {
            pointer_bits: f.pointer_bits.unwrap_or(0),
            created_unix_ms: f.created_unix_ms.unwrap_or(0),
            session_id: f.session_id.unwrap_or(0),
            ..t
        }
    }
    #[cfg(not(windows))]
    t
}

#[cfg(windows)]
pub use live::{GameMemory, attach, candidates};

#[cfg(windows)]
mod live {
    use super::{AttachError, Candidate, classify_open, select};
    use crate::layout::memory::{Memory, Region, Unreadable};
    use crate::platform::windows::{self, Process};

    /// Every process, as candidates.
    pub fn candidates() -> Result<Vec<Candidate>, AttachError> {
        windows::processes()
            .map_err(|e| AttachError::Os {
                os_error: e.raw_os_error().unwrap_or(0),
                context: "listing processes",
            })
            .map(|list| {
                list.into_iter()
                    .map(|p| Candidate {
                        pid: p.pid,
                        parent_pid: p.parent_pid,
                        image_name: p.image_name,
                    })
                    .collect()
            })
    }

    /// The game process's memory, opened with read rights only.
    pub struct GameMemory {
        process: Process,
        regions: Vec<Region>,
    }

    impl Memory for GameMemory {
        fn regions(&self) -> &[Region] {
            &self.regions
        }

        fn read_into(&self, address: u64, buf: &mut [u8]) -> Result<(), Unreadable> {
            self.process.read(address, buf).map_err(|_| Unreadable {
                address,
                len: buf.len(),
            })
        }
    }

    /// Choose the target and open it.
    pub fn attach(pid: u32) -> Result<(Candidate, GameMemory), AttachError> {
        let chosen = select(&candidates()?, pid)?;
        let facts = windows::facts(chosen.pid);
        if facts.pointer_bits == Some(32) {
            return Err(AttachError::Unsupported {
                reason: format!("{} is a 32-bit process", chosen.image_name),
            });
        }
        let process = windows::open(chosen.pid).map_err(|e| {
            let elevated = windows::is_elevated().unwrap_or(false);
            classify_open(e.raw_os_error().unwrap_or(0), elevated)
        })?;
        let regions = process.regions();
        Ok((chosen, GameMemory { process, regions }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(pid: u32, name: &str) -> Candidate {
        Candidate {
            pid,
            parent_pid: 1,
            image_name: name.into(),
        }
    }

    #[test]
    fn no_game_process_is_not_found() {
        let all = [c(4, "System"), c(10, "explorer.exe")];
        assert_eq!(select(&all, 0), Err(AttachError::NotFound { pid: 0 }));
        assert_eq!(AttachError::NotFound { pid: 0 }.code(), code::NOT_FOUND);
        assert_eq!(AttachError::NotFound { pid: 0 }.exit(), exit::NOT_ATTACHED);
    }

    #[test]
    fn one_game_process_is_chosen_whatever_its_case() {
        let all = [c(10, "explorer.exe"), c(20, "Onmyoji.EXE")];
        assert_eq!(select(&all, 0), Ok(c(20, "Onmyoji.EXE")));
    }

    #[test]
    fn two_game_processes_are_ambiguous_and_listed_by_pid() {
        let all = [
            c(30, "onmyoji_future.exe"),
            c(10, "x.exe"),
            c(20, "onmyoji.exe"),
        ];
        assert_eq!(
            select(&all, 0),
            Err(AttachError::Ambiguous {
                candidates: vec![c(20, "onmyoji.exe"), c(30, "onmyoji_future.exe")]
            })
        );
    }

    #[test]
    fn a_given_pid_chooses_that_process_or_is_not_found() {
        let all = [
            c(20, "onmyoji.exe"),
            c(30, "onmyoji.exe"),
            c(40, "renamed.exe"),
        ];
        assert_eq!(select(&all, 30), Ok(c(30, "onmyoji.exe")));
        assert_eq!(select(&all, 40), Ok(c(40, "renamed.exe")));
        assert_eq!(select(&all, 99), Err(AttachError::NotFound { pid: 99 }));
    }

    #[test]
    fn access_denied_means_elevation_only_when_not_elevated() {
        let unelevated = classify_open(OS_ACCESS_DENIED, false);
        assert_eq!(unelevated, AttachError::ElevationRequired);
        assert_eq!(unelevated.code(), code::ELEVATION_REQUIRED);
        assert_eq!(unelevated.exit(), exit::ELEVATION_REQUIRED);
        let elevated = classify_open(OS_ACCESS_DENIED, true);
        assert_eq!(elevated, AttachError::AccessDenied);
        assert_eq!(elevated.code(), code::ACCESS_DENIED);
        assert_eq!(elevated.exit(), exit::NOT_ATTACHED);
    }

    #[test]
    fn a_vanished_process_and_other_errors_are_told_apart() {
        assert_eq!(
            classify_open(OS_INVALID_PARAMETER, false),
            AttachError::Exited
        );
        let other = classify_open(1450, false);
        assert_eq!(other.code(), code::INTERNAL);
        assert_eq!(other.os_error(), 1450);
    }

    #[test]
    fn a_layout_mismatch_has_no_read_strategy() {
        let e = AttachError::Layout(LayoutError::DictKeys);
        assert_eq!(e.code(), code::LAYOUT_MISMATCH);
        assert_eq!(e.exit(), exit::NO_STRATEGY);
    }

    #[cfg(windows)]
    #[test]
    fn a_process_that_refuses_reading_is_classified() {
        // csrss.exe is a protected process: it refuses reading rights to every user process,
        // elevated or not, which is the same answer an elevated game gives an unelevated reader.
        let all = candidates().expect("listed");
        let csrss = all
            .iter()
            .find(|p| p.image_name.eq_ignore_ascii_case("csrss.exe"))
            .expect("csrss runs on every Windows");
        let e = crate::platform::windows::open(csrss.pid)
            .err()
            .expect("refused");
        assert_eq!(e.raw_os_error(), Some(OS_ACCESS_DENIED));
        let elevated = crate::platform::windows::is_elevated().expect("known");
        let expected = if elevated {
            AttachError::AccessDenied
        } else {
            AttachError::ElevationRequired
        };
        assert_eq!(classify_open(OS_ACCESS_DENIED, elevated), expected);
    }

    #[cfg(windows)]
    #[test]
    fn a_pid_that_does_not_exist_is_not_found() {
        // Process ids are multiples of four, so an odd one never exists.
        assert_eq!(
            attach(4_000_000_001).err(),
            Some(AttachError::NotFound { pid: 4_000_000_001 })
        );
    }

    #[cfg(windows)]
    #[test]
    fn no_game_is_running_on_a_test_machine() {
        let all = candidates().expect("listed");
        if all.iter().any(|p| is_game(&p.image_name)) {
            return;
        }
        assert_eq!(attach(0).err(), Some(AttachError::NotFound { pid: 0 }));
    }
}
