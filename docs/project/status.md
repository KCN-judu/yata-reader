---
kind: project
status: current
area: process
---

# Project status

What exists right now. "Implemented" means present at HEAD with a test that
exercises it.

Only the repository tooling is implemented. The reader itself is _designed_.

| Area                            | State       | What exists                                                          | What does not              | Evidence                                           |
| ------------------------------- | ----------- | -------------------------------------------------------------------- | -------------------------- | -------------------------------------------------- |
| repository tooling              | implemented | preflight profiles, docs validator, Markdown formatting, CI workflow | a first green CI run       | `scripts/preflight.py`, `.github/workflows/ci.yml` |
| crate and modules               | designed    | architecture/overview.md                                             | the crate, any module      | —                                                  |
| `unsafe` confinement (ADR-0002) | accepted    | ADR; `unsafe-confinement` check                                      | a named module             | —                                                  |
| security baseline R1–R11        | accepted    | the main repository's spec; `system-changes`, `no-binaries` checks   | the other checks and tests | —                                                  |
| desktop channel                 | designed    | architecture/overview.md                                             | any code                   | —                                                  |
| MuMu channel                    | designed    | architecture/overview.md                                             | any code                   | —                                                  |
| export mode                     | designed    | architecture/overview.md                                             | any code                   | —                                                  |

## Related

- What comes next: [roadmap.md](roadmap.md)
