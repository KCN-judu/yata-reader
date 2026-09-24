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
| `rust` | Windows, Linux | `rust-ci` | formatting, Clippy with `-D warnings`, the tests, the fixtures reproduced byte for byte                                                                                                                        |

The reader's desktop channel is Windows-only, so the Rust job runs on Windows as
well as Linux. On Windows the job also installs a CPython 3.11, apart from the
tooling Python, and names it in `YATA_TEST_PYTHON`, so `tests/real_cpython.rs`
reads a real process from outside. Locally that test runs when the variable is
set and says it did not run otherwise; the profile is the same.

## Related

- The checks and their hints: `scripts/preflight.py`
