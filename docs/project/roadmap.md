---
kind: project
status: current
area: process
---

# Roadmap

What is planned next, in order, and what gates each item. The roadmap is never
evidence that anything exists; [status.md](status.md) says what exists.

| #   | Item                                                                                                                                                                         | Gated on                                           |
| --- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------- |
| 1   | The game itself: its process names, its runtime's layout, and the first recordings (local only); then the soul record's typed fields, each stated established with its basis | the game running on the maintainer's machine       |
| 2   | The remaining security checks: release override rejection (R5), `cargo-deny` network bans (R8)                                                                               | —                                                  |
| 3   | The MuMu channel, with its device command constants, cleanup, and hostile-input tests (R3, R4)                                                                               | 1; the main repository's roadmap after milestone 1 |

## Related

- The main repository's roadmap, which this one serves:
  [KCN-judu/yata](https://github.com/KCN-judu/yata/blob/main/docs/project/roadmap.md)
