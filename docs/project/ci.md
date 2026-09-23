---
kind: project
status: current
area: tooling
---

# Continuous integration

What each CI job proves. Every job runs one profile of `scripts/preflight.py`,
and the same profile runs locally with `just run <profile>`.

| Job    | Runs on        | Profile   | Proves                                                                                                                                                                                                         |
| ------ | -------------- | --------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `docs` | Linux          | `docs-ci` | records are well-formed; Markdown is formatted and lint-clean; scripts are typed and lint-clean; nothing local-only or binary is tracked; `unsafe` stays in named modules; no system-changing API appears (R1) |
| `rust` | Windows, Linux | `rust-ci` | formatting, Clippy with `-D warnings`, the tests; passes with nothing to check until the crate exists                                                                                                          |

The reader's desktop channel is Windows-only, so the Rust job runs on Windows as
well as Linux.

## Related

- The checks and their hints: `scripts/preflight.py`
