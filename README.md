# yata-reader

The reader for [Yata](https://github.com/KCN-judu/yata), a desktop tool for
players of 阴阳师 (Onmyoji). It reads the player's own account data (souls,
items, and similar) from the running game and hands it to Yata. It can also
write one reading to a file, for importing on another machine.

Nothing is released yet. [docs/project/status.md](docs/project/status.md) says
what exists.

## What it may and may not do

The reader is the only part of Yata that touches another program, and it may be
run with administrator rights, so its source is public for anyone to audit. The
rules it is held to are Yata's
[reader security baseline](https://github.com/KCN-judu/yata/blob/main/docs/spec/reader-security.md).
In short, it:

- reads the game from outside, with read rights only, and never writes to the
  game, injects code into it, or calls its functions
- has no networking code; everything it reads stays on the machine
- creates no services, scheduled tasks, registry entries, or other lasting
  changes to the system
- is built by CI from this public source, with no binary committed

Yata is an unofficial fan project, not affiliated with or endorsed by NetEase.
The game's names, data, and artwork belong to NetEase.

## Working in this repository

The requirements every change meets are
[docs/guides/engineering-requirements.md](docs/guides/engineering-requirements.md).
The toolchains are the main repository's.

```bash
just fast   # before a commit
just check  # before a push: everything CI runs
just fmt    # rewrites files with every formatter
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you state otherwise, any
contribution you submit for inclusion is dual licensed as above, without any
additional terms or conditions.
