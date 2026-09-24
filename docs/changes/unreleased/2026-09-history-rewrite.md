# The history of `main` was rewritten to remove agent attribution

- Date: 2026-09-24
- Area: process
- Affected: developers
- Related: ADR-0001

## What changed

- The one commit on `main` was rewritten to drop its `Co-Authored-By: Claude …`
  trailer, and force-pushed. `main` moved from `31b01e1` to `98e915e`. The tree,
  author, and date are unchanged.
- Commits carry no agent attribution from now on, under the main repository's
  ADR-0020, which this repository cites (ADR-0001).

## Compatibility and migration

A clone made before 2026-09-24 has the old history. Do not merge or pull it.
Reset it instead: `git fetch origin && git reset --hard origin/main`.

## Evidence

Checked before the push: the old and new commits have the same tree, author, and
date. The new message has no `Co-Authored-By` line.
