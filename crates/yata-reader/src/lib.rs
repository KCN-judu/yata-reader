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

/// Whether a name is one the daemon creates: its prefix and 32 lowercase hexadecimal digits.
/// Anything else is refused before the reader connects to it.
pub fn valid_pipe_name(name: &str) -> bool {
    name.strip_prefix(PIPE_PREFIX).is_some_and(|rest| {
        rest.len() == 32
            && rest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// On a panic, send the daemon a well-formed session-level `Failed` before the reader exits with
/// code 4 (`probe-protocol.md`, "Frame").
pub fn install_panic_report(out: session::Outgoing) {
    std::panic::set_hook(Box::new(move |info| {
        let error = session::Failure::new(
            yata_protocol::probe::code::INTERNAL,
            format!("the reader panicked: {info}"),
            yata_protocol::probe::exit::INTERNAL,
        )
        .error();
        let message =
            yata_protocol::probe::probe_message::Kind::Failed(yata_protocol::probe::Failed {
                request_id: 0,
                error: Some(error),
            });
        let _ = session::send(&out, message);
        std::process::exit(i32::from(yata_protocol::probe::exit::INTERNAL));
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_daemons_pipe_names_are_accepted() {
        // Written out rather than built from PIPE_PREFIX, so a wrong prefix fails here.
        let good = r"\\.\pipe\yata-reader-0123456789abcdef0123456789abcdef";
        assert!(valid_pipe_name(good));
        assert!(!valid_pipe_name(&good.to_uppercase()));
        assert!(!valid_pipe_name(r"\\.\pipe\other"));
        assert!(!valid_pipe_name(&format!("{good}0")));
        assert!(!valid_pipe_name(&format!(
            r"\\server\pipe\yata-reader-{}",
            "0".repeat(32)
        )));
    }

    #[test]
    fn the_build_id_names_the_version_and_commit() {
        assert!(BUILD_ID.starts_with("yata-reader 0.1.0+"));
    }
}
