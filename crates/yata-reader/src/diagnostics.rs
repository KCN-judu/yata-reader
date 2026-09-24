//! The reader's own log file (the main repository's ADR-0006, rule 8): diagnostics never go into
//! the pipe, and an elevated reader has no console anyone can read. The file lives in the reader's
//! own data directory, is the only file the reader writes besides an export the user names (R9),
//! and is rotated once it passes [`ROTATE_BYTES`].

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::clock;

/// A log file this large is moved aside when the reader starts.
pub const ROTATE_BYTES: u64 = 1024 * 1024;

pub struct Diagnostics {
    file: Option<Mutex<File>>,
}

impl Diagnostics {
    /// No log: what tests use.
    pub fn none() -> Diagnostics {
        Diagnostics { file: None }
    }

    /// The log in `dir`, rotated: `reader.log`, with the previous one kept as `reader.1.log`.
    pub fn in_dir(dir: &Path) -> Diagnostics {
        let open = || -> std::io::Result<File> {
            std::fs::create_dir_all(dir)?;
            let path = dir.join("reader.log");
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > ROTATE_BYTES) {
                std::fs::rename(&path, dir.join("reader.1.log"))?;
            }
            OpenOptions::new().create(true).append(true).open(path)
        };
        Diagnostics {
            file: open().ok().map(Mutex::new),
        }
    }

    /// The log in the reader's data directory, when the system names one.
    pub fn default_location() -> Option<PathBuf> {
        #[cfg(windows)]
        {
            crate::platform::windows::local_app_data().map(|d| d.join("yata-reader").join("logs"))
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    pub fn open_default() -> Diagnostics {
        match Diagnostics::default_location() {
            Some(dir) => Diagnostics::in_dir(&dir),
            None => Diagnostics::none(),
        }
    }

    /// One line, stamped with the time. A log that cannot be written is not an error.
    pub fn line(&self, text: &str) {
        if let Some(f) = &self.file
            && let Ok(mut f) = f.lock()
        {
            let _ = writeln!(f, "{} {text}", clock::now_rfc3339());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_log_is_moved_aside() {
        let dir = std::env::temp_dir().join(format!("yata-reader-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join("reader.log"),
            vec![b'x'; ROTATE_BYTES as usize + 1],
        )
        .expect("written");
        let d = Diagnostics::in_dir(&dir);
        d.line("started");
        let now = std::fs::read_to_string(dir.join("reader.log")).expect("log");
        assert!(now.ends_with("started\n"));
        assert!(dir.join("reader.1.log").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
