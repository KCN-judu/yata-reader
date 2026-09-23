"""Every check this repository runs, as one named list run by profile.

What `just fast` and `just check` run locally is what CI runs: each CI job is one profile of
this script (docs/project/ci.md). The requirements are docs/guides/engineering-requirements.md.

- Read-only: nothing is formatted or rewritten, except by the `fix` profile, which says so.
- Standard library only; tracked files come from `git ls-files` in a repository, otherwise from
  a walk of the tree filtered by .gitignore.
- A check whose tool is missing is skipped, not failed. A check that has nothing to check yet
  (no Cargo.toml) passes and says so.

Usage: python scripts/preflight.py [fast|full|fix|<ci profile>|<check>...] [--verbose]
"""

from __future__ import annotations

import fnmatch
import importlib.util
import os
import re
import shutil
import subprocess
import sys
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

PRETTIER = "prettier@3.9.9"
MARKDOWNLINT = "markdownlint-cli2@0.23.3"

MAX_LINES = 1000
# path -> (line ceiling, reason). A ceiling, so an exempt file cannot keep growing.
LENGTH_ALLOWLIST: dict[str, tuple[int, str]] = {}
GENERATED = ("/gen/", "/generated/")

# ADR-0002: the only files that may allow `unsafe`, each with its reason.
UNSAFE_MODULES: dict[str, str] = {}
UNSAFE_USE = re.compile(r"\bunsafe\s*(\{|fn\b|impl\b|trait\b|extern\b)|allow\s*\(\s*unsafe_code\s*\)")

# R1 (reader-security.md in the main repository): APIs and commands that change the system.
SYSTEM_CHANGES = re.compile(
    r"schtasks|CreateService\w*|RegSetValue\w*|RegCreateKey\w*|RegDeleteKey\w*|netsh|MpPreference|"
    r"setenforce|\bmount\s+-o\s+remount|SeDebugPrivilege|WriteProcessMemory|CreateRemoteThread\w*|"
    r"VirtualAllocEx|NtWriteVirtualMemory|ptrace"
)
# R6: nothing built is committed.
BINARY_EXT = (".exe", ".dll", ".so", ".dylib", ".a", ".lib", ".pdb", ".apk", ".dex", ".o", ".obj")


@dataclass
class Result:
    ok: bool
    output: str = ""
    skipped: bool = False


@dataclass
class Check:
    name: str
    description: str
    run: Callable[[], Result]
    tools: tuple[str, ...]
    hint: str


# ---------------------------------------------------------------- files


def _ignore_patterns() -> list[str]:
    gi = ROOT / ".gitignore"
    if not gi.exists():
        return []
    lines = gi.read_text(encoding="utf-8").splitlines()
    return [ln.strip() for ln in lines if ln.strip() and not ln.lstrip().startswith(("#", "!"))]


def _ignored(relpath: str, is_dir: bool, patterns: list[str]) -> bool:
    name = relpath.rsplit("/", 1)[-1]
    for pat in patterns:
        dir_only = pat.endswith("/")
        p = pat.rstrip("/")
        if dir_only and not is_dir:
            continue
        if "/" in p:
            if fnmatch.fnmatch(relpath, p.lstrip("/")):
                return True
        elif fnmatch.fnmatch(name, p):
            return True
    return False


