"""Label every bare Markdown code fence as `text`, so markdownlint's MD040 holds.

Runs between Prettier and markdownlint in the `fix` profile. Mutates the files it is given.

Usage: python scripts/md_normalize.py FILE...
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

OPEN = re.compile(r"^(\s*)(```+|~~~+)\s*$")


def normalize(text: str) -> str:
    out: list[str] = []
    fence: str | None = None
    for line in text.split("\n"):
        m = re.match(r"^(\s*)(```+|~~~+)(.*)$", line)
        if m and fence is None:
            fence = m.group(2)
            bare = OPEN.match(line)
            out.append(f"{m.group(1)}{m.group(2)}text" if bare else line)
            continue
        if m and fence is not None and m.group(2).startswith(fence) and not m.group(3).strip():
            fence = None
        out.append(line)
    return "\n".join(out)


def main(paths: list[str]) -> int:
    for p in map(Path, paths):
        text = p.read_text(encoding="utf-8")
        new = normalize(text)
        if new != text:
            p.write_text(new, encoding="utf-8", newline="\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
