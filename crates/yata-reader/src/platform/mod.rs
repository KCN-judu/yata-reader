//! The operating-system calls the desktop channel needs, behind safe functions (ADR-0002). Only
//! Windows has them; elsewhere the desktop channel reports an unsupported environment.

#[cfg(windows)]
pub mod windows;
