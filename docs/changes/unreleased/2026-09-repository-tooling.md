# The repository has its records and tooling

- Date: 2026-09-24
- Area: tooling
- Affected: developers
- Related: ADR-0001, ADR-0002

## What changed

- `just fast`, `just check`, and `just fmt` run the named checks of
  `scripts/preflight.py`; CI runs the same checks by profile
  ([../../project/ci.md](../../project/ci.md)).
- The records say what this repository decides and what it takes from the main
  repository (ADR-0001), and how `unsafe` is confined (ADR-0002).

## Compatibility and migration

Install the Python tooling with `pip install -r requirements-dev.txt`. Prettier
and markdownlint run through `npx`, which needs Node.

## Evidence

`just fast` passes on the maintainer's Windows machine.
