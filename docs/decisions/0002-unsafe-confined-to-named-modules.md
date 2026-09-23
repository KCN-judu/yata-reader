---
id: ADR-0002
status: accepted
date: 2026-09-24
area: reading
supersedes: []
superseded-by: []
related: [ADR-0001]
---

# ADR-0002: `unsafe` is denied everywhere except in named platform modules, and every block states why it is sound

## Status

Accepted, 2026-09-24.

## Context

The main repository forbids `unsafe` in its workspace and moved the code that
needs it here. The need is real but narrow: opening another process and copying
bytes out of it calls operating-system functions that Rust can only reach
through `unsafe`. Everything else the reader does is parsing bytes, framing
messages, and writing a file, none of which needs it.

A workspace-wide `forbid` cannot be used here, because some module must call
those functions. A workspace with no rule at all would let `unsafe` spread into
the parsing code, which handles data the game controls and is the code most
likely to have a bug.

## Decision

1. **The workspace denies `unsafe`.** `[workspace.lints.rust]` sets
   `unsafe_code = "deny"`. Every crate inherits the workspace lints with
   `[lints] workspace = true` and declares nothing else.
2. **Only named modules may allow it.** A module that calls an operating-system
   API needing `unsafe` starts with `#![allow(unsafe_code)]`, and its path is
   listed in the `unsafe-confinement` check of `scripts/preflight.py` with a
   one-line reason. An `unsafe` block or an `allow(unsafe_code)` in any other
   file fails preflight.
3. **Every `unsafe` block says why it is sound.** Clippy's
   `undocumented_unsafe_blocks` and `multiple_unsafe_ops_per_block` are denied:
   a block holds one unsafe operation and a `// SAFETY:` comment stating the
   invariant it relies on.
4. **Named modules expose safe functions only.** Their public functions take and
   return owned, checked values (a process handle wrapper, a `Vec<u8>` of the
   bytes read). Parsing never happens inside them. The parsers are safe code and
   are tested on recorded bytes.
5. **No `unsafe` for performance.** A named module exists only for an
   operating-system call. Speed is never a reason to add one.

## Alternatives

- **`forbid` with the platform calls in a third-party crate.** Rejected: the
  unsafe code would move out of sight, not away. A reviewer of this repository
  could no longer read it.
- **No workspace rule, review only.** Rejected: the main repository's history of
  such rules is that an unenforced one is a decoration.
- **A separate crate per platform module.** Not needed yet. The main
  repository's split triggers apply (its ADR-0005); a platform module becomes a
  crate when one of them holds.

## Consequences

**Easier.** An auditor finds every `unsafe` block by reading the files the
preflight list names. The parsing code, where game-controlled data enters, is
safe Rust.

**Harder.** Adding a platform call means adding a list entry with a reason, and
every block needs a written invariant.
