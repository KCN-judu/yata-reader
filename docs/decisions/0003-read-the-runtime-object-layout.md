---
id: ADR-0003
status: accepted
date: 2026-09-25
area: reading
supersedes: []
superseded-by: []
related: [ADR-0001, ADR-0002]
---

# ADR-0003: The desktop channel reads the game's embedded runtime by that runtime's public object layout, checks the layout at attach, and reports records verbatim

## Status

Accepted, 2026-09-25.

## Context

The main repository's ADR-0007 decides that the reader reads the game from
outside, with read rights only, and interprets its data structures in the
reader's own address space. It leaves open how the reader finds and interprets
them. Three facts bound that choice.

**The game's data lives in the objects of an embedded runtime, CPython,** by the
prior tool's account: a hypothesis until a recording of the game shows it, and
one the attach checks below test on every run. Those objects have a layout the
runtime publishes in its own source, per version. A reader that interprets them
by that layout depends on the runtime version, which changes rarely, not on the
game's code, which changes with every patch.

**Nothing here can be tested against the game in CI.** Whatever is read must be
testable on bytes that are not the game's: synthetic memory, and a real process
of the same runtime.

**What the reader knows about the game's records is partly inherited.** The
prior tool's rule for recognising a soul is a hypothesis (the main repository's
ADR-0014), and no mapping from a record's entries to a soul's fields has been
re-established by this project's own recordings.

## Decision

1. **Types are found, not assumed.** At attach the reader finds the runtime's
   `type` object — the one object in a loaded image that is its own type and is
   named `type` — and from it each builtin type it decodes, each exactly once in
   loaded images. Pointers are compared against these addresses; no address or
   offset of the game's is hard-coded.
2. **The layout is checked at attach, never guessed.** Each builtin type's
   instance sizes must be those of the layout the reader implements, and the
   dict key table must have one of the two shapes that layout has. Any
   difference is `probe.layout_mismatch`, with what differed, and nothing is
   read.
3. **Only builtin value kinds are decoded.** Null, booleans, integers that fit
   64 bits, floats, texts, lists, tuples, and dicts; anything else is reported
   unread with its type name and the reason. Every read is bounded by the named
   limits in `layout::limits`.
4. **Records are reported verbatim.** A record is recognised by a stated rule
   and reported with every entry it holds, in the main repository's
   `ObservedRecord`. A typed field of the record is filled only when the reading
   states a `Mapping` for it with the evidence behind it, as the main
   repository's `probe-protocol.md`, "Evidence", requires; a field not yet
   re-established has no mapping and stays unfilled.
5. **Parsing is safe code over a memory trait.** `layout` reads through
   `layout::memory::Memory`, which the game process, a synthetic image, and any
   test implement alike. Only `platform::windows` touches the operating system
   (ADR-0002).
6. **Two tests stand in for the game.** Synthetic images built by
   `layout::synthetic` exercise every path, including both key table shapes and
   every unread reason, and produce the public fixtures. A real interpreter of
   the implemented layout, holding a made-up inventory, is read from outside in
   the Windows CI job.

## Alternatives

- **Byte patterns for the game's own structures.** Rejected: they move with
  every game patch, and a pattern that matches the wrong bytes reads garbage
  with nothing to say so.
- **Finding types through the runtime library's exported symbols.** Rejected for
  now: listing a process's modules needs more than the two access rights the
  reader holds (the main repository's R2). The self-typed `type` object is found
  with read rights alone.
- **Mapping every record field from the prior tool's descriptions at once.**
  Rejected: it would turn hypotheses into typed values that look established.
  The observed record carries the same information without the claim.

## Consequences

**Easier.** A game patch that keeps the runtime version needs no change to the
decoder. A runtime version change is reported as a layout mismatch naming the
check that failed, not as wrong values. Every decoding path runs in CI.

**Harder.** Records are larger than typed ones, because they carry every entry.
A new runtime layout is a new decoder, found by attach and tested the same way.
Which objects are the game's souls still rests on the inherited recognition rule
until a recording of the game re-establishes it.
