---
kind: project
status: current
area: process
---

# docs/

Front door. Every page in this directory is listed here. The requirements every
change must meet are
[guides/engineering-requirements.md](guides/engineering-requirements.md).

The reader's wire, formats, and security baseline are decided in the main
repository, [KCN-judu/yata](https://github.com/KCN-judu/yata), and are cited
here, never restated (ADR-0001).

## Architecture — `architecture/`

- [architecture/overview.md](architecture/overview.md) — where the reader sits,
  its crate and modules, what CI can and cannot see

## Decisions — `decisions/`

- [decisions/0001-rules-live-in-the-main-repository.md](decisions/0001-rules-live-in-the-main-repository.md)
  — the main repository is the authority for the wire, the formats, and the
  security baseline; this repository implements them
- [decisions/0002-unsafe-confined-to-named-modules.md](decisions/0002-unsafe-confined-to-named-modules.md)
  — `unsafe` is denied except in named platform modules, each block with a
  stated invariant

## Guides — `guides/`

- [guides/engineering-requirements.md](guides/engineering-requirements.md) — the
  requirements every change must meet

## Project — `project/`

- [project/status.md](project/status.md) — what exists and what does not
- [project/roadmap.md](project/roadmap.md) — what comes next and what gates it
- [project/ci.md](project/ci.md) — what each CI job proves

## Changes — `changes/`

- [changes/unreleased/2026-09-repository-tooling.md](changes/unreleased/2026-09-repository-tooling.md)
  — records and tooling
- [changes/unreleased/2026-09-history-rewrite.md](changes/unreleased/2026-09-history-rewrite.md)
  — `main` rewritten to remove agent attribution; reset old clones

## Local only — not published

`research/` (notes on how the game is read) and `recordings/` (recordings and
exports of a real account) live beside `docs/` in the working copy and are
excluded from the repository (ADR-0001).
