---
id: ADR-0001
status: accepted
date: 2026-09-24
area: process
supersedes: []
superseded-by: []
related: []
---

# ADR-0001: The reader's contract and rules live in the main repository; this repository implements them

## Status

Accepted, 2026-09-24.

## Context

`yata-reader` is the program that reads the game for Yata. It is a separate
repository because it needs `unsafe` code and the main workspace forbids it. The
main repository, [KCN-judu/yata](https://github.com/KCN-judu/yata), has already
decided most of what the reader is:

- the wire it speaks and the recording format: `docs/spec/probe-protocol.md` and
  its ADR-0006
- that it is open source, reads from outside without writing to the game, and
  meets a security baseline: its ADR-0007 and `docs/spec/reader-security.md`
- the export file used on macOS: its ADR-0008
- the languages allowed and the Python boundary: its ADR-0013
- that nothing from any earlier tool is ported: its ADR-0014
- what is kept off public repositories: its ADR-0016

If this repository restated those rules, the two copies would drift, and a
reader of one would not know which is current.

## Decision

1. **The main repository is the authority** for the reader's wire, its recording
   and export formats, its security baseline (R1–R11), and every decision listed
   above. This repository cites them by the main repository's path and record
   id, and never restates them.
2. **This repository's records cover only how the reader is built**: its crate
   layout, how it confines `unsafe`, its internal module boundaries, its tests,
   and its release build. They follow the same engineering standard as the main
   repository
   ([guides/engineering-requirements.md](../guides/engineering-requirements.md)).
3. **The consumer defines the contract.** A change to the wire starts as a
   change to `probe-protocol.md` in the main repository. The reader follows it,
   through the `yata-protocol` crate it depends on at a pinned revision.
4. **Two kinds of material stay off this repository**, as in the main one:
   - notes on how the game's data is laid out and found, kept in a local
     `research/` folder on the maintainer's machine
   - recordings and exports of a real account, which are personal data, kept in
     a local `recordings/` folder

   The code that reads the game is public (the main repository's ADR-0007
   accepts that the source shows how the game is read). The notes behind it are
   not published, and the public records describe the reader's structure, not
   the game's.

## Alternatives

- **Restate the rules here.** Rejected: two copies of the security baseline is
  exactly the drift this record prevents.
- **Keep the protocol here and have the main repository follow.** Rejected: the
  main repository is the consumer, and it tests the decoder against recordings.
  The side that tests the contract defines it.

## Consequences

**Easier.** An auditor reads one security baseline. A protocol change has one
source.

**Harder.** Reading this repository alone does not tell the whole story. Every
cross-repository change touches two clones and two CI runs, which the main
repository accepted when it split the reader out.
