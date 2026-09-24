---
kind: architecture
status: current
area: reading
---

# Architecture overview

How the reader is put together. What exists is in
[../project/status.md](../project/status.md).

## Where the reader sits

```text
  game process ◄── reads ── yata-reader ── frames over a named pipe ──► yata-daemon
  (read only)               (this repository)                           (main repository)
                                 │
                                 └── export mode ──► one JSON file the user names
```

The daemon starts the reader, gives it a pipe name, and reads frames from it.
The frames, their messages, and the recording format are the main repository's
`docs/spec/probe-protocol.md`. The export file is the same schema's
`ProbeExport` message in proto3 JSON (the main repository's ADR-0008).

## Crate

One crate, `yata-reader`, a library and the binary over it. It depends on
`yata-protocol` from the main repository at a pinned revision, for the frame
codec, the generated message types, the export file's JSON mapping, the request
discipline, and the error and exit codes. A module becomes a crate only when one
of the main repository's split triggers holds (its ADR-0005).

## Modules

| Module        | Owns                                                                                                | Safe             |
| ------------- | --------------------------------------------------------------------------------------------------- | ---------------- |
| `session`     | the handshake, request dispatch on a worker thread, cancellation, shutdown, over any byte stream    | yes              |
| `backend`     | the desktop channel and synthetic memory, behind one trait the session drives                       | yes              |
| `desktop`     | choosing the game process deterministically; opening it with read rights only; classifying refusals | yes              |
| `layout`      | turning copied bytes into records (ADR-0003); every read bounded by `layout::limits`                | yes              |
| `export`      | export mode: one read, written as proto3 JSON to a new file the user names                          | yes              |
| `diagnostics` | the reader's own log file, rotated, in its own data directory                                       | yes              |
| `platform`    | the operating-system calls `desktop` needs, behind safe functions                                   | named (ADR-0002) |
| `main`        | the command line; connecting to the daemon's named pipe after checking its name                     | yes              |

The rule the table encodes: bytes cross from the game into the reader only
through `platform`, and are interpreted only in `layout`, which is safe code
tested on synthetic memory and on a real process of the runtime it reads.

The MuMu channel is not built yet.

## What CI can and cannot see

| Part                                     | In CI | How                                                                                   |
| ---------------------------------------- | ----- | ------------------------------------------------------------------------------------- |
| `layout`, `session`, `export`            | yes   | tests on synthetic memory and synthetic frames; the fixtures reproduced byte for byte |
| `desktop` against a real process         | yes   | the Windows job reads a real interpreter of the runtime's layout from outside         |
| the security checks R1–R11               | yes   | preflight checks and tests, named per requirement                                     |
| `desktop` and `mumu` against a live game | never | by the maintainer, with the game running; each run records                            |

A recording made while the game can be read is what keeps the parsers testable
after the game changes.

## Related

- Why the rules live in the main repository:
  [../decisions/0001-rules-live-in-the-main-repository.md](../decisions/0001-rules-live-in-the-main-repository.md)
- How `unsafe` is confined:
  [../decisions/0002-unsafe-confined-to-named-modules.md](../decisions/0002-unsafe-confined-to-named-modules.md)
- The requirements:
  [../guides/engineering-requirements.md](../guides/engineering-requirements.md)
