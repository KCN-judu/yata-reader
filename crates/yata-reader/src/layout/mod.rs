//! Turning another process's bytes into typed records: safe code over a [`memory::Memory`], so
//! every path runs in tests on synthetic memory (ADR-0002, rule 4).
//!
//! - [`memory`]: regions and reads, over the game or over bytes held here.
//! - [`cpython`]: the embedded runtime's object layout, checked at attach.
//! - [`souls`]: the souls scope.
//! - [`limits`]: every bound the parsers read under.
//! - [`synthetic`], [`fixture`]: a synthetic runtime and the inventory the public fixtures hold.

pub mod cpython;
pub mod fixture;
pub mod limits;
pub mod memory;
pub mod souls;
pub mod synthetic;
