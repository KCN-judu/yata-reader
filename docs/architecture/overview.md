---
kind: architecture
status: current
area: reading
---

# Architecture overview

How the reader is put together. Everything here is _designed_. Nothing is
_implemented_ — see [../project/status.md](../project/status.md).

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

One crate, `yata-reader`, a binary. It depends on `yata-protocol` from the main
repository at a pinned revision, for the frame codec and the generated message
types. A module becomes a crate only when one of the main repository's split
triggers holds (its ADR-0005).

## Modules

| Module     | Owns                                                                                              | Safe             |
| ---------- | ------------------------------------------------------------------------------------------------- | ---------------- |
| `session`  | the command line, the handshake, request dispatch, cancellation, shutdown                         | yes              |
| `pipe`     | connecting to the daemon's named pipe; frames in and out through `yata-protocol`                  | yes              |
| `desktop`  | finding the game process and opening it with read rights only; copying bytes out                  | named (ADR-0002) |
| `mumu`     | the emulator channel: adb commands as constants, temporary root, reading through `/proc`, cleanup | yes              |
| `layout`   | turning copied bytes into typed records; every read bounded by named limits                       | yes              |
| `export`   | export mode: one read, written as proto3 JSON to the file the user named                          | yes              |
| `platform` | the operating-system calls `desktop` needs, behind safe functions                                 | named (ADR-0002) |

The rule the table encodes: bytes cross from the game into the reader only
through `desktop` or `mumu`, and are interpreted only in `layout`, which is safe
code tested on recorded bytes.

## What CI can and cannot see

| Part                                     | In CI | How                                                        |
| ---------------------------------------- | ----- | ---------------------------------------------------------- |
| `layout`, `session`, `export`            | yes   | unit tests on recorded bytes and on synthetic frames       |
| the security checks R1–R11               | yes   | preflight checks and tests, named per requirement          |
| `desktop` and `mumu` against a live game | never | by the maintainer, with the game running; each run records |

A recording made while the game can be read is what keeps the parsers testable
after the game changes.

## Related

- Why the rules live in the main repository:
  [../decisions/0001-rules-live-in-the-main-repository.md](../decisions/0001-rules-live-in-the-main-repository.md)
- How `unsafe` is confined:
  [../decisions/0002-unsafe-confined-to-named-modules.md](../decisions/0002-unsafe-confined-to-named-modules.md)
- The requirements:
  [../guides/engineering-requirements.md](../guides/engineering-requirements.md)
