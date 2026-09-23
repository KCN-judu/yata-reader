---
kind: project
status: current
area: process
---

# Roadmap

What is planned next, in order, and what gates each item. The roadmap is never
evidence that anything exists; [status.md](status.md) says what exists.

| #   | Item                                                                                                                                                                                                      | Gated on                                                               |
| --- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------- |
| 1   | The crate and the `yata-protocol` dependency at a pinned revision                                                                                                                                         | the probe schema and the frame codec in the main repository            |
| 2   | The security checks that need code: the access mask test (R2), device command constants and cleanup tests (R3), hostile-input tests (R4), release override rejection (R5), `cargo-deny` network bans (R8) | 1                                                                      |
| 3   | Export mode on Windows: one read of the souls, written as a `ProbeExport`                                                                                                                                 | 1; the first recordings, taken by the maintainer with the game running |
| 4   | The named-pipe session with the daemon, and elevation on request                                                                                                                                          | 3; the daemon side in the main repository                              |
| 5   | The MuMu channel                                                                                                                                                                                          | 4                                                                      |

## Related

- The main repository's roadmap, which this one serves:
  [KCN-judu/yata](https://github.com/KCN-judu/yata/blob/main/docs/project/roadmap.md)
