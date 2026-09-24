# The reader reads souls from the desktop game, serves the daemon, and exports

- Date: 2026-09-25
- Area: reading
- Affected: developers, the main repository's daemon
- Related: ADR-0002, ADR-0003

## What changed

- The `yata-reader` crate exists: `yata-reader pipe <name>` serves the daemon on
  its named pipe, `yata-reader export <file> [--pid <n>]` writes one reading to
  a new export file, and `yata-reader processes` lists the processes it would
  consider as the game.
- The desktop channel chooses the game process deterministically, opens it with
  read rights only, and tells a missing, ambiguous, refused, exited, or 32-bit
  target apart, each with its code and exit code.
- The runtime's layout is found and checked at attach (ADR-0003); the souls
  scope reports every recognised record verbatim, with the recognition rule
  stated as inherited and no typed field filled.
- `unsafe` lives in one named module, `platform/windows.rs` (ADR-0002).
- The public fixtures, a session recording and an export of a synthetic
  inventory, are reproduced byte for byte by the tests.

## Compatibility and migration

None: this is the first code in the repository. The Windows CI job installs a
CPython 3.11 for `tests/real_cpython.rs`; locally, set `YATA_TEST_PYTHON` to
such an interpreter to run it.

## Evidence

[../../project/status.md](../../project/status.md).
