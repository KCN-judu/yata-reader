//! The desktop channel: choosing the game process, opening it with read rights only, and its
//! memory as the parsers see it.
//!
//! Choosing is deterministic and never guesses. With a pid from the daemon, that process is the
//! target, whatever its name. Without one, the processes whose file name is one of
//! [`IMAGE_NAMES`] are the candidates: none is `probe.not_found`, more than one is
//! `probe.ambiguous_target` with every candidate listed, and only exactly one is chosen.
//!
//! Opening classifies the operating system's answer: access denied to a reader known to be
//! unelevated is `probe.elevation_required`, to any other reader `probe.access_denied`; a pid that
//! is gone is `probe.process_exited`; a 32-bit target is an unsupported environment. The target's
//! memory then goes to [`crate::layout`], which checks its layout before anything is read.

use std::num::NonZeroU32;

use yata_protocol::probe::{PointerWidth, TargetProcess};

use crate::session::{SessionFailure, SessionReason, Target};

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

/// Whether this reader runs elevated, as far as the system says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elevation {
    Elevated,
    Unelevated,
    /// The system would not say.
    Unknown,
}

/// Why the reader could not attach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachError {
    /// No process has one of the game's names.
    NoGame,
    /// No process has the pid the daemon named.
    NoPid { pid: NonZeroU32 },
    /// More than one candidate.
    Ambiguous { candidates: Vec<Candidate> },
    /// The game refused a reader known to be unelevated.
    ElevationRequired,
    /// The game refused an elevated reader.
    AccessDenied,
    /// The game refused a reader that cannot tell whether it is elevated. Elevating is not
    /// suggested: it may not be the cause.
    DeniedUnknownElevation,
    /// The process was gone by the time it was opened.
    Exited,
    /// A host or a target this reader has no strategy for.
    Unsupported { reason: String },
    /// Any other operating-system failure, with the system's error number when it gave one.
    Os {
        os_error: Option<i32>,
        context: &'static str,
    },
}

impl AttachError {
    pub fn message(&self) -> String {
        match self {
            AttachError::NoGame => {
                format!("no running process is named {}", IMAGE_NAMES.join(" or "))
            }
            AttachError::NoPid { pid } => format!("no process has pid {pid}"),
            AttachError::Ambiguous { candidates } => {
                format!("{} processes match; choose one by pid", candidates.len())
            }
            AttachError::ElevationRequired => {
                "the game refused access; it runs elevated and the reader does not".to_owned()
            }
            AttachError::AccessDenied => "the game refused access to an elevated reader".to_owned(),
            AttachError::DeniedUnknownElevation => "the game refused access, and the system would \
                                                    not say whether this reader is elevated"
                .to_owned(),
            AttachError::Exited => "the process exited before it could be opened".to_owned(),
            AttachError::Unsupported { reason } => reason.clone(),
            AttachError::Os {
                os_error: Some(e),
                context,
            } => format!("{context} failed with system error {e}"),
            AttachError::Os {
                os_error: None,
                context,
            } => format!("{context} failed, and the system gave no error number"),
        }
    }

    /// The operating-system error the failure came from, when there is one.
    pub fn os_error(&self) -> Option<NonZeroU32> {
        let raw = match self {
            AttachError::ElevationRequired
            | AttachError::AccessDenied
            | AttachError::DeniedUnknownElevation => Some(OS_ACCESS_DENIED),
            AttachError::Exited => Some(OS_INVALID_PARAMETER),
            AttachError::Os { os_error, .. } => *os_error,
            AttachError::NoGame
            | AttachError::NoPid { .. }
            | AttachError::Ambiguous { .. }
            | AttachError::Unsupported { .. } => None,
        };
        raw.and_then(|e| u32::try_from(e).ok())
            .and_then(NonZeroU32::new)
    }

    /// The failure the daemon is told about, which ends the session.
    pub fn failure(&self) -> SessionFailure {
        let reason = match self {
            AttachError::NoGame | AttachError::NoPid { .. } => SessionReason::NotFound,
            AttachError::Ambiguous { candidates } => SessionReason::Ambiguous {
                candidates: candidates.iter().map(target_of).collect(),
            },
            AttachError::ElevationRequired => SessionReason::ElevationRequired,
            AttachError::AccessDenied | AttachError::DeniedUnknownElevation => {
                SessionReason::AccessDenied
            }
            AttachError::Exited => SessionReason::ProcessExited,
            AttachError::Unsupported { .. } => SessionReason::UnsupportedEnvironment,
            AttachError::Os { .. } => SessionReason::Internal,
        };
        SessionFailure {
            os_error: self.os_error(),
            ..SessionFailure::new(reason, self.message())
        }
    }
}