def tracked_files() -> list[str]:
    """Paths relative to the root, POSIX style: what is or would be committed."""
    if (ROOT / ".git").exists() and shutil.which("git"):
        out = subprocess.run(
            ["git", "ls-files", "--cached", "--others", "--exclude-standard"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            check=True,
        ).stdout
        return sorted(ln for ln in out.splitlines() if ln and (ROOT / ln).is_file())
    patterns = [*_ignore_patterns(), ".git/"]
    found: list[str] = []
    for dirpath, dirnames, filenames in os.walk(ROOT):
        base = Path(dirpath).relative_to(ROOT).as_posix()
        base = "" if base == "." else base + "/"
        dirnames[:] = sorted(d for d in dirnames if not _ignored(base + d, True, patterns))
        found.extend(base + f for f in filenames if not _ignored(base + f, False, patterns))
    return sorted(found)


def markdown_files() -> list[str]:
    return [f for f in tracked_files() if f.endswith(".md")]


def _run(cmd: list[str], cwd: Path = ROOT) -> Result:
    exe = shutil.which(cmd[0]) or cmd[0]
    proc = subprocess.run([exe, *cmd[1:]], cwd=cwd, capture_output=True, text=True, encoding="utf-8", errors="replace")
    return Result(proc.returncode == 0, (proc.stdout + proc.stderr).strip())


def _has_rust() -> bool:
    return (ROOT / "Cargo.toml").exists()


def _read(f: str) -> str:
    return (ROOT / f).read_text(encoding="utf-8")


# ---------------------------------------------------------------- repository checks


def python_location() -> Result:
    bad = [f for f in tracked_files() if f.endswith(".py") and not f.startswith("scripts/")]
    return Result(not bad, "\n".join(f"{f}: Python outside scripts/ (the main repository's ADR-0013)" for f in bad))


def publication() -> Result:
    """Nothing ADR-0001 keeps local is about to be committed, and no binary (R6)."""
    local = ("research/", "recordings/", ".claude/")
    files = tracked_files()
    bad = [f"{f}: local-only path (ADR-0001)" for f in files if f.startswith(local)]
    bad += [f"{f}: CLAUDE.md is agent tooling" for f in files if f.endswith("CLAUDE.md")]
    bad += [f"{f}: account export, personal data (ADR-0001)" for f in files if f.endswith(".yata-export.json")]
    bad += [f"{f}: built binary in the tracked tree (R6)" for f in files if f.lower().endswith(BINARY_EXT)]
    return Result(not bad, "\n".join(bad))


def unsafe_confinement() -> Result:
    bad: list[str] = []
    for f in tracked_files():
        if f.endswith(".rs") and f not in UNSAFE_MODULES:
            for n, line in enumerate(_read(f).splitlines(), 1):
                if not line.lstrip().startswith("//") and UNSAFE_USE.search(line):
                    bad.append(f"{f}:{n}: unsafe outside the named modules (ADR-0002)")
    bad += [f"{f}: named in UNSAFE_MODULES but missing" for f in UNSAFE_MODULES if not (ROOT / f).exists()]
    return Result(not bad, "\n".join(bad))


def system_changes() -> Result:
    bad: list[str] = []
    for f in tracked_files():
        if f.endswith((".rs", ".toml")):
            for n, line in enumerate(_read(f).splitlines(), 1):
                m = SYSTEM_CHANGES.search(line)
                if m:
                    bad.append(f"{f}:{n}: '{m.group(0)}' changes the system or the game (R1, R2, R3)")
    return Result(not bad, "\n".join(bad))


def file_length() -> Result:
    bad: list[str] = []
    for f in tracked_files():
        if not f.endswith((".rs", ".py")) or any(g in f for g in GENERATED):
            continue
        if f.startswith("tests/") or "/tests/" in f or "/test/" in f:
            continue
        n = len(_read(f).splitlines())
        limit, reason = LENGTH_ALLOWLIST.get(f, (MAX_LINES, ""))
        if n > limit:
            note = f" (allowlisted to {limit}: {reason})" if reason else ""
            bad.append(f"{f}: {n} lines > {limit}{note}")
    return Result(not bad, "\n".join(bad))


def docs_validate() -> Result:
    return _run([sys.executable, "scripts/validate_docs.py"])


def status_shape() -> Result:
    page = ROOT / "docs" / "project" / "status.md"
    if not page.exists():
        return Result(False, "docs/project/status.md: missing")
    lines = page.read_text(encoding="utf-8").splitlines()
    bad: list[str] = []
    snap = next((i for i, ln in enumerate(lines) if ln.startswith("**Snapshot:**")), None)
    if snap is not None:
        n = 0
        while snap + n < len(lines) and lines[snap + n].strip():
            n += 1
        if n > 3:
            bad.append(f"status.md: snapshot is {n} lines > 3")
    header: list[str] = []
    for i, ln in enumerate(lines, 1):
        if not ln.startswith("|"):
            header = [] if not ln.strip() else header
            continue
        if re.match(r"^\|[\s|:-]+\|$", ln):
            continue
        cells = [c.strip() for c in ln.strip().strip("|").split("|")]
        if not header:
            header = [c.lower() for c in cells]
            continue
        row = dict(zip(header, cells, strict=False))
        bad += [f"status.md:{i}: cell of {len(c)} characters > 300" for c in cells if len(c) > 300]
        state, evidence = row.get("state", ""), row.get("evidence", "")
        if state in {"implemented", "tested"} and evidence in {"", "—"}:
            bad.append(f"status.md:{i}: '{state}' with no evidence")
    return Result(not bad, "\n".join(bad))


def docs_format() -> Result:
    return _run(["npx", "--yes", PRETTIER, "--check", *markdown_files()])


def docs_lint() -> Result:
    return _run(["npx", "--yes", MARKDOWNLINT, *markdown_files()])


def _module(name: str, *args: str) -> Result:
    """A Python tool run as `python -m`, skipped when it is not installed."""
    if importlib.util.find_spec(name) is None:
        return Result(True, f"{name} not installed (pip install -r requirements-dev.txt)", skipped=True)
    return _run([sys.executable, "-m", name, *args])


def python_lint() -> Result:
    a = _module("ruff", "check", "scripts")
    b = _module("ruff", "format", "--check", "scripts")
    return Result(a.ok and b.ok, "\n".join(x for x in (a.output, b.output) if x), a.skipped)


def python_types() -> Result:
    return _module("mypy", "--strict", "scripts")


# ---------------------------------------------------------------- Rust


def _rust(cmd: list[str]) -> Result:
    if not _has_rust():
        return Result(True, "no Cargo.toml yet", skipped=True)
    return _run(cmd)


def rust_format() -> Result:
    return _rust(["cargo", "fmt", "--all", "--check"])


def rust_check() -> Result:
    return _rust(["cargo", "check", "--workspace", "--all-targets", "--locked"])


def rust_clippy() -> Result:
    return _rust(["cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"])


def rust_test() -> Result:
    return _rust(["cargo", "test", "--workspace", "--locked"])


# ---------------------------------------------------------------- mutating


def fix_formatting() -> Result:
    print("fix: rewriting files with cargo fmt, prettier, markdownlint --fix, ruff format")
    steps: list[Result] = []
    if _has_rust() and shutil.which("cargo"):
        steps.append(_run(["cargo", "fmt", "--all"]))
    if shutil.which("npx"):
        md = markdown_files()
        steps.append(_run(["npx", "--yes", PRETTIER, "--write", *md]))
        steps.append(_run([sys.executable, "scripts/md_normalize.py", *md]))
        steps.append(_run(["npx", "--yes", MARKDOWNLINT, "--fix", *md]))
    if importlib.util.find_spec("ruff") is not None:
        steps.append(_run([sys.executable, "-m", "ruff", "format", "scripts"]))
    return Result(all(s.ok for s in steps), "\n".join(s.output for s in steps if s.output))


CHECKS = [
    Check("python-location", "no .py outside scripts/", python_location, (), "move it to scripts/"),
    Check("publication", "no local-only file or binary is tracked", publication, (), "keep it local (ADR-0001, R6)"),
    Check("file-length", "no hand-written file over 1 000 lines", file_length, (), "split by responsibility"),
    Check("unsafe-confinement", "unsafe only in named modules", unsafe_confinement, (), "ADR-0002"),
    Check("system-changes", "no API or command changing the system", system_changes, (), "R1-R3"),
    Check("docs-validate", "engineering records are well-formed", docs_validate, (), "python scripts/validate_docs.py"),
    Check("status-shape", "status.md is scannable; claims carry evidence", status_shape, (), "shorten the cell"),
    Check("docs-format", "Prettier would change no Markdown", docs_format, ("npx",), "just fmt"),
    Check("docs-lint", "markdownlint reports nothing", docs_lint, ("npx",), "just fmt, then fix what remains"),
    Check("python-lint", "ruff clean on scripts/", python_lint, (), "ruff check --fix scripts; just fmt"),
    Check("python-types", "mypy --strict clean on scripts/", python_types, (), "annotate; narrow Any"),
    Check("rust-format", "cargo fmt would change nothing", rust_format, ("cargo",), "cargo fmt --all"),
    Check("rust-check", "the workspace builds", rust_check, ("cargo",), "cargo check --workspace --all-targets"),
    Check("rust-clippy", "clippy with -D warnings is clean", rust_clippy, ("cargo",), "fix the lint, never allow it"),
    Check("rust-test", "workspace tests pass", rust_test, ("cargo",), "cargo test --workspace"),
    Check("fix-formatting", "MUTATES: runs every formatter", fix_formatting, (), "fix what the formatters report"),
]
STRUCTURE = [
    "python-location",
    "publication",
    "file-length",
    "unsafe-confinement",
    "system-changes",
    "docs-validate",
    "status-shape",
]
DOCS = ["docs-format", "docs-lint"]
PYTHON = ["python-lint", "python-types"]
PROFILES = {
    "fast": STRUCTURE + DOCS + PYTHON + ["rust-format", "rust-check"],
    "full": STRUCTURE + DOCS + PYTHON + ["rust-format", "rust-clippy", "rust-test"],
    "docs-ci": STRUCTURE + DOCS + PYTHON,
    "rust-ci": ["rust-format", "rust-clippy", "rust-test"],
    "fix": ["fix-formatting"],
}


def main(argv: list[str]) -> int:
    verbose = "--verbose" in argv or bool(os.environ.get("GITHUB_ACTIONS"))
    args = [a for a in argv if not a.startswith("--")] or ["fast"]
    by_name = {c.name: c for c in CHECKS}
    names: list[str] = []
    for a in args:
        if a in PROFILES:
            names += PROFILES[a]
        elif a in by_name:
            names.append(a)
        else:
            print(f"unknown profile or check '{a}'; profiles: {', '.join(PROFILES)}; checks: {', '.join(by_name)}")
            return 2
    failed: list[str] = []
    timings: list[tuple[float, str]] = []
    for name in dict.fromkeys(names):
        c = by_name[name]
        missing = [t for t in c.tools if shutil.which(t) is None]
        if missing:
            print(f"  skip  {name:<16} {', '.join(missing)} not installed")
            continue
        t0 = time.perf_counter()
        r = c.run()
        dt = time.perf_counter() - t0
        timings.append((dt, name))
        tag = "skip" if r.skipped else "ok" if r.ok else "FAIL"
        print(f"  {tag:<5} {name:<16} {dt:5.2f}s  {c.description}")
        if (not r.ok or verbose) and r.output:
            print("\n".join("        " + ln for ln in r.output.splitlines()))
        if not r.ok:
            print(f"        hint: {c.hint}")
            failed.append(name)
    slow = [f"{n} {t:.1f}s" for t, n in sorted(timings, reverse=True) if t > 5.0]
    if slow:
        print(f"slow: {', '.join(slow)}")
    print(f"preflight: {len(failed)} failed: {', '.join(failed)}" if failed else "preflight: all passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
