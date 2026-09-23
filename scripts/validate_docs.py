"""Validate the engineering records in docs/.

Checks decisions, proposals, issues, change fragments, page headers, the front door, and every
relative link in docs/ and README.md. The requirements are docs/guides/engineering-requirements.md.

Standard library only; the frontmatter subset is scalars and inline lists ([a, b]).

Usage: python scripts/validate_docs.py        exit 1 with one line per problem, else 0
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs"

AREAS = {"reading", "wire", "security", "tooling", "process"}
KINDS = {"spec", "architecture", "project", "guides", "evidence", "archive"}
PAGE_STATUS = {"current", "archived"}
ADR_STATUS = {"accepted", "superseded", "rejected", "withdrawn"}
PRP_STATUS = {"draft", "discussion", "accepted", "rejected", "withdrawn"}
ISS_STATE = {"open", "deferred", "resolved"}

RECORD_DIRS = {"decisions": "ADR", "proposals": "PRP", "issues": "ISS"}
SECTIONS = {
    "ADR": ["Status", "Context", "Decision", "Alternatives", "Consequences"],
    "PRP": [
        "Problem",
        "Goals and non-goals",
        "Proposed design",
        "Compatibility and migration",
        "Alternatives",
        "Implementation and evidence",
        "Open questions",
        "Outcome",
    ],
    "ISS": ["Problem", "Why it matters", "Current evidence", "Dependencies"],
}
REQUIRED = {
    "ADR": ["id", "status", "date", "area", "supersedes", "superseded-by"],
    "PRP": ["id", "status", "date", "area", "related-issues", "superseded-by"],
    "ISS": ["id", "state", "area", "opened", "resolved-by"],
}
CHANGE_BULLETS = ["Date", "Area", "Affected", "Related"]
CHANGE_SECTIONS = ["What changed", "Compatibility and migration", "Evidence"]

LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)\s]+)\)")
FENCE = re.compile(r"^(```|~~~)")
ID_RE = re.compile(r"^(ADR|PRP|ISS)-\d{4}$")
# Valid ids whose records are kept outside docs/ (none in this repository).
LOCAL_IDS: set[str] = set()

Frontmatter = dict[str, str | list[str]]


def rel(p: Path) -> str:
    return p.relative_to(ROOT).as_posix()


def read(p: Path) -> str:
    return p.read_text(encoding="utf-8")


def frontmatter(text: str) -> tuple[Frontmatter | None, str]:
    """The leading --- block as a dict, and the body after it."""
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return None, text
    try:
        end = next(i for i in range(1, len(lines)) if lines[i].strip() == "---")
    except StopIteration:
        return None, text
    fm: Frontmatter = {}
    for line in lines[1:end]:
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        key, sep, value = line.partition(":")
        if not sep:
            continue
        v = value.strip()
        if v.startswith("[") and v.endswith("]"):
            fm[key.strip()] = [x.strip() for x in v[1:-1].split(",") if x.strip()]
        else:
            fm[key.strip()] = v
    return fm, "\n".join(lines[end + 1 :])


def headings(body: str, level: int) -> list[str]:
    out: list[str] = []
    in_fence = False
    for line in body.splitlines():
        if FENCE.match(line):
            in_fence = not in_fence
            continue
        if not in_fence and line.startswith("#" * level + " "):
            out.append(line[level + 1 :].strip())
    return out


def section_text(body: str, name: str) -> str:
    """The text under '## name' up to the next '## '."""
    m = re.search(rf"^## {re.escape(name)}\s*$(.*?)(?=^## |\Z)", body, re.S | re.M)
    return m.group(1).strip() if m else ""


def scalar(fm: Frontmatter, key: str) -> str:
    v = fm.get(key, "")
    return v if isinstance(v, str) else ""


def items(fm: Frontmatter, key: str) -> list[str]:
    v = fm.get(key, [])
    return v if isinstance(v, list) else [v] if v else []


def check_records(problems: list[str]) -> dict[str, tuple[Path, Frontmatter]]:
    records: dict[str, tuple[Path, Frontmatter]] = {}
    for folder, prefix in RECORD_DIRS.items():
        d = DOCS / folder
        if not d.is_dir():
            continue
        for p in sorted(d.glob("*.md")):
            m = re.match(r"^(\d{4})-[a-z0-9]+(?:-[a-z0-9]+)*\.md$", p.name)
            if not m:
                problems.append(f"{rel(p)}: file name is not NNNN-kebab-slug.md")
                continue
            fm, body = frontmatter(read(p))
            if fm is None:
                problems.append(f"{rel(p)}: no frontmatter")
                continue
            for key in REQUIRED[prefix]:
                if key not in fm:
                    problems.append(f"{rel(p)}: frontmatter lacks '{key}'")
            rid = scalar(fm, "id")
            if rid != f"{prefix}-{m.group(1)}":
                problems.append(f"{rel(p)}: id '{rid}' does not match file name")
            titles = headings(body, 1)
            if len(titles) != 1 or not titles[0].startswith(f"{rid}: "):
                problems.append(f"{rel(p)}: needs one title '# {rid}: ...'")
            if rid in records:
                problems.append(f"{rel(p)}: duplicate id {rid}")
            records[rid] = (p, fm)
            if scalar(fm, "area") not in AREAS:
                problems.append(f"{rel(p)}: unknown area '{scalar(fm, 'area')}'")
            status_key = "state" if prefix == "ISS" else "status"
            allowed = {"ADR": ADR_STATUS, "PRP": PRP_STATUS, "ISS": ISS_STATE}[prefix]
            if scalar(fm, status_key) not in allowed:
                problems.append(f"{rel(p)}: unknown {status_key} '{scalar(fm, status_key)}'")
            present = headings(body, 2)
            for s in SECTIONS[prefix]:
                if s not in present:
                    problems.append(f"{rel(p)}: missing section '## {s}'")
            if prefix == "ISS" and scalar(fm, "state") in {"resolved", "deferred"}:
                if not section_text(body, "Resolution"):
                    problems.append(f"{rel(p)}: {scalar(fm, 'state')} but Resolution is empty")
                if scalar(fm, "state") == "resolved" and not items(fm, "resolved-by"):
                    problems.append(f"{rel(p)}: resolved but resolved-by is empty")
            outcome = section_text(body, "Outcome")
            if prefix == "PRP" and scalar(fm, "status") == "accepted" and not re.search(r"ADR-\d{4}", outcome):
                problems.append(f"{rel(p)}: accepted but Outcome names no ADR")
    return records


def check_links_between(records: dict[str, tuple[Path, Frontmatter]], problems: list[str]) -> None:
    for rid, (p, fm) in records.items():
        for key in ("supersedes", "superseded-by", "resolved-by", "related", "related-issues"):
            for ref in items(fm, key):
                if not ID_RE.match(ref):
                    problems.append(f"{rel(p)}: {key} entry '{ref}' is not a record id")
                elif ref not in records and ref not in LOCAL_IDS:
                    problems.append(f"{rel(p)}: {key} names {ref}, which does not exist")
        if rid.startswith("ADR"):
            for old in items(fm, "supersedes"):
                if old in records and rid not in items(records[old][1], "superseded-by"):
                    problems.append(f"{rel(p)}: supersedes {old}, but {old} lacks superseded-by {rid}")
            for new in items(fm, "superseded-by"):
                if new in records and rid not in items(records[new][1], "supersedes"):
                    problems.append(f"{rel(p)}: superseded-by {new}, but {new} does not list it in supersedes")
            if scalar(fm, "status") == "superseded" and not items(fm, "superseded-by"):
                problems.append(f"{rel(p)}: superseded but names no replacement")


def check_changes(problems: list[str]) -> list[Path]:
    found: list[Path] = []
    for p in sorted((DOCS / "changes").rglob("*.md")):
        found.append(p)
        text = read(p)
        if p.parent.name == "unreleased" and not re.match(r"^\d{4}-\d{2}-[a-z0-9-]+\.md$", p.name):
            problems.append(f"{rel(p)}: file name is not YYYY-MM-slug.md")
        for b in CHANGE_BULLETS:
            if not re.search(rf"^- {b}: \S", text, re.M):
                problems.append(f"{rel(p)}: missing '- {b}:' line")
        present = headings(text, 2)
        for s in CHANGE_SECTIONS:
            if s not in present:
                problems.append(f"{rel(p)}: missing section '## {s}'")
    return found


def check_pages(problems: list[str]) -> list[Path]:
    pages: list[Path] = []
    for p in sorted(DOCS.rglob("*.md")):
        top = p.relative_to(DOCS).parts
        if len(top) == 1:
            if p.name != "README.md":
                problems.append(f"{rel(p)}: loose page at the top of docs/")
            continue
        if top[0] in RECORD_DIRS or top[0] == "changes":
            continue
        pages.append(p)
        fm, _ = frontmatter(read(p))
        if fm is None:
            problems.append(f"{rel(p)}: no kind/status/area header")
            continue
        if scalar(fm, "kind") not in KINDS:
            problems.append(f"{rel(p)}: unknown kind '{scalar(fm, 'kind')}'")
        elif scalar(fm, "kind") != top[0]:
            problems.append(f"{rel(p)}: kind '{scalar(fm, 'kind')}' but lives in docs/{top[0]}/")
        if scalar(fm, "status") not in PAGE_STATUS:
            problems.append(f"{rel(p)}: unknown status '{scalar(fm, 'status')}'")
        if scalar(fm, "area") not in AREAS:
            problems.append(f"{rel(p)}: unknown area '{scalar(fm, 'area')}'")
    return pages


def link_targets(p: Path) -> list[str]:
    out: list[str] = []
    in_fence = False
    for line in read(p).splitlines():
        if FENCE.match(line):
            in_fence = not in_fence
            continue
        if not in_fence:
            out.extend(LINK.findall(re.sub(r"`[^`]*`", "", line)))
    return out


def check_links(files: list[Path], problems: list[str]) -> None:
    for p in files:
        for target in link_targets(p):
            if re.match(r"^[a-z][a-z0-9+.-]*:", target) or target.startswith("#"):
                continue
            path = target.split("#", 1)[0]
            if not (p.parent / path).exists():
                problems.append(f"{rel(p)}: link '{target}' does not resolve")


def check_front_door(listed: list[Path], problems: list[str]) -> None:
    door = DOCS / "README.md"
    if not door.exists():
        problems.append("docs/README.md: missing")
        return
    linked = {(door.parent / t.split("#", 1)[0]).resolve() for t in link_targets(door)}
    for p in listed:
        if p.resolve() not in linked:
            problems.append(f"docs/README.md: does not list {rel(p)}")


def main() -> int:
    problems: list[str] = []
    records = check_records(problems)
    check_links_between(records, problems)
    changes = check_changes(problems)
    pages = check_pages(problems)
    md = [*sorted(DOCS.rglob("*.md")), ROOT / "README.md"]
    check_links([p for p in md if p.exists()], problems)
    check_front_door(pages + [p for p, _ in records.values()] + changes, problems)
    for line in problems:
        print(line)
    if problems:
        return 1
    print(f"docs: {len(records)} records, {len(pages)} pages, {len(changes)} change records; all well-formed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
