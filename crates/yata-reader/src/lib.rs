//! `yata-reader`: reads the player's own data from the running game, from outside and read-only,
//! and hands it to Yata's daemon over the probe protocol, or writes it to an export file.
//!
//! The rules it is held to are the main repository's `reader-security.md`; the wire is its
//! `probe-protocol.md`. `unsafe` lives only in [`platform`] (ADR-0002).
//!
//! - [`session`]: the probe protocol, over any byte stream.
//! - [`backend`]: the desktop channel, and synthetic memory for tests.
//! - [`desktop`]: choosing the game process and opening it with read rights only.
//! - [`layout`]: turning copied bytes into typed records.
//! - [`export`]: export mode.
//! - [`platform`]: the operating-system calls, behind safe functions.
//! - [`diagnostics`], [`clock`]: the reader's own log file, and the time.

pub mod backend;
pub mod clock;
pub mod desktop;
pub mod diagnostics;
pub mod export;
pub mod layout;
pub mod platform;
pub mod session;

/// This build's identity, as `HandshakeAck.probe_build_id` and exports carry it: the version and
/// the commit it was built from.
pub const BUILD_ID: &str = concat!(
    "yata-reader ",
    env!("CARGO_PKG_VERSION"),
    "+",
    env!("YATA_READER_COMMIT")
);

/// The prefix of every pipe name the daemon creates.
pub const PIPE_PREFIX: &str = r"\\.\pipe\yata-reader-";

/// A pipe name the daemon creates: [`PIPE_PREFIX`] and 32 lowercase hexadecimal digits. Any
/// other name is refused before the reader connects to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipeName(String);

impl PipeName {
    pub fn parse(name: &str) -> Option<PipeName> {
        let rest = name.strip_prefix(PIPE_PREFIX)?;
        (rest.len() == 32
            && rest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then(|| PipeName(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// On a panic, send the daemon a well-formed session-level `Failed` before the reader exits with
/// the internal exit code (`probe-protocol.md`, "Frame").
pub fn install_panic_report(out: session::Outgoing) {
    std::panic::set_hook(Box::new(move |info| {
        let f = session::SessionFailure::new(
            session::SessionReason::Internal,
            format!("the reader panicked: {info}"),
        );
        session::send_session_failure(&out, &f);
        std::process::exit(i32::from(f.exit().code()));
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_daemons_pipe_names_are_accepted() {
        // Written out rather than built from PIPE_PREFIX, so a wrong prefix fails here.
        let good = r"\\.\pipe\yata-reader-0123456789abcdef0123456789abcdef";
        assert_eq!(
            PipeName::parse(good).map(|p| p.as_str().to_owned()),
            Some(good.to_owned())
        );
        assert_eq!(PipeName::parse(&good.to_uppercase()), None);
        assert_eq!(PipeName::parse(r"\\.\pipe\other"), None);
        assert_eq!(PipeName::parse(&format!("{good}0")), None);
        assert_eq!(
            PipeName::parse(&format!(r"\\server\pipe\yata-reader-{}", "0".repeat(32))),
            None
        );
    }

    #[test]
    fn the_build_id_names_the_version_and_commit() {
        assert!(BUILD_ID.starts_with("yata-reader 0.1.0+"));
    }
}
