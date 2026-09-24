---
kind: guides
status: current
area: process
---

# Engineering requirements

What every change to this repository must satisfy. They are the main
repository's requirements, adapted to a reader: the same record system, the same
verification contract, and a different rule for `unsafe`.

## Records

- Each kind of fact has one home: how the reader is built in
  `docs/architecture/`, why a choice was made in `docs/decisions/`, what exists
  in `docs/project/status.md`, what changed for someone in `docs/changes/`. A
  fact is written once.
- The wire, the formats, and the security baseline are the main repository's
  (ADR-0001). They are cited, never restated.
- A decision record is append-only. A changed decision gets a new record that
  supersedes the old one.
- Claims use exactly these words: _intended_, _designed_, _accepted_,
  _implemented_ (present at HEAD), _tested_ (a named test exercises it).
- Every page in `docs/` carries its header and is listed in `docs/README.md`.
  Relative links resolve.

## Language and naming

- Records, identifiers, commit messages, file names, and log targets are
  English.
- File names are English kebab-case. Terms follow the main repository's
  glossary.
- Product identifiers take the `yata` prefix.

## Safety

- `unsafe` is denied in the workspace and allowed only in the modules the
  `unsafe-confinement` check names, each block with a `// SAFETY:` comment
  (ADR-0002).
- The reader meets R1–R11 of the main repository's
  `docs/spec/reader-security.md`. Each requirement's check is a named preflight
  check or test here.
- No networking code, no persistence on the user's system, no committed binary.

## Code

- Errors are structured values carrying typed fields, returned in the type.
- Input read from the game or the emulator is checked against named limits
  before allocation, and never enters a command unvalidated.
- Hand-written source files stay at or under 1 000 lines, and are split by
  responsibility.
- Comments say why, not what.

## Verification

- The toolchains are pinned.
- Formatting is done by tools, never by hand.
- Every check CI runs can be run locally with one command, and CI runs nothing
  else.

## Commits

- Commit messages follow Conventional Commits 1.0.0 (the main repository's
  ADR-0021). This repository's scopes name its own parts; `docs` commits take
  the folder under `docs/`.
- One logical change per commit, at a point where the tree builds and the fast
  checks pass.
- A change to a record goes in the same commit as the change it records.
- Commits and pull requests carry no agent attribution: no `Co-Authored-By`
  trailer or "Generated with" line naming an agent or model (the main
  repository's ADR-0020).

## Publication

- The local `research/` and `recordings/` folders and agent tooling are never
  committed (ADR-0001).