/// Whether a file name is one of the game's.
pub fn is_game(image_name: &str) -> bool {
    IMAGE_NAMES
        .iter()
        .any(|n| n.eq_ignore_ascii_case(image_name))
}

/// Choose the target among all processes: the one with the pid, or the one game process.
pub fn select(all: &[Candidate], target: Target) -> Result<Candidate, AttachError> {
    if let Target::Pid(pid) = target {
        return all
            .iter()
            .find(|c| c.pid == pid.get())
            .cloned()
            .ok_or(AttachError::NoPid { pid });
    }
    let mut games: Vec<Candidate> = all
        .iter()
        .filter(|c| is_game(&c.image_name))
        .cloned()
        .collect();
    games.sort_by_key(|c| c.pid);
    match games.as_slice() {
        [] => Err(AttachError::NoGame),
        [one] => Ok(one.clone()),
        _ => Err(AttachError::Ambiguous { candidates: games }),
    }
}

/// What an `OpenProcess` failure means, given what is known of this reader's elevation.
pub fn classify_open(os_error: Option<i32>, elevation: Elevation) -> AttachError {
    match (os_error, elevation) {
        (Some(OS_ACCESS_DENIED), Elevation::Unelevated) => AttachError::ElevationRequired,
        (Some(OS_ACCESS_DENIED), Elevation::Elevated) => AttachError::AccessDenied,
        (Some(OS_ACCESS_DENIED), Elevation::Unknown) => AttachError::DeniedUnknownElevation,
        (Some(OS_INVALID_PARAMETER), _) => AttachError::Exited,
        (os_error, _) => AttachError::Os {
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
        parent_pid: Some(c.parent_pid),
        ..TargetProcess::default()
    };
    #[cfg(windows)]
    {
        use crate::platform::windows::{self, Bitness};
        let f = windows::facts(c.pid);
        TargetProcess {
            pointer_width: f.bitness.map(|b| {
                match b {
                    Bitness::Bits32 => PointerWidth::PointerWidth32,
                    Bitness::Bits64 => PointerWidth::PointerWidth64,
                }
                .into()
            }),
            created_unix_ms: f.created_unix_ms,
            session_id: f.session_id,
            ..t
        }
    }
    #[cfg(not(windows))]
    {
        let _: Option<PointerWidth> = None;
        t
    }
}

#[cfg(windows)]
pub use live::{GameMemory, attach, candidates};

#[cfg(windows)]
mod live {
    use super::{AttachError, Candidate, Elevation, classify_open, select};
    use crate::layout::memory::{Memory, Region, Unreadable};
    use crate::platform::windows::{self, Bitness, Process};
    use crate::session::Target;

    /// Every process, as candidates.
    pub fn candidates() -> Result<Vec<Candidate>, AttachError> {
        windows::processes()
            .map_err(|e| AttachError::Os {
                os_error: e.raw_os_error(),
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
    pub fn attach(target: Target) -> Result<(Candidate, GameMemory), AttachError> {
        let chosen = select(&candidates()?, target)?;
        if windows::facts(chosen.pid).bitness == Some(Bitness::Bits32) {
            return Err(AttachError::Unsupported {
                reason: format!("{} is a 32-bit process", chosen.image_name),
            });
        }
        let process = windows::open(chosen.pid).map_err(|e| {
            let elevation = match windows::is_elevated() {
                Ok(true) => Elevation::Elevated,
                Ok(false) => Elevation::Unelevated,
                Err(_) => Elevation::Unknown,
            };
            classify_open(e.raw_os_error(), elevation)
        })?;
        let regions = process.regions();
        Ok((chosen, GameMemory { process, regions }))
    }
}

#[cfg(test)]
mod tests {
    use yata_protocol::failure::{Exit, SessionCode};

    use super::*;

    fn c(pid: u32, name: &str) -> Candidate {
        Candidate {
            pid,
            parent_pid: 1,
            image_name: name.into(),
        }
    }

    fn nonzero(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).expect("nonzero")
    }

    #[test]
    fn no_game_process_is_not_found() {
        let all = [c(4, "System"), c(10, "explorer.exe")];
        assert_eq!(select(&all, Target::Discover), Err(AttachError::NoGame));
        let f = AttachError::NoGame.failure();
        assert_eq!(f.code(), SessionCode::NotFound);
        assert_eq!(f.exit(), Exit::NotAttached);
        assert_eq!(f.os_error, None);
    }

    #[test]
    fn one_game_process_is_chosen_whatever_its_case() {
        let all = [c(10, "explorer.exe"), c(20, "Onmyoji.EXE")];
        assert_eq!(select(&all, Target::Discover), Ok(c(20, "Onmyoji.EXE")));
    }

    #[test]
    fn two_game_processes_are_ambiguous_and_listed_by_pid() {
        let all = [
            c(30, "onmyoji_future.exe"),
            c(10, "x.exe"),
            c(20, "onmyoji.exe"),
        ];
        let e = select(&all, Target::Discover).expect_err("ambiguous");
        assert_eq!(
            e,
            AttachError::Ambiguous {
                candidates: vec![c(20, "onmyoji.exe"), c(30, "onmyoji_future.exe")]
            }
        );
        let SessionReason::Ambiguous { candidates } = e.failure().reason else {
            panic!("ambiguous")
        };
        let pids: Vec<u32> = candidates.iter().map(|t| t.pid).collect();
        assert_eq!(pids, vec![20, 30]);
    }

    #[test]
    fn a_given_pid_chooses_that_process_or_is_not_found() {
        let all = [
            c(20, "onmyoji.exe"),
            c(30, "onmyoji.exe"),
            c(40, "renamed.exe"),
        ];
        let pid = |n| Target::Pid(nonzero(n));
        assert_eq!(select(&all, pid(30)), Ok(c(30, "onmyoji.exe")));
        assert_eq!(select(&all, pid(40)), Ok(c(40, "renamed.exe")));
        assert_eq!(
            select(&all, pid(99)),
            Err(AttachError::NoPid { pid: nonzero(99) })
        );
    }

    #[test]
    fn access_denied_means_elevation_only_when_known_unelevated() {
        let unelevated = classify_open(Some(OS_ACCESS_DENIED), Elevation::Unelevated);
        assert_eq!(unelevated, AttachError::ElevationRequired);
        assert_eq!(unelevated.failure().exit(), Exit::ElevationRequired);
        let elevated = classify_open(Some(OS_ACCESS_DENIED), Elevation::Elevated);
        assert_eq!(elevated, AttachError::AccessDenied);
        assert_eq!(elevated.failure().reason, SessionReason::AccessDenied);
        assert_eq!(elevated.failure().exit(), Exit::NotAttached);
        // Not knowing is no reason to ask the user to elevate.
        let unknown = classify_open(Some(OS_ACCESS_DENIED), Elevation::Unknown);
        assert_eq!(unknown, AttachError::DeniedUnknownElevation);
        assert_eq!(unknown.failure().reason, SessionReason::AccessDenied);
    }

    #[test]
    fn a_vanished_process_and_other_errors_are_told_apart() {
        assert_eq!(
            classify_open(Some(OS_INVALID_PARAMETER), Elevation::Unelevated),
            AttachError::Exited
        );
        let other = classify_open(Some(1450), Elevation::Unelevated).failure();
        assert_eq!(other.reason, SessionReason::Internal);
        assert_eq!(other.os_error, Some(nonzero(1450)));
        let silent = classify_open(None, Elevation::Unelevated).failure();
        assert_eq!(silent.os_error, None);
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
        let (elevation, expected) = if crate::platform::windows::is_elevated().expect("known") {
            (Elevation::Elevated, AttachError::AccessDenied)
        } else {
            (Elevation::Unelevated, AttachError::ElevationRequired)
        };
        assert_eq!(classify_open(Some(OS_ACCESS_DENIED), elevation), expected);
    }

    #[cfg(windows)]
    #[test]
    fn a_pid_that_does_not_exist_is_not_found() {
        // Process ids are multiples of four, so an odd one never exists.
        let odd = nonzero(4_000_000_001);
        assert_eq!(
            attach(Target::Pid(odd)).err(),
            Some(AttachError::NoPid { pid: odd })
        );
    }

    #[cfg(windows)]
    #[test]
    fn no_game_is_running_on_a_test_machine() {
        let all = candidates().expect("listed");
        if all.iter().any(|p| is_game(&p.image_name)) {
            return;
        }
        assert_eq!(attach(Target::Discover).err(), Some(AttachError::NoGame));
    }
}
